//! The orchestrator.

mod execution;
mod feedback;
mod hearing;
mod teaching;

use execution::Attempt;

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use spoon_concept::{Concept, ConceptMeta, Provenance, SymbolTable, Tier, holes, render};
use spoon_ears::{NativeEars, PhrasingIndex};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_infer::{DeriveBudget, DiscriminationTree, Engine};
use spoon_learn::{SynthBudget, SynthOutcome, synthesize};
use spoon_seat::{
    Ears, Heard, Mouth, Seat, SeatCounters, Spec, Teacher, TeacherAsk, TeacherReply, Turn,
};
use spoon_store::Store;
use spoon_store::pairs::PairSource;

use crate::episode::{EarsPath, Episode, MouthPath, TurnMetrics};
use crate::resolve::{Move, resolve};

#[derive(Debug, Clone)]
pub struct BrainConfig {
    pub permission: PermissionMode,
    pub eval_budget: Budget,
    pub derive_budget: DeriveBudget,
    /// How many concept names to offer the ears each turn.
    ///
    /// The prompt is working memory, not a dictionary. Showing everything Spoon
    /// knows buries the handful of concepts this user actually reaches for.
    pub vocabulary_size: usize,
    /// Let the Teacher fill gaps. Off means Spoon says what it cannot do
    /// instead of going and finding out.
    pub teaching: bool,
    /// Check one in this many otherwise-clean model readings.
    ///
    /// A reading that is wrong and still evaluates is the worst case, because
    /// it produces a confident wrong answer and reports no gap at all, so the
    /// only way to catch it is to check readings that went fine. Checking
    /// every one of them was affordable when the Teacher was the same small
    /// model as the ears. Against a 27b model it costs tens of seconds a turn,
    /// which is most of the time a long run spends.
    ///
    /// So: always check a turn that visibly went wrong, and sample the rest.
    /// Silent wrong readings still get caught, just not all in the same hour.
    /// 1 checks everything, 0 checks none of the clean ones.
    pub check_clean_readings: u32,
}

impl Default for BrainConfig {
    fn default() -> Self {
        BrainConfig {
            permission: PermissionMode::AskWrites,
            eval_budget: Budget::default(),
            derive_budget: DeriveBudget::default(),
            vocabulary_size: 250,
            teaching: true,
            check_clean_readings: 8,
        }
    }
}

/// The three seats, plus the counters that prove the interior used none of
/// them.
pub struct Seats {
    pub ears: Box<dyn Ears>,
    pub mouth: Box<dyn Mouth>,
    /// Absent means Spoon says what it cannot do rather than going to find out.
    pub teacher: Option<Box<dyn Teacher>>,
    pub counters: Arc<SeatCounters>,
}

#[derive(Debug, Clone)]
pub struct TurnResult {
    pub reply: String,
    pub episode: Episode,
}

/// Everything one Spoon is.
pub struct Brain {
    store: Store,
    registry: NativeRegistry,
    symbols: Arc<SymbolTable>,
    ears: Box<dyn Ears>,
    mouth: Box<dyn Mouth>,
    teacher: Option<Box<dyn Teacher>>,
    counters: Arc<SeatCounters>,
    config: BrainConfig,
    next_episode: u64,
    /// The stored phrasing that produced this turn's reading, if one did.
    ///
    /// Held between hearing and the end of the turn so the pair can be
    /// credited or blamed. The store has kept success and failure counts for
    /// pairs from the beginning and nothing outside the tests ever wrote to
    /// them, so a phrasing that generalized badly kept firing forever and
    /// training could only ever make the ears more confident, never better.
    last_pair: Option<i64>,
    /// Readings learned from turns the model got right.
    ///
    /// Held here rather than inside the ears because it is fed by what the
    /// whole turn concluded, not by what the ears alone produced: a reading is
    /// only worth reusing once the interior acted on it without complaint.
    phrasing: PhrasingIndex,
}

impl Brain {
    /// Build a brain around an already-shared symbol table.
    ///
    /// The table is passed in rather than created here because the mouth and
    /// the teacher need the same one: a name learned this turn has to be
    /// printable this turn.
    pub fn new(
        store: Store,
        registry: NativeRegistry,
        symbols: Arc<SymbolTable>,
        seats: Seats,
        config: BrainConfig,
    ) -> spoon_store::Result<Self> {
        let next_episode = store.next_episode_id()?;
        let phrasing = PhrasingIndex::from_store(&store).unwrap_or_default();
        Ok(Brain {
            store,
            registry,
            symbols,
            ears: seats.ears,
            mouth: seats.mouth,
            teacher: seats.teacher,
            counters: seats.counters,
            config,
            next_episode,
            phrasing,
            last_pair: None,
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn symbols(&self) -> &Arc<SymbolTable> {
        &self.symbols
    }

    pub fn counters(&self) -> &Arc<SeatCounters> {
        &self.counters
    }

    /// One turn.
    pub async fn turn(&mut self, session: &str, text: &str) -> spoon_store::Result<TurnResult> {
        self.turn_with_events(session, text, None).await
    }

    pub async fn turn_with_events(
        &mut self,
        session: &str,
        text: &str,
        sink: Option<crate::event::EventSink>,
    ) -> spoon_store::Result<TurnResult> {
        use crate::event::{TurnEvent, emit};
        let sink = sink.as_ref();
        emit(sink, TurnEvent::TurnStarted { session: session.into(), text: text.into() });
        let started = Instant::now();
        let mut metrics = TurnMetrics::default();
        let previous = self.feedback_target(session, text)?;
        let retry_feedback = previous.is_some() && feedback::retries_request(text);
        if let Some(previous) = &previous { self.reject_episode(previous, text)?; }
        let heard_text = if retry_feedback {
            let e = previous.as_ref().unwrap();
            e.request_text.as_deref().unwrap_or(&e.user_text).to_string()
        } else { crate::correct::repaired(text).unwrap_or_else(|| text.into()) };
        let feedback_context = previous.as_ref().map(|e| format!(
            "The user rejected the previous answer. Request: {}\nReading: {}\nAnswer: {}\nUser feedback: {}\nReconsider the reading or teach a corrected capability using this feedback.",
            e.request_text.as_deref().unwrap_or(&e.user_text),
            e.steps.iter().map(|s| render(s, &self.symbols)).collect::<Vec<_>>().join("; "),
            e.result.as_ref().map(|c| render(c, &self.symbols)).unwrap_or_else(|| "no answer".into()), text,
        ));

        emit(sink, TurnEvent::EarsStarted);
        let ears_started = Instant::now();
        self.last_pair = None;
        let (mut heard, ears_path) = if retry_feedback {
            let e = previous.as_ref().unwrap();
            // Do not ask the ears to interpret "that's wrong" as a new task.
            (Heard::native(e.steps.clone(), 0.0), e.ears_path)
        } else { self.hear(session, &heard_text, &mut metrics).await };
        for name in &heard.names { self.symbols.intern(name); }
        metrics.millis_ears = ears_started.elapsed().as_millis() as u64;
        emit(sink, TurnEvent::EarsResult {
            path: ears_path,
            steps: heard.steps.iter().map(|s| render(s, &self.symbols)).collect(),
            unknown: heard.unknown.iter().map(|w| w.to_string()).collect(),
        });
        let reconciled = crate::reconcile::reconcile(&heard.steps, &self.store, &self.symbols);
        crate::reconcile::remember(&reconciled, &self.store);
        let mut steps = reconciled.steps;
        let mut moves = resolve(&steps, crate::resolve::is_question(&heard_text));
        let mut pair = self.last_pair.take();
        emit(sink, TurnEvent::InteriorStarted { goal: steps.first().map(|s| render(s, &self.symbols)) });
        // Rejection is applied before any re-execution. A side effect from the
        // rejected turn must never be repeated merely to obtain another answer.
        let mut attempt = if retry_feedback { Attempt::default() } else { self.execute(&moves, &mut metrics)? };
        emit(sink, TurnEvent::InteriorResult { result: attempt.result.as_ref().map(|r| render(r, &self.symbols)) });

        let went_wrong = !attempt.gaps.is_empty() || !heard.unknown.is_empty()
            || attempt.result.as_ref().is_none_or(|r| !is_answer(&self.store, r) || is_unknown(r));
        let spot_check = ears_path == EarsPath::Model && self.config.check_clean_readings > 0
            && self.next_episode.is_multiple_of(u64::from(self.config.check_clean_readings));
        let mut learning = Vec::new();
        let mut teacher_exchanges = Vec::new();
        if previous.is_some() { learning.push("recorded negative feedback against the earlier turn".into()); }
        let mut changed_reading = false;
        if self.config.teaching && (went_wrong || spot_check || previous.is_some()) {
            let gaps = if retry_feedback {
                let e = previous.as_ref().unwrap();
                // A capability can run successfully and still be what the user
                // rejected. It is a teaching target even without an exception.
                let mut gaps = e.gaps.clone();
                if gaps.is_empty() && let Some(goal) = &e.goal { gaps.push(goal.clone()); }
                gaps
            } else { attempt.gaps.clone() };
            let taught = self.consult_teacher(&heard_text, &heard, &gaps, &mut metrics,
                feedback_context.as_deref()).await?;
            learning.extend(taught.notes);
            teacher_exchanges.extend(taught.exchanges);
            if let Some((replacement, replacement_pair)) = taught.reading {
                changed_reading = true;
                if let Some(old_pair) = pair {
                    self.store.record_pair_outcome(old_pair, false)?;
                }
                let reconciled = crate::reconcile::reconcile(&replacement, &self.store, &self.symbols);
                crate::reconcile::remember(&reconciled, &self.store);
                steps = reconciled.steps;
                heard.steps = replacement;
                moves = resolve(&steps, crate::resolve::is_question(&heard_text));
                pair = Some(replacement_pair);
            }
            let previous_effect = previous.as_ref().is_some_and(|e| e.trace.iter().any(|s|
                matches!(s.effect.as_str(), "write" | "network" | "shell")));
            let safe_retry = !attempt.external_effects && !(retry_feedback && previous_effect);
            if safe_retry && (changed_reading || (went_wrong && !learning.is_empty())) {
                // Roll back only assertions made by this attempt if its reading
                // was replaced, then use the new moves rather than the old ones.
                if changed_reading {
                    for (id, _) in &attempt.assertions { self.store.retract(spoon_store::AssertionId(*id), Utc::now())?; }
                }
                // An unchanged rejected assertion must not be reasserted.
                if changed_reading || (attempt.assertions.is_empty() && !moves.iter().any(|m| matches!(m, Move::Assert(_)))) {
                    attempt = self.execute(&moves, &mut metrics)?;
                    if retry_feedback && previous.as_ref().is_some_and(|e| e.result == attempt.result) {
                        attempt.result = None;
                        learning.push("the new attempt repeated the rejected answer; no corrected answer yet".into());
                    } else {
                        learning.push("used what it just learned".into());
                    }
                }
            }
        }
        let answered = attempt.result.as_ref().is_some_and(|r| is_answer(&self.store, r) && !is_unknown(r));
        if let Some(id) = pair {
            self.store.record_pair_outcome(id, answered)?;
            self.phrasing = PhrasingIndex::from_store(&self.store)?;
        }
        if ears_path == EarsPath::Model && !retry_feedback && !changed_reading
            && attempt.gaps.is_empty() && !steps.is_empty() && answered {
            pair = Some(self.remember_pair(&heard_text, &steps, PairSource::Model)?);
        }
        emit(sink, TurnEvent::MouthStarted);
        let mouth_started = Instant::now();
        let response = if retry_feedback && attempt.result.is_none() {
            // Record the correction without pretending that relearning succeeded.
            Concept::call("answer", [Concept::text("I recorded that answer as wrong. I do not have a corrected answer yet.")])
        } else { self.build_response(&moves, attempt.result.as_ref(), &attempt.gaps, &heard) };
        let must_mention: Vec<_> = attempt.result.iter().cloned().collect();
        let (reply, mouth_path, mouth_exchange) = self.say(&response, &must_mention, &mut metrics).await;
        metrics.millis_mouth = mouth_started.elapsed().as_millis() as u64;
        metrics.millis_total = started.elapsed().as_millis() as u64;
        emit(sink, TurnEvent::MouthResult { path: mouth_path, reply: reply.clone() });
        let episode = Episode {
            id: self.next_episode, at: Utc::now(), session: session.into(), user_text: text.into(),
            request_text: retry_feedback.then_some(heard_text), phrasing: pair,
            assertions: Some(attempt.assertions), correction_of: previous.as_ref().map(|e| e.id),
            steps, ears_path, unknown_words: heard.unknown.iter().map(|w| w.to_string()).collect(),
            goal: attempt.goal, result: attempt.result, gaps: attempt.gaps,
            realizations: attempt.realizations, trace: attempt.trace, rules: attempt.rules,
            learning, reply: reply.clone(), mouth_path, metrics, correction: None,
            ears_exchange: heard.exchange, teacher_exchanges, mouth_exchange,
        };
        self.store.put_episode(&episode_json(&episode)?, episode.id)?;
        self.next_episode += 1;
        emit(sink, TurnEvent::TurnFinished { episode_id: episode.id, millis_total: episode.metrics.millis_total });
        Ok(TurnResult { reply, episode })
    }

    fn remember_names(&self, concept: &Concept) {
        for node in spoon_concept::pre_order(concept) {
            if let Some(id) = node.as_symbol()
                && let Some(name) = self.symbols.resolve(id)
                && self.store.symbol_name(id).ok().flatten().is_none()
            {
                let _ = self.store.register_symbol(&name);
            }
        }
    }

    /// Is this something the store now actively asserts?
    fn holds(&self, concept: &Concept) -> bool {
        self.store.holds(concept).unwrap_or(false)
    }

    fn build_response(
        &self,
        moves: &[Move],
        result: Option<&Concept>,
        gaps: &[Concept],
        heard: &Heard,
    ) -> Concept {
        if heard.is_empty() {
            return Concept::call("did-not-understand", []);
        }
        match (moves.first(), result) {
            (Some(Move::Chat(c)), _) => c.clone(),
            (Some(Move::Assert(c)), _) => Concept::call("noted", [c.clone()]),
            // A statement that arrived as a `Do` was still a statement.
            (Some(Move::Do(_)), Some(value)) if gaps.is_empty() && self.holds(value) => {
                Concept::call("noted", [value.clone()])
            }
            // Nothing reduced and something was missing, so the "result" is
            // just the request read back. Showing it is worse than useless:
            // the reader gets internal notation instead of an answer and no
            // indication that anything went wrong.
            (Some(m), Some(value)) if !gaps.is_empty() && value == m.concept() => {
                Concept::call("cannot-yet", [value.clone()])
            }
            (_, Some(value)) if !gaps.is_empty() => {
                Concept::call("partial", [value.clone(), Concept::int(gaps.len() as i64)])
            }
            (_, Some(value)) => Concept::call("answer", [value.clone()]),
            (_, None) => Concept::call("nothing-to-say", []),
        }
    }

    async fn say(
        &self,
        response: &Concept,
        must_mention: &[Concept],
        metrics: &mut TurnMetrics,
    ) -> (String, MouthPath, Option<spoon_seat::Exchange>) {
        match self.mouth.say(response, must_mention).await {
            Ok(mr) => {
                if mr.exchange.is_some() {
                    metrics.mouth_model += 1;
                    (mr.text, MouthPath::Model, mr.exchange)
                } else {
                    metrics.mouth_template += 1;
                    (mr.text, MouthPath::Template, None)
                }
            }
            Err(_) => {
                metrics.mouth_template += 1;
                (
                    self.mouth.say_native(response, must_mention),
                    MouthPath::Template,
                    None,
                )
            }
        }
    }

    /// Render a concept using the names this brain knows.
    pub fn render(&self, concept: &Concept) -> String {
        render(concept, &self.symbols)
    }

    pub fn seat_calls(&self, seat: Seat) -> u64 {
        self.counters.get(seat)
    }
}

/// The concept a Teacher question was about, when it had one.
fn subject_of(ask: &TeacherAsk) -> Option<Concept> {
    match ask {
        TeacherAsk::Capability { concept, .. } | TeacherAsk::Examples { concept, .. } => {
            // The head, not the whole call: the gap was `reverse<"hello">` and
            // the capability being taught is `reverse`.
            concept.head().cloned().or_else(|| Some(concept.clone()))
        }
        // A reading is about the whole utterance, not about one concept.
        TeacherAsk::Vocabulary { .. } | TeacherAsk::Concept { .. } | TeacherAsk::Reading { .. } => {
            None
        }
    }
}

/// Prefer the concept that actually failed over the name the Teacher chose.
///
/// Its own name is kept only when there is nothing to bind to, which happens
/// for vocabulary questions where the Teacher is naming something genuinely
/// new rather than explaining something Spoon already tried and could not do.
fn retarget(proposed: Concept, subject: Option<&Concept>) -> Concept {
    match subject {
        Some(actual) if actual.is_named() => actual.clone(),
        _ => proposed,
    }
}

/// Is this concept a statement rather than a request?
///
/// Only the declarative meta-vocabulary counts. An irreducible concept is
/// ambiguous on its face: `Symmetric<Friends>` is something Spoon was told,
/// while `FindIndicesSummingTo<[2,7], 9>` is something Spoon was asked for and
/// cannot do. Both reduce to themselves.
///
/// Guessing "fact" for the second is the expensive mistake: it stores the
/// request as though it were true, reports no capability gap, and so the
/// Teacher is never asked and the capability is never learned. Guessing
/// "request" for the first only means a fact goes unstored and the user says it
/// again.
fn is_declarative(concept: &Concept) -> bool {
    const DECLARATIVE: &[&str] = &[
        "symmetric",
        "transitive",
        "inverse-of",
        "subtype-of",
        "participates",
        "synonym",
        "default-expectation",
        "denotes",
        "works-well-with",
        "works-poorly-with",
    ];
    concept.head_symbol().is_some_and(|head| {
        DECLARATIVE
            .iter()
            .any(|d| head == spoon_concept::SymbolId::of(d))
    })
}

/// A concept as the ears should see it: what it is called, what shape it takes,
/// and what it does.
#[derive(Debug, Clone)]
pub struct Described {
    pub name: Arc<str>,
    pub arity: Option<String>,
    pub doc: Option<String>,
}

impl Described {
    /// One line for a prompt. Compact on purpose: this appears a hundred times
    /// over, and a paragraph each would crowd out the utterance being read.
    pub fn line(&self) -> String {
        match (&self.arity, &self.doc) {
            (Some(a), Some(d)) => format!("{} ({a}) - {d}", self.name),
            (Some(a), None) => format!("{} ({a})", self.name),
            (None, Some(d)) => format!("{} - {d}", self.name),
            (None, None) => self.name.to_string(),
        }
    }
}

/// Words in an utterance that look like names.
///
/// A phrasing match can produce a concept for something the store has never
/// heard of, and a symbol id is derived from its name, so without recording the
/// spelling the reply prints hex where it should print "mary".
fn name_shaped_words(text: &str) -> Vec<Arc<str>> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| w.len() > 1 && w.chars().all(|c| c.is_alphanumeric()))
        .map(|w| Arc::from(w.to_lowercase().as_str()))
        .collect()
}

fn episode_json(episode: &Episode) -> spoon_store::Result<String> {
    serde_json::to_string(episode).map_err(spoon_store::StoreError::from)
}

/// Whether an evaluated value is an answer or a term that stalled.
///
/// "Did anything change" is too weak a test on its own. Asked how many r's
/// are in a string, the ears composed `count-matching<chars<"...">, "r">`
/// against a head nothing realizes. `chars` reduced, so the term changed,
/// so the old test passed it through and reported
/// `count-matching<list<"f", "o", ...>, "r">` as the answer. Half a
/// reduction is not a result, and reporting one means the gap is never
/// seen and the capability is never learned.
///
/// An unrealized compound is data when the store asserts it and a stall
/// when it does not. `friend-with<greg, keal>` is a fact and stays an
/// answer; a head applied to arguments that nothing can carry out and
/// nobody ever claimed is a gap.
/// Whether a value is Spoon reporting that it could not answer.
///
/// `unknown<...>` is a real result in the sense that the turn produced it, and
/// not one in the sense the user cares about.
/// Whether free text names any of these concepts.
///
/// Word boundaries matter: `count` must not match inside `count-matching`, or
/// retiring one name would silently mute advice about the other.
fn mentions_any(text: &str, names: &[String]) -> bool {
    let lower = text.to_lowercase();
    let boundary = |c: char| !c.is_alphanumeric() && c != '-';
    names.iter().any(|name| {
        lower.match_indices(name.as_str()).any(|(at, _)| {
            let before = lower[..at].chars().next_back().is_none_or(boundary);
            let after = lower[at + name.len()..].chars().next().is_none_or(boundary);
            before && after
        })
    })
}

/// Is this reading nothing but small talk?
fn only_pleasantries(steps: &[Concept]) -> bool {
    !steps.is_empty()
        && steps.iter().all(|s| {
            s.head_symbol()
                .is_some_and(|h| h == spoon_concept::SymbolId::of("chat"))
        })
}

pub fn is_unknown(value: &Concept) -> bool {
    value
        .head_symbol()
        .is_some_and(|h| h == spoon_concept::SymbolId::of("unknown"))
}

pub fn is_answer(store: &Store, value: &Concept) -> bool {
    // A hole is an unfilled blank. `make keyboard uppercase` came back as
    // `map<?0, upper<?0>>`, which is a function, not an answer: nothing was
    // ever substituted into it.
    if !spoon_concept::holes(value).is_empty() {
        return false;
    }
    spoon_concept::pre_order(value).all(|node| {
        let Concept::Compound { head, .. } = node else {
            return true;
        };
        if head.as_symbol().is_none() {
            return true;
        }
        let realized = store.realizations_for(head).is_ok_and(|r| !r.is_empty());
        realized || store.holds(node).unwrap_or(false)
    })
}
