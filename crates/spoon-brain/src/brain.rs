//! The orchestrator.

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use spoon_concept::{Concept, ConceptMeta, Provenance, SymbolTable, Tier, holes, render};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_infer::{DeriveBudget, DiscriminationTree, Engine};
use spoon_seat::{Ears, Heard, Mouth, Seat, SeatCounters, Teacher, TeacherAsk, TeacherReply};
use spoon_store::Store;

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
}

impl Default for BrainConfig {
    fn default() -> Self {
        BrainConfig {
            permission: PermissionMode::AskWrites,
            eval_budget: Budget::default(),
            derive_budget: DeriveBudget::default(),
            vocabulary_size: 120,
            teaching: true,
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

        // ---- ears ----
        let ears_started = Instant::now();
        let (heard, ears_path) = self.hear(text, &mut metrics).await;
        // Register spellings first. Everything downstream renders concepts, and
        // an id whose name was never recorded can only print as hex.
        for name in &heard.names {
            self.symbols.intern(name);
            let _ = self.store.register_symbol(name);
        }
        metrics.millis_ears = ears_started.elapsed().as_millis() as u64;

        // ---- interior ----
        let interior_started = Instant::now();
        let moves = resolve(&heard.steps);
        let mut gaps = Vec::new();
        let mut realizations = Vec::new();
        let mut rules = Vec::new();
        let mut result = None;
        let mut goal = None;

        for m in &moves {
            goal = Some(m.concept().clone());
            match self.act(m, &mut metrics, &mut gaps, &mut realizations, &mut rules) {
                Ok(Some(value)) => result = Some(value),
                Ok(None) => {}
                Err(err) => {
                    result = Some(Concept::call("error", [Concept::text(err.to_string())]));
                }
            }
        }
        metrics.millis_interior = interior_started.elapsed().as_millis() as u64;

        // ---- teacher, only for what the interior could not do ----
        if self.config.teaching && (!gaps.is_empty() || !heard.unknown.is_empty()) {
            self.consult_teacher(text, &heard, &gaps, &mut metrics)
                .await;
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
            steps: heard.steps.clone(),
            ears_path,
            unknown_words: heard.unknown.iter().map(|w| w.to_string()).collect(),
            goal,
            result,
            gaps,
            realizations,
            rules,
            reply: reply.clone(),
            mouth_path,
            metrics,
            correction: None,
        };
        self.store
            .put_episode(&episode_json(&episode)?, episode.id)?;
        self.next_episode += 1;

        Ok(TurnResult { reply, episode })
    }

    async fn hear(&self, text: &str, metrics: &mut TurnMetrics) -> (Heard, EarsPath) {
        // Native first, always. Every turn the model does not handle is the
        // weaning curve moving.
        if let Some(heard) = self.ears.hear_native(text) {
            metrics.ears_native += 1;
            return (heard, EarsPath::Native);
        }
        let vocabulary = self.vocabulary();
        match self.ears.hear(text, &vocabulary).await {
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

    /// Concept names worth showing the ears, most useful first.
    ///
    /// Ranked by activation, so a brain used for one domain surfaces that
    /// domain's vocabulary and a rarely used concept stops costing prompt
    /// space without being forgotten.
    fn vocabulary(&self) -> Vec<Arc<str>> {
        self.store
            .ranked_surface_forms(self.config.vocabulary_size, Utc::now())
            .unwrap_or_default()
    }

    fn act(
        &mut self,
        m: &Move,
        metrics: &mut TurnMetrics,
        gaps: &mut Vec<Concept>,
        realizations: &mut Vec<(String, bool)>,
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
                }
                gaps.extend(trace.irreducible().into_iter().cloned());
                let _ = evaluator.commit_evidence();
                Ok(match outcome {
                    Outcome::Value(v) => Some(v),
                    Outcome::Stuck { concept, .. } => Some(concept),
                    Outcome::Exhausted { concept, .. } => Some(concept),
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
        &self,
        text: &str,
        heard: &Heard,
        gaps: &[Concept],
        metrics: &mut TurnMetrics,
    ) {
        let Some(teacher) = &self.teacher else { return };
        let mut asks = Vec::new();
        for word in &heard.unknown {
            asks.push(TeacherAsk::Vocabulary {
                word: word.clone(),
                utterance: Arc::from(text),
                position: Arc::from("unknown"),
            });
        }
        for gap in gaps.iter().take(2) {
            asks.push(TeacherAsk::Capability {
                concept: gap.clone(),
                attempted: Vec::new(),
            });
        }

        for ask in asks.into_iter().take(3) {
            let Ok(reply) = teacher.teach(&ask).await else {
                continue;
            };
            metrics.teacher_calls += 1;
            self.absorb(reply);
        }
    }

    /// Take what the Teacher said and make it part of Spoon.
    ///
    /// Everything lands as an ordinary stored concept, at provisional tier: the
    /// Teacher proposes, experience decides. Nothing it says is trusted enough
    /// to arrive as kernel.
    fn absorb(&self, reply: TeacherReply) {
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
            // A spec is what the synthesizer searches against, and an admitted
            // blank is worth storing so the same question is not asked forever.
            TeacherReply::Spec(_) | TeacherReply::Unknown { .. } => {}
        }
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

fn episode_json(episode: &Episode) -> spoon_store::Result<String> {
    serde_json::to_string(episode).map_err(spoon_store::StoreError::from)
}
