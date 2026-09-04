//! The orchestrator.

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
            vocabulary_size: 120,
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
        let started = Instant::now();
        let mut metrics = TurnMetrics::default();

        // A repair marker means the previous turn was wrong, so undo it before
        // acting on what follows. Doing it after would leave the bad claim
        // standing while the replacement is stored beside it, and Spoon would
        // then believe both.
        let mut correction = None;
        if crate::correct::is_correction(text)
            && let Some(previous) = self.last_asserting_episode(session)?
        {
            let applied = crate::correct::apply(&self.store, &previous, Utc::now())?;
            if !applied.is_empty() {
                self.store.mark_episode_corrected(previous.id, text)?;
            }
            correction = Some(applied);
        }

        // ---- ears ----
        let ears_started = Instant::now();
        let (heard, ears_path) = self.hear(session, text, &mut metrics).await;
        // Record spellings in the session table so everything downstream can
        // print words instead of hex. They are deliberately NOT persisted here:
        // the store's symbol set is what reconciliation treats as established
        // concepts, and writing every name the ears just invented into it would
        // make each one look already known and defeat the reconciliation that
        // runs on the next line. A name earns a store row by being used in
        // something stored, which `remember_names` does.
        for name in &heard.names {
            self.symbols.intern(name);
        }
        metrics.millis_ears = ears_started.elapsed().as_millis() as u64;

        // ---- reconcile names before acting on them ----
        // The model names things freshly each time it speaks, so a reading can
        // easily refer to something Spoon already knows under a different
        // spelling. Resolving that here keeps one idea as one concept.
        let reconciled = crate::reconcile::reconcile(&heard.steps, &self.store, &self.symbols);
        crate::reconcile::remember(&reconciled, &self.store);
        let steps = reconciled.steps.clone();

        // ---- interior ----
        let interior_started = Instant::now();
        let moves = resolve(&steps, crate::resolve::is_question(text));
        let mut gaps = Vec::new();
        let mut realizations = Vec::new();
        let mut interior = Vec::new();
        let mut rules = Vec::new();
        let mut result = None;
        let mut goal = None;

        for m in &moves {
            goal = Some(m.concept().clone());
            match self.act(
                m,
                &mut metrics,
                &mut gaps,
                &mut realizations,
                &mut interior,
                &mut rules,
            ) {
                Ok(Some(value)) => result = Some(value),
                Ok(None) => {}
                Err(err) => {
                    result = Some(Concept::call("error", [Concept::text(err.to_string())]));
                }
            }
        }
        metrics.millis_interior = interior_started.elapsed().as_millis() as u64;

        // ---- teacher, only for what the interior could not do ----
        // Checked on every turn the model read, not only on ones that visibly
        // went wrong.
        //
        // A wrong reading that happens to evaluate is the worst case, because
        // it produces a confident wrong answer and no signal at all: asked how
        // many r's are in Strawberry, the ears composed
        // `count-matching<text-contains<"Strawberry", "r">>`, which asks the
        // store how many concepts match `true`, answers 0, and reports no gap.
        // Waiting for trouble means never catching it.
        //
        // The cost amortizes to nothing. Checking a reading is the cheapest
        // thing the Teacher does and the only answer that compounds, since a
        // correction is kept as a phrasing and the native path takes that shape
        // for free from then on.
        // Something visibly went wrong, so there is a specific thing to ask
        // about. These are always worth the Teacher's time.
        let went_wrong = !gaps.is_empty()
            || !heard.unknown.is_empty()
            || result
                .as_ref()
                .is_none_or(|r| !is_answer(&self.store, r) || is_unknown(r));
        // Nothing went wrong, which is exactly when a wrong reading hides.
        // Sampled rather than skipped.
        let spot_check = ears_path == EarsPath::Model
            && self.config.check_clean_readings > 0
            && self
                .next_episode
                .is_multiple_of(u64::from(self.config.check_clean_readings));
        let mut learning = Vec::new();
        if self.config.teaching && (went_wrong || spot_check) {
            learning = self
                .consult_teacher(text, &heard, &gaps, &mut metrics)
                .await;

            // Use what was just learned, on this turn.
            //
            // Teaching ran after the interior and nothing re-ran it, so a turn
            // that synthesized exactly the capability it needed still answered
            // "unknown" and made the user ask twice. Learning a thing and then
            // not using it is the most annoying possible way to succeed.
            //
            // Only when the first pass came up short, and only once. A turn
            // that already had an answer has nothing to redo, and a second
            // consultation could not use its own results either, so it would
            // just be a slower way to reach the same place.
            let fell_short = result
                .as_ref()
                .is_none_or(|r| !is_answer(&self.store, r) || is_unknown(r));
            if fell_short && !learning.is_empty() {
                let retry_started = Instant::now();
                let mut retried = None;
                for m in &moves {
                    if let Ok(Some(value)) = self.act(
                        m,
                        &mut metrics,
                        &mut gaps,
                        &mut realizations,
                        &mut interior,
                        &mut rules,
                    ) {
                        retried = Some(value);
                    }
                }
                if let Some(value) = retried
                    && is_answer(&self.store, &value)
                    && !is_unknown(&value)
                {
                    result = Some(value);
                    learning.push("used what it just learned".to_string());
                }
                metrics.millis_interior += retry_started.elapsed().as_millis() as u64;
            }
        }

        // ---- credit the phrasing that produced the reading ----
        // The one thing that lets training make the ears better rather than
        // only more confident. A pair that reads an utterance into something
        // that does not answer it is wrong, and saying so pulls its standing
        // down until it stops being chosen. Without this the standing computed
        // when the index is built can only go up.
        //
        // Judged on whether the turn produced an answer, not on whether the
        // answer was right, because Spoon has no way to know the latter. That
        // still catches the failure that matters: `make REALIZATION lowercase`
        // matched a phrasing learned from a different sentence and came back
        // unknown, where the model would have got it.
        if let Some(pair) = self.last_pair.take() {
            let worked = result
                .as_ref()
                .is_some_and(|r| is_answer(&self.store, r) && !is_unknown(r));
            let _ = self.store.record_pair_outcome(pair, worked);
            self.phrasing.record(pair, worked);
        }

        // ---- mouth ----
        let mouth_started = Instant::now();
        let response = self.build_response(&moves, result.as_ref(), &gaps, &heard);
        let must_mention: Vec<Concept> = result.iter().cloned().collect();
        let (reply, mouth_path) = self.say(&response, &must_mention, &mut metrics).await;
        metrics.millis_mouth = mouth_started.elapsed().as_millis() as u64;
        metrics.millis_total = started.elapsed().as_millis() as u64;

        let episode = Episode {
            id: self.next_episode,
            at: Utc::now(),
            session: session.to_string(),
            user_text: text.to_string(),
            steps: steps.clone(),
            ears_path,
            unknown_words: heard.unknown.iter().map(|w| w.to_string()).collect(),
            goal,
            result,
            gaps,
            realizations,
            trace: interior,
            rules,
            learning,
            reply: reply.clone(),
            mouth_path,
            metrics,
            correction: correction.as_ref().map(|_| text.to_string()),
        };
        // A reading the model produced that then answered the question is
        // worth remembering, so the same shape costs nothing next time. This
        // is the only part of the ears that improves with use.
        //
        // Readings that hit a gap are deliberately not learned: reusing a bad
        // one makes it permanent, because the model that would have got it
        // right is never consulted again for that shape.
        //
        // Nor is "it did not get stuck" enough on its own. An assertion always
        // succeeds, whatever it asserts, so a turn that stored something is no
        // evidence the sentence was read correctly. "frank is friends with
        // carol" was read as `participates<frank, ...>`, asserted happily, and
        // learned as a phrasing; every later question about friendship then
        // matched that phrasing and came back unknown. Requiring an answer
        // means the reading has been used for something that could fail.
        //
        // Statements still get phrasings, through the Teacher-confirmed path,
        // which is the better evidence anyway.
        let answered = episode
            .result
            .as_ref()
            .is_some_and(|r| is_answer(&self.store, r) && !is_unknown(r));
        if ears_path == EarsPath::Model && episode.gaps.is_empty() && !steps.is_empty() && answered
        {
            let _ = self.store.put_pair(text, &steps, PairSource::Model);
            self.phrasing.learn(text, &steps);
        }

        self.store
            .put_episode(&episode_json(&episode)?, episode.id)?;
        self.next_episode += 1;

        Ok(TurnResult { reply, episode })
    }

    /// The most recent turn that actually claimed something.
    ///
    /// Not simply the previous turn. "no wait, mary has the dog" repairs the
    /// last thing Spoon was told, and questions asked in between do not reset
    /// that: a user who states a fact, asks about it, then corrects themselves
    /// means the fact, not the question. Looking only one turn back would find
    /// the query, have nothing to withdraw, and quietly leave the wrong claim
    /// standing.
    fn last_asserting_episode(&self, session: &str) -> spoon_store::Result<Option<Episode>> {
        const LOOKBACK: usize = 12;
        let raw = self.store.recent_episodes(LOOKBACK, Some(session))?;
        Ok(raw
            .iter()
            .filter_map(|json| serde_json::from_str::<Episode>(json).ok())
            .find(|e| e.correction.is_none() && e.reply.starts_with("noted")))
    }

    async fn hear(
        &mut self,
        session: &str,
        text: &str,
        metrics: &mut TurnMetrics,
    ) -> (Heard, EarsPath) {
        // Native first, always. Every turn the model does not handle is the
        // weaning curve moving.
        self.last_pair = None;
        if let Some(heard) = self.ears.hear_native(text) {
            metrics.ears_native += 1;
            return (heard, EarsPath::Native);
        }
        // Then whatever this user has said before. A phrasing learned from an
        // earlier turn costs nothing and is the only part of the ears that gets
        // better with use.
        if let Some((steps, confidence, pair)) = self.phrasing.recognize_from(text) {
            metrics.ears_native += 1;
            let mut heard = Heard::native(steps, confidence);
            // A recognized reading can mint names the store has never seen, and
            // an id whose spelling was never recorded prints as hex.
            heard.names = name_shaped_words(text);
            self.last_pair = pair;
            return (heard, EarsPath::Native);
        }
        let vocabulary: Vec<String> = self
            .described_vocabulary()
            .iter()
            .map(Described::line)
            .collect();
        let recent = self.recent_turns(session);
        let rules = self.ears_rules();
        match self.ears.hear(text, &vocabulary, &recent, &rules).await {
            Ok(heard) if only_pleasantries(&heard.steps) && !NativeEars::is_social_only(text) => {
                // The model saw a greeting and dropped the request. "hey
                // reverse spoon lol" is a request with a polite opener, and
                // replying hello to it is worse than saying it was not
                // understood: a greeting looks like success, so nothing is
                // recorded, the Teacher is never asked, and the same sentence
                // fails the same way forever.
                //
                // Whether the sentence is only pleasantries is something the
                // native path already decides without a model, so there is
                // something solid to check the model against.
                metrics.ears_failed += 1;
                (Heard::native(Vec::new(), 0.0), EarsPath::Failed)
            }
            Ok(heard) => {
                metrics.ears_model += 1;
                (heard, EarsPath::Model)
            }
            Err(_) => {
                metrics.ears_failed += 1;
                (Heard::native(Vec::new(), 0.0), EarsPath::Failed)
            }
        }
    }

    /// Rules the Teacher has written about how to read things.
    ///
    /// Ordinary stored concepts, so they survive a restart, appear in the
    /// inspector, and can be retracted like anything else that turns out to be
    /// wrong. Capped because the prompt is working memory: past a point another
    /// rule costs more attention than it buys.
    fn ears_rules(&self) -> Vec<String> {
        const MAX_RULES: usize = 12;
        let retired = self.retired_names();
        self.store
            .concepts_by_head(spoon_concept::SymbolId::of("ears-rule"), 64)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| self.store.holds(c).unwrap_or(false))
            .filter_map(|c| {
                c.arg(0)
                    .and_then(Concept::as_ground)
                    .and_then(|g| g.as_str().map(str::to_string))
            })
            // Advice about a capability that no longer exists is worse than no
            // advice: it reads with all the authority of something Spoon
            // learned the hard way, and it steers the ears straight at a head
            // nothing can carry out. Dropping it at read time means rules
            // written before a rename die on their own, with no migration.
            .filter(|rule| !mentions_any(rule, &retired))
            .take(MAX_RULES)
            .collect()
    }

    /// Names that used to mean something and no longer do.
    fn retired_names(&self) -> Vec<String> {
        self.store
            .concepts_by_head(spoon_concept::SymbolId::of("retired"), 128)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| self.store.holds(c).unwrap_or(false))
            .filter_map(|c| c.arg(0)?.as_symbol())
            .filter_map(|id| self.symbols.resolve(id).map(|n| n.to_lowercase()))
            .collect()
    }

    /// The last few turns of this session, oldest first.
    ///
    /// Half of ordinary speech refers backwards. "that is called a palindrome"
    /// is not interpretable on its own, and reading each utterance in isolation
    /// is why it came back as a synonym pointing at an unbound hole: the ears
    /// correctly knew something was missing and had no way to find it.
    fn recent_turns(&self, session: &str) -> Vec<Turn> {
        const WINDOW: usize = 4;
        let mut turns: Vec<Turn> = self
            .store
            .recent_episodes(WINDOW, Some(session))
            .unwrap_or_default()
            .iter()
            .filter_map(|json| serde_json::from_str::<Episode>(json).ok())
            .map(|e| Turn {
                said: Arc::from(e.user_text.as_str()),
                // What it was read as, not what was said back. The ears are
                // being reminded of their own prior output so a reference can
                // resolve to a concept rather than to prose.
                understood: Arc::from(
                    e.steps
                        .iter()
                        .map(|s| render(s, &self.symbols))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .as_str(),
                ),
            })
            .collect();
        turns.reverse();
        turns
    }

    /// Concept names worth showing the ears, most useful first.
    ///
    /// Ranked by activation, so a brain used for one domain surfaces that
    /// domain's vocabulary and a rarely used concept stops costing prompt
    /// space without being forgotten.
    fn vocabulary(&self) -> Vec<Arc<str>> {
        // Retired names keep their meta, which is what makes them still look
        // like vocabulary. Offering one to the ears or the Teacher is how a
        // dead capability gets written back into a durable rule two turns
        // after it was retired.
        let retired = self.retired_names();
        let live: Vec<Arc<str>> = self
            .store
            // Ask for more than will be shown, because the ranking that
            // matters here is not the one activation produces.
            .ranked_surface_forms(self.config.vocabulary_size * 4, Utc::now())
            .unwrap_or_default()
            .into_iter()
            .filter(|name| !retired.iter().any(|r| r == &name.to_lowercase()))
            .collect();

        // Capabilities first, entities with whatever room is left.
        //
        // The ears prompt is working memory and it fills up. After a training
        // run that mentioned a few dozen people, those names outranked the
        // verbs on recency and pushed them out, and the model started failing
        // sentences it had been reading correctly an hour earlier: "make
        // REALIZATION lowercase" came back as a shrug once `lower` was no
        // longer in front of it.
        //
        // The asymmetry is real rather than a heuristic. The ears' job is to
        // choose an operation, and the operations are a small closed set that
        // has to be visible. Entities arrive in the sentence itself and can be
        // read straight off it, so a name absent from the prompt costs much
        // less than a verb absent from it.
        // One query, not one per name. Asking the store about each candidate
        // separately meant several hundred round trips per turn and took the
        // bench from three seconds a case to forty.
        let realized: std::collections::HashSet<_> = self
            .store
            .all_realizations()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| !matches!(r.spec, spoon_concept::RealizationSpec::Rule { .. }))
            .map(|r| r.target.content_id())
            .collect();
        let (capabilities, entities): (Vec<_>, Vec<_>) = live
            .into_iter()
            .partition(|name| realized.contains(&Concept::named(name).content_id()));
        capabilities
            .into_iter()
            .chain(entities)
            .take(self.config.vocabulary_size)
            .collect()
    }

    /// What already realizes this concept, and how each one just fared.
    ///
    /// Answering a gap well needs to know the concept is not missing, only
    /// incomplete. Sent as plain sentences because the Teacher is a language
    /// model and this is the context a person would give.
    fn existing_forms(&self, gap: &Concept) -> Vec<Arc<str>> {
        let Some(head) = gap.head() else {
            return Vec::new();
        };
        let mut out: Vec<Arc<str>> = Vec::new();

        for realization in self.store.realizations_for(head).unwrap_or_default() {
            let shape = match &realization.spec {
                spoon_concept::RealizationSpec::Native { native } => self
                    .registry
                    .get(native)
                    .map(|e| format!("{} ({})", e.doc, e.arity.describe()))
                    .unwrap_or_else(|| "a native".to_string()),
                spoon_concept::RealizationSpec::Composed { body } => {
                    format!("built as {}", render(body, &self.symbols))
                }
                other => format!("a {} realization", other.kind().as_str()),
            };
            out.push(Arc::from(
                format!("{} already exists: {shape}", realization.name).as_str(),
            ));
        }

        // The arguments it was actually given, which is what the existing forms
        // could not handle and what a new one has to.
        let given: Vec<String> = gap
            .args()
            .iter()
            .map(|a| render(a, &self.symbols))
            .collect();
        if !given.is_empty() {
            out.push(Arc::from(
                format!(
                    "it was called with {}, which none of those accept",
                    given.join(", ")
                )
                .as_str(),
            ));
        }
        out
    }

    /// The same vocabulary, with each concept's shape and what it does.
    ///
    /// A bare list of names makes the ears guess at how a concept is called,
    /// and every wrong guess turns into a hand-written accommodation somewhere
    /// else: `list<[4, 9, 2, 7]>` where `list<4, 9, 2, 7>` was meant, `reverse`
    /// handed text when it takes a list. Those are one bug wearing different
    /// clothes, and patching each place it surfaces is how a system accumulates
    /// special cases instead of getting better.
    ///
    /// Arity comes from the registry for natives and from the hole count for
    /// learned bodies, since those are the only places that know it.
    fn described_vocabulary(&self) -> Vec<Described> {
        self.vocabulary()
            .into_iter()
            .map(|name| {
                let target = Concept::named(&name);
                let native_arity = self
                    .store
                    .realizations_for(&target)
                    .ok()
                    .into_iter()
                    .flatten()
                    .find_map(|r| match &r.spec {
                        spoon_concept::RealizationSpec::Native { native } => {
                            self.registry.get(native).map(|e| e.arity.describe())
                        }
                        spoon_concept::RealizationSpec::Composed { body } => {
                            Some(format!("exactly {} argument(s)", holes(body).len()))
                        }
                        _ => None,
                    });
                let doc = self
                    .store
                    .get_meta(&target)
                    .ok()
                    .flatten()
                    .and_then(|m| m.note.map(|n| n.to_string()));
                Described {
                    name,
                    arity: native_arity,
                    doc,
                }
            })
            .collect()
    }

    fn act(
        &mut self,
        m: &Move,
        metrics: &mut TurnMetrics,
        gaps: &mut Vec<Concept>,
        realizations: &mut Vec<(String, bool)>,
        interior: &mut Vec<crate::episode::TraceStep>,
        rules: &mut Vec<String>,
    ) -> spoon_store::Result<Option<Concept>> {
        match m {
            Move::Assert(c) => {
                self.store
                    .assert_concept(c, Provenance::User { episode: None }, None, None)?;
                self.remember_names(c);
                Ok(Some(c.clone()))
            }
            Move::Ask(goal) => {
                let index = DiscriminationTree::from_store(&self.store)
                    .map_err(|_| spoon_store::StoreError::HoleNotStorable)?;
                let mut engine =
                    Engine::new(&self.store, &index).with_budget(self.config.derive_budget);
                let derived = engine.derive(goal).unwrap_or_default();
                metrics.derive_steps += engine.steps_used();
                for d in &derived {
                    rules.extend(d.rules_used().iter().map(|r| r.to_string()));
                }
                // A question can be answered two ways, and derivation is only
                // one of them. "Who owns a dog" is recalled; "what is the
                // smallest of these" is worked out. Routing every question to
                // the fact store means anything computable comes back unknown
                // while the answer was one evaluation away.
                // Holes are not a reason to skip this. A hole in a goal can be
                // a question variable, as in `owns<?0, dog>` meaning who, or a
                // lambda parameter, as in
                // `count<filter<chars<"strawberry">, eq<?0, "r">>>` where it
                // stands for each element and is bound before anything is
                // asked. Refusing to evaluate anything containing a hole
                // treated the second as the first and answered "unknown" to a
                // question whose answer was three.
                //
                // Trying and looking at the result tells them apart exactly: a
                // question variable leaves the goal irreducible, and a lambda
                // parameter reduces to an answer.
                if derived.is_empty() {
                    let mut evaluator = Evaluator::new(&self.store, &self.registry)
                        .with_budget(self.config.eval_budget)
                        .with_permission(self.config.permission);
                    if let Outcome::Value(value) = evaluator.evaluate(goal) {
                        // Only when something actually reduced, and only when
                        // what came back is an answer rather than a term that
                        // stalled partway.
                        if value != *goal && is_answer(&self.store, &value) {
                            metrics.eval_nodes += evaluator.trace().nodes_used;
                            let _ = evaluator.commit_evidence();
                            return Ok(Some(value));
                        }
                    }
                }
                if derived.is_empty() {
                    // A question can go unanswered for two very different
                    // reasons: the fact is genuinely not known, or the goal
                    // mentions something Spoon cannot compute. "is science a
                    // palindrome" is the second, and it looked identical to the
                    // first, so the Teacher was never told and the capability
                    // was never learned.
                    //
                    // Probing the goal separates them. Anything inside it that
                    // no realization can reduce is a gap, and a gap is what the
                    // Teacher acts on.
                    let mut probe = Evaluator::new(&self.store, &self.registry)
                        .with_budget(self.config.eval_budget)
                        .with_permission(PermissionMode::AlwaysAsk);
                    probe.evaluate(goal);
                    let trace = probe.trace();
                    for concept in trace.irreducible() {
                        if !gaps.contains(concept) {
                            gaps.push(concept.clone());
                        }
                    }
                    for (concept, _, _) in trace.failures() {
                        if !gaps.contains(concept) {
                            gaps.push(concept.clone());
                        }
                    }
                }
                Ok(Some(match derived.len() {
                    0 => Concept::call("unknown", [goal.clone()]),
                    _ if holes(goal).is_empty() => Concept::bool(true),
                    _ => Concept::call(
                        "list",
                        derived.iter().map(|d| d.goal.clone()).collect::<Vec<_>>(),
                    ),
                }))
            }
            // Chat is not a computation. Reducing `greet<>` would find no
            // realization, record a capability gap, and turn a hello into a
            // report about what Spoon cannot do.
            Move::Chat(expr) => Ok(Some(expr.clone())),
            // A hole-free expression that nothing can reduce is not a
            // computation, it is a claim. `Symmetric<Friends>` has no
            // realization and never will: saying it at Spoon is telling it
            // something, and evaluating it to itself and discarding the result
            // means the turn taught nothing.
            //
            // Holes exclude questions, since `Owns<?0, Dog>` is asking rather
            // than stating, and anything reducible is excluded because `Add<2,
            // 3>` is a computation whose answer is 5, not a fact about addition.
            Move::Do(expr)
                if holes(expr).is_empty()
                    && is_declarative(expr)
                    && self.is_irreducible_data(expr) =>
            {
                self.store
                    .assert_concept(expr, Provenance::User { episode: None }, None, None)?;
                self.remember_names(expr);
                Ok(Some(expr.clone()))
            }
            Move::Do(expr) => {
                let mut evaluator = Evaluator::new(&self.store, &self.registry)
                    .with_budget(self.config.eval_budget)
                    .with_permission(self.config.permission);
                let outcome = evaluator.evaluate(expr);
                let trace = evaluator.trace();
                metrics.eval_nodes += trace.nodes_used;
                for step in &trace.steps {
                    if let Some(name) = &step.realization {
                        realizations.push((
                            name.to_string(),
                            matches!(step.outcome, spoon_eval::StepOutcome::Reduced(_)),
                        ));
                    }
                    interior.push(crate::episode::TraceStep {
                        concept: render(&step.concept, &self.symbols),
                        realization: step.realization.as_ref().map(|r| r.to_string()),
                        alternatives: step
                            .alternatives
                            .iter()
                            .map(|(n, s)| (n.to_string(), *s))
                            .collect(),
                        explored: step.explored,
                        effect: step.effect.as_str().to_string(),
                        depth: step.depth,
                        outcome: match &step.outcome {
                            spoon_eval::StepOutcome::Reduced(c) => {
                                format!("reduced to {}", render(c, &self.symbols))
                            }
                            spoon_eval::StepOutcome::Irreducible => "irreducible".to_string(),
                            spoon_eval::StepOutcome::Failed(why) => format!("failed: {why}"),
                        },
                    });
                }
                // Two different shapes of "I cannot do that", and both are
                // worth telling the Teacher about. Nothing realizes this head
                // at all is the obvious one. A realization existing and failing
                // on these arguments is the other: `reverse` can reverse a list
                // and was handed a string, which is a gap in what Spoon can do
                // even though the concept is not unknown.
                gaps.extend(trace.irreducible().into_iter().cloned());
                // Deduplicated and capped. A realization that recurses fails
                // once per level, and reporting five hundred copies of the same
                // gap tells the Teacher nothing it does not learn from one.
                const MAX_GAPS: usize = 8;
                for (concept, _, _) in trace.failures() {
                    if gaps.len() >= MAX_GAPS {
                        break;
                    }
                    if !gaps.contains(concept) {
                        gaps.push(concept.clone());
                    }
                }
                let _ = evaluator.commit_evidence();
                // A stall is not a result. Handing the unevaluated term back
                // as the answer is how `reverse the word banana` replied
                // `reverse<"banana">`: the gap was recorded correctly and then
                // the stalled term was reported as though it were the answer.
                // Saying so plainly is both more honest and what the mouth
                // already knows how to phrase.
                Ok(match outcome {
                    Outcome::Value(v) => Some(v),
                    Outcome::Stuck { concept, .. } => {
                        Some(Concept::call("cannot-yet", [concept]))
                    }
                    Outcome::Exhausted { concept, .. } => {
                        Some(Concept::call("cannot-yet", [concept]))
                    }
                    Outcome::NeedsPermission {
                        concept, effect, ..
                    } => Some(Concept::call(
                        "needs-permission",
                        [concept, Concept::text(effect.as_str())],
                    )),
                    Outcome::Failed(err) => {
                        Some(Concept::call("error", [Concept::text(err.to_string())]))
                    }
                })
            }
        }
    }

    /// Does this reduce to itself with nothing applied?
    ///
    /// Checked by evaluating rather than by inspecting the store, because
    /// "irreducible" means no realization fired anywhere in the term, which is
    /// exactly what the evaluator already reports. Pure only: a probe must not
    /// have side effects, and anything needing authority is a computation by
    /// definition.
    fn is_irreducible_data(&self, expr: &Concept) -> bool {
        let mut probe = Evaluator::new(&self.store, &self.registry)
            .with_budget(self.config.eval_budget)
            .with_permission(PermissionMode::AlwaysAsk);
        match probe.evaluate(expr) {
            Outcome::Value(v) => {
                v == *expr && probe.trace().steps.iter().all(|s| s.realization.is_none())
            }
            _ => false,
        }
    }

    /// Persist the spelling of any name in a concept that the session knows but
    /// the store does not yet.
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

    async fn consult_teacher(
        &mut self,
        text: &str,
        heard: &Heard,
        gaps: &[Concept],
        metrics: &mut TurnMetrics,
    ) -> Vec<String> {
        let mut learning = Vec::new();
        let Some(teacher) = &self.teacher else {
            return learning;
        };
        // The same ranked vocabulary the ears get. A Teacher that does not know
        // what Spoon already has will refuse work it could have done: asked to
        // build string reversal without being told `chars` exists, it correctly
        // reports that nothing turns a string into a list.
        let vocabulary = self.vocabulary();
        let mut asks = Vec::new();
        for word in &heard.unknown {
            asks.push(TeacherAsk::Vocabulary {
                word: word.clone(),
                utterance: Arc::from(text),
                position: Arc::from("unknown"),
            });
        }
        // Ask about the reading first. When a turn goes wrong the cause is
        // often how it was heard rather than a capability that is missing, and
        // fixing a capability to serve a misreading builds the wrong thing
        // carefully. It is also the only answer that compounds: a corrected
        // reading becomes a phrasing the native path reuses for free.
        if !heard.steps.is_empty() {
            asks.insert(
                0,
                TeacherAsk::Reading {
                    utterance: Arc::from(text),
                    heard: Arc::from(
                        heard
                            .steps
                            .iter()
                            .map(|s| render(s, &self.symbols))
                            .collect::<Vec<_>>()
                            .join("\n")
                            .as_str(),
                    ),
                    trouble: Arc::from(
                        if gaps.is_empty() {
                            "nothing could be worked out from it".to_string()
                        } else {
                            format!(
                                "nothing realizes {}",
                                gaps.iter()
                                    .map(|g| render(g, &self.symbols))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        }
                        .as_str(),
                    ),
                },
            );
        }
        for gap in gaps.iter().take(2) {
            asks.push(TeacherAsk::Capability {
                concept: gap.clone(),
                // What already exists for this concept, and how it just failed.
                //
                // A concept that fails is usually not one Spoon cannot do at
                // all: `reverse` reverses lists perfectly well and was handed
                // text. Told only "cannot carry out reverse<text>", the Teacher
                // proposes a replacement. Told that a list form already exists
                // and this input was text, it can propose the form that is
                // missing, which is what having several realizations per
                // concept is for.
                attempted: self.existing_forms(gap),
            });
            // Examples are the only thing the synthesizer can actually search
            // against, so asking for them is what turns a gap into a capability
            // rather than a note about one.
            asks.push(TeacherAsk::Examples {
                concept: gap.clone(),
                arity: gap.arity().max(1),
            });
        }

        // Both questions get asked, and they are not redundant. A composition
        // is the Teacher writing the answer; examples are it saying what the
        // answer must do. Search can VERIFY a body far larger than it can FIND:
        // with a hundred operators an eight-node body is combinatorially out of
        // reach, and `join<reverse<chars<?0>>, "">` is exactly eight. Four
        // hundred thousand candidates over twenty-eight seconds did not reach
        // it; checking the Teacher's guess against three examples takes
        // microseconds.
        //
        // So the composition supplies the candidate and the examples supply the
        // verdict. That keeps the rule that matters, which is that nothing the
        // Teacher says is believed on its word, while dropping the assumption
        // that the synthesizer has to be the one to find it.
        let mut proposal: Option<(Concept, Concept)> = None;
        let mut spec: Option<Spec> = None;

        // Enough for the reading plus a capability and its examples for each
        // gap. Truncating below that would drop the capability questions the
        // moment a reading question is added, which is not a trade: the reading
        // says whether the request was understood and the capability says
        // whether it can be carried out, and a wrong turn can be either.
        const MAX_ASKS: usize = 6;
        for ask in asks.into_iter().take(MAX_ASKS) {
            let Ok(reply) = teacher.teach(&ask, &vocabulary).await else {
                continue;
            };
            metrics.teacher_calls += 1;
            if std::env::var("SPOON_DEBUG").is_ok() {
                eprintln!("[teacher] {ask:?}\n      -> {reply:?}");
            }
            let subject = subject_of(&ask);
            match reply {
                TeacherReply::Composition { target, body } => {
                    proposal = Some((retarget(target, subject.as_ref()), body));
                }
                TeacherReply::Spec(s) => {
                    spec = Some(Spec {
                        target: retarget(s.target.clone(), subject.as_ref()),
                        ..s
                    });
                }
                // Vocabulary answers stand on their own and need no checking.
                // A corrected reading is kept as a phrasing, not just applied
                // here. The same shape will be said again, and the point is to
                // stop paying a model for it.
                TeacherReply::Reading { steps, lesson } => {
                    learning.push("the Teacher corrected how this was read".to_string());
                    let _ = self.store.put_pair(text, &steps, PairSource::Confirmed);
                    self.phrasing.learn(text, &steps);
                    // A rule outlives the sentence that produced it, so it is
                    // stored as an ordinary concept and read back into the ears
                    // prompt. The prompt stops being a fixed string somebody
                    // maintains and becomes something Spoon accumulates from
                    // its own mistakes.
                    if let Some(lesson) = lesson {
                        learning.push(format!("new rule for reading: {lesson}"));
                        let rule = Concept::call("ears-rule", [Concept::text(&*lesson)]);
                        let _ = self.store.assert_concept(
                            &rule,
                            Provenance::Teacher { episode: None },
                            None,
                            None,
                        );
                    }
                }
                other => self.absorb(other, subject),
            }
        }

        match (proposal, spec) {
            (Some((target, body)), Some(spec)) => {
                // Keep both, when both work. Realizations compete, so there is
                // no reason to pick a winner here on a guess about which will
                // turn out better: the taught body is available immediately and
                // can be any size, the searched one is minimal and verified by
                // construction, and which of those matters depends on inputs
                // neither of them has seen yet.
                //
                // Selection scores them on evidence as they get used, which is
                // a better judge than this function could be. Storing one and
                // discarding the other throws away the comparison before it
                // happens.
                let taught = self.verify(&body, &spec);
                if taught {
                    learning.push(format!(
                        "the Teacher wrote {} and it passed its examples",
                        render(&target, &self.symbols)
                    ));
                    self.store_composed(&target, &body);
                } else {
                    learning.push(
                        "the Teacher proposed a body that failed its own examples".to_string(),
                    );
                }
                // Searched anyway. It usually finds nothing for a body this
                // size, and when it does the result is smaller than what the
                // Teacher wrote and worth having beside it.
                if let Some(found) = self.learn_from_spec(&spec) {
                    learning.push(found);
                }
            }
            (None, Some(spec)) => {
                if let Some(found) = self.learn_from_spec(&spec) {
                    learning.push(found);
                }
            }
            // A guess with nothing to check it against is not worth keeping.
            // Storing one unchecked is how a body that called itself got in.
            (Some(_), None) | (None, None) => {}
        }
        learning
    }

    /// Take what the Teacher said and make it part of Spoon.
    ///
    /// Everything lands as an ordinary stored concept, at provisional tier: the
    /// Teacher proposes, experience decides. Nothing it says is trusted enough
    /// to arrive as kernel.
    /// Take what the Teacher said and make it part of Spoon.
    ///
    /// `subject` is the concept the question was about, when there was one. The
    /// Teacher names its own answers, and asked how to reverse a string it will
    /// propose `reverse-text` while the concept that actually failed was
    /// `reverse`. Storing it under the invented name produces a correct
    /// realization that nothing ever calls, because no utterance will ever
    /// mention it. Binding the answer to the concept that failed is what closes
    /// the loop.
    fn absorb(&self, reply: TeacherReply, subject: Option<Concept>) {
        let now = Utc::now();
        match reply {
            TeacherReply::Synonym { word, concept, .. } => {
                let claim = Concept::call("synonym", [Concept::text(&*word), concept]);
                let _ = self.store.assert_concept(
                    &claim,
                    Provenance::Teacher { episode: None },
                    None,
                    None,
                );
            }
            TeacherReply::NewConcept {
                concept,
                relations,
                surface_forms,
            } => {
                let meta = ConceptMeta::new(
                    concept.clone(),
                    Provenance::Teacher { episode: None },
                    Tier::Provisional,
                    now,
                )
                .with_surface_forms(surface_forms.iter().map(|s| s.as_ref()));
                let _ = self.store.put_meta(&meta);
                for relation in relations {
                    let _ = self.store.assert_concept(
                        &relation,
                        Provenance::Teacher { episode: None },
                        None,
                        None,
                    );
                }
            }
            TeacherReply::Composition { target, body } => {
                let target = retarget(target, subject.as_ref());
                // The Teacher proposes; something has to verify. A composition
                // arrives unchecked, and one that calls its own target without
                // a base case recurses until a budget stops it, turning a
                // single request into hundreds of failed steps. The Teacher
                // offered exactly that here: `reverse` defined as
                // `reverse<chars<?0>>`.
                //
                // A body may legitimately use other realizations of its own
                // target, which is how reversing text builds on reversing a
                // list, but only with something between the two calls. A body
                // whose outermost application is its own target has nothing in
                // between and cannot terminate.
                if body.head_symbol() == target.as_symbol() {
                    return;
                }
                let realization = spoon_concept::Realization {
                    target,
                    name: format!("taught-{}", now.timestamp_millis()).into(),
                    spec: spoon_concept::RealizationSpec::Composed { body },
                    effect: spoon_concept::Effect::Pure,
                    activation: spoon_concept::Activation::new(now),
                    provenance: Provenance::Teacher { episode: None },
                    tier: Tier::Provisional,
                };
                let _ = self.store.put_realization(&realization);
            }
            // A spec is examples, and examples are searchable. This is the
            // path that turns "I cannot do that" into something Spoon can do.
            TeacherReply::Spec(spec) => {
                let spec = Spec {
                    target: retarget(spec.target, subject.as_ref()),
                    ..spec
                };
                self.learn_from_spec(&spec);
            }
            // Handled where the reply arrives, since it is kept as a phrasing
            // rather than absorbed as knowledge.
            TeacherReply::Reading { .. } => {}
            // An admitted blank is a real answer. Nothing to store yet, but it
            // is worth not treating as a failure.
            TeacherReply::Unknown { .. } => {}
        }
    }

    /// Does this body actually do what the examples say?
    ///
    /// The same check synthesis applies to every candidate it considers, run
    /// once against the Teacher's guess. Verification is cheap where search is
    /// not, which is what makes accepting a body too large to find safe.
    fn verify(&self, body: &Concept, spec: &Spec) -> bool {
        if spec.examples.len() < 3 {
            return false;
        }
        spec.examples.iter().all(|(inputs, expected)| {
            let term = spoon_concept::substitute_positional(body, inputs);
            let mut evaluator = Evaluator::new(&self.store, &self.registry)
                .with_budget(self.config.eval_budget)
                .with_permission(PermissionMode::AlwaysAsk);
            matches!(evaluator.evaluate(&term), Outcome::Value(v) if v == *expected)
        })
    }

    /// Store a verified body as a realization of the concept that failed.
    fn store_composed(&self, target: &Concept, body: &Concept) {
        let now = Utc::now();
        let _ = self.store.put_realization(&spoon_concept::Realization {
            target: target.clone(),
            name: format!("taught-{}", now.timestamp_millis()).into(),
            spec: spoon_concept::RealizationSpec::Composed { body: body.clone() },
            effect: spoon_concept::Effect::Pure,
            activation: spoon_concept::Activation::new(now),
            provenance: Provenance::Teacher { episode: None },
            tier: Tier::Provisional,
        });
    }

    /// Search for a body satisfying the Teacher's examples, and keep it if one
    /// exists.
    ///
    /// The Teacher proposes; the synthesizer verifies. That separation is why a
    /// model is allowed near this at all: nothing it says is trusted, only its
    /// examples are, and a body that fails one of them is discarded.
    fn learn_from_spec(&self, spec: &Spec) -> Option<String> {
        if spec.examples.is_empty() {
            return None;
        }
        // Larger than the default size cap, because the bodies worth learning
        // from a conversation are a little bigger than the ones worth testing.
        // Reversing text is `join<reverse<chars<?0>>, "">`, which is eight
        // nodes, and a cap of seven puts it permanently out of reach while
        // reporting an honest "searched everything, found nothing". The time
        // budget is what actually bounds the cost.
        let budget = SynthBudget {
            max_size: 9,
            max_millis: 8_000,
            ..SynthBudget::default()
        };
        let outcome = synthesize(spec, &self.store, &self.registry, budget);
        let SynthOutcome::Found { body, size, .. } = outcome else {
            return None;
        };
        let now = Utc::now();
        let realization = spoon_concept::Realization {
            target: spec.target.clone(),
            name: format!("synth-{}", now.timestamp_millis()).into(),
            spec: spoon_concept::RealizationSpec::Composed { body },
            effect: spoon_concept::Effect::Pure,
            activation: spoon_concept::Activation::new(now),
            // Provisional: it fits the examples it was shown, which is evidence
            // rather than proof. Use decides the rest.
            provenance: Provenance::Synthesized { episode: None },
            tier: Tier::Provisional,
        };
        let _ = self.store.put_realization(&realization);
        Some(format!(
            "the synthesizer found {} in {size} nodes, verified on {} examples",
            render(&spec.target, &self.symbols),
            spec.examples.len()
        ))
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
    ) -> (String, MouthPath) {
        match self.mouth.say(response, must_mention).await {
            Ok(text) => {
                metrics.mouth_model += 1;
                (text, MouthPath::Model)
            }
            Err(_) => {
                metrics.mouth_template += 1;
                (
                    self.mouth.say_native(response, must_mention),
                    MouthPath::Template,
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
