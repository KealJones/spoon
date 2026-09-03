//! The evaluation loop.
//!
//! Lowering, inference, planning, and arithmetic are all this one loop with a
//! different realization kind behind it. There is no separate compiler, no
//! separate inference engine, and no separate interpreter for learned code.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use spoon_concept::{
    Concept, ContentId, Effect, Realization, RealizationSpec, SymbolId, generalizes, substitute,
    substitute_positional,
};
use spoon_store::Store;

use crate::budget::{Budget, BudgetState};
use crate::ctx::Ctx;
use crate::error::{EvalError, EvalResult, Gap, GapReason, Outcome};
use crate::native::{ArgStrategy, NativeRegistry};
use crate::select::{self, FitKind, Rng, Scored};
use crate::trace::{Note, Step, StepOutcome, Trace};

/// How much authority realizations may exercise without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    /// Confirm anything above pure computation.
    AlwaysAsk,
    /// Confirm writes and above. The default: reading is cheap to undo,
    /// writing is not.
    #[default]
    AskWrites,
    /// Run anything without asking.
    Bypass,
}

impl PermissionMode {
    /// Whether this effect may run unattended, needs confirmation, or is out of
    /// the question.
    fn verdict(self, effect: Effect) -> Verdict {
        match (self, effect) {
            (PermissionMode::Bypass, _) => Verdict::Allow,
            (PermissionMode::AlwaysAsk, Effect::Pure) => Verdict::Allow,
            (PermissionMode::AlwaysAsk, _) => Verdict::Confirm,
            (PermissionMode::AskWrites, Effect::Pure | Effect::Read) => Verdict::Allow,
            (PermissionMode::AskWrites, _) => Verdict::Confirm,
        }
    }
}

enum Verdict {
    Allow,
    Confirm,
}

/// An LLM seat, for `Neural` realizations.
///
/// Injected rather than built in. A brain with no seat configured simply has no
/// selectable neural realizations, which is a real and coherent state rather
/// than a failure: the interior never needs a model to make a choice.
pub trait NeuralSeat {
    fn invoke(&self, prompt: &Concept, parse: &Concept, ctx: &mut dyn Ctx) -> EvalResult;
}

/// Anything outside the process: HTTP, a subprocess, a foreign database.
pub trait ExternalRunner {
    fn run(&self, spec: &Concept, args: &[Concept], ctx: &mut dyn Ctx) -> EvalResult;
}

/// Everything an evaluation needs that outlives a single call.
pub struct Evaluator<'a> {
    store: &'a Store,
    registry: &'a NativeRegistry,
    neural: Option<&'a dyn NeuralSeat>,
    external: Option<&'a dyn ExternalRunner>,
    permission: PermissionMode,
    now: DateTime<Utc>,
    situation: Vec<Concept>,
    budget: BudgetState,
    rng: Rng,
    /// Pure results only. Purity is what makes reuse sound: the same input
    /// cannot produce a different answer.
    cache: HashMap<ContentId, Concept>,
    /// Realizations for a head, loaded once per evaluation.
    realizations: HashMap<ContentId, Vec<Arc<Realization>>>,
    /// Contextual evidence per realization name, loaded once per evaluation.
    fit_evidence: HashMap<Arc<str>, Vec<(FitKind, Concept)>>,
    /// Concepts currently being derived, so a rule cannot re-enter its own goal.
    in_progress: HashSet<ContentId>,
    trace: Trace,
    depth: u32,
}

impl<'a> Evaluator<'a> {
    pub fn new(store: &'a Store, registry: &'a NativeRegistry) -> Self {
        let now = Utc::now();
        Evaluator {
            store,
            registry,
            neural: None,
            external: None,
            permission: PermissionMode::default(),
            now,
            situation: Vec::new(),
            budget: BudgetState::new(Budget::default()),
            rng: Rng::from_time(now),
            cache: HashMap::new(),
            realizations: HashMap::new(),
            fit_evidence: HashMap::new(),
            in_progress: HashSet::new(),
            trace: Trace::default(),
            depth: 0,
        }
    }

    pub fn with_budget(mut self, budget: Budget) -> Self {
        if budget.deterministic {
            // A fixed seed keeps a deterministic run reproducible even though
            // exploration is off, so turning exploration back on for a debug
            // run still replays identically.
            self.rng = Rng::new(0x5EED);
        }
        self.budget = BudgetState::new(budget);
        self
    }

    pub fn with_permission(mut self, mode: PermissionMode) -> Self {
        self.permission = mode;
        self
    }

    /// Pin the clock. Tests need reproducible activation numbers.
    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    /// Concepts describing the current situation, matched against stored
    /// `WorksWellWith` evidence when ranking realizations.
    pub fn with_situation(mut self, situation: Vec<Concept>) -> Self {
        self.situation = situation;
        self
    }

    pub fn with_neural(mut self, seat: &'a dyn NeuralSeat) -> Self {
        self.neural = Some(seat);
        self
    }

    pub fn with_external(mut self, runner: &'a dyn ExternalRunner) -> Self {
        self.external = Some(runner);
        self
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn into_trace(self) -> Trace {
        self.trace
    }

    /// Reduce a concept as far as it goes.
    pub fn evaluate(&mut self, concept: &Concept) -> Outcome {
        let result = self.reduce(concept).map(|(c, _)| c);
        self.trace.nodes_used = self.budget.nodes_used();
        self.trace.millis = self.budget.elapsed().as_millis() as u64;
        Outcome::from(result)
    }

    /// Reduce, returning the highest effect exercised along the way.
    ///
    /// The effect travels back up so that caching stays sound: a subtree that
    /// read the store taints everything above it, and a cached read is a stale
    /// read.
    fn reduce(&mut self, concept: &Concept) -> Result<(Concept, Effect), EvalError> {
        if let Some(limit) = self.budget.check_depth(self.depth) {
            return Err(EvalError::Exhausted {
                concept: concept.clone(),
                limit,
            });
        }

        match concept {
            // Atomic concepts and unbound holes are already as reduced as they
            // get. A hole is not an error here: it is a visible gap, which is
            // what lets a caller report a placeholder instead of inventing a
            // value.
            Concept::Atomic(_) | Concept::Hole(_) => Ok((concept.clone(), Effect::Pure)),
            Concept::Compound { .. } => self.reduce_compound(concept),
        }
    }

    fn reduce_compound(&mut self, concept: &Concept) -> Result<(Concept, Effect), EvalError> {
        let id = concept.content_id();
        if let Some(hit) = self.cache.get(&id) {
            return Ok((hit.clone(), Effect::Pure));
        }
        if !self.in_progress.insert(id) {
            return Err(EvalError::Cycle {
                concept: concept.clone(),
            });
        }
        let result = self.reduce_compound_inner(concept);
        self.in_progress.remove(&id);

        if let Ok((value, Effect::Pure)) = &result {
            self.cache.insert(id, value.clone());
        }
        result
    }

    fn reduce_compound_inner(&mut self, concept: &Concept) -> Result<(Concept, Effect), EvalError> {
        let head = concept.head().expect("compound has a head");
        let (head_value, head_effect) = self.descend(head)?;

        let candidates = self.candidates_for(&head_value)?;

        if candidates.is_empty() {
            // Nothing realizes this head, which is the normal state for data.
            // `FriendWith<Greg, Keal>` is a fact, not a computation, and must
            // reduce to itself rather than raise. Arguments are still reduced so
            // `Height<Add<1, 2>>` becomes `Height<3>`, which is strictly more
            // useful to whoever reads it. The step is recorded so Stage 6 can
            // see what Spoon could not do.
            let (args, args_effect) = self.reduce_args(concept.args(), ArgStrategy::Eager)?;
            let rebuilt = Concept::apply(head_value, args);
            self.check_size(&rebuilt)?;
            self.trace.steps.push(Step {
                concept: concept.clone(),
                realization: None,
                alternatives: Vec::new(),
                explored: false,
                effect: Effect::Pure,
                depth: self.depth,
                outcome: StepOutcome::Irreducible,
            });
            return Ok((rebuilt, head_effect.join(args_effect)));
        }

        self.apply_best(concept, head_effect, candidates)
    }

    fn apply_best(
        &mut self,
        concept: &Concept,
        head_effect: Effect,
        candidates: Vec<Arc<Realization>>,
    ) -> Result<(Concept, Effect), EvalError> {
        let ranked = self.rank(candidates);
        if ranked.is_empty() {
            return Err(EvalError::Stuck {
                concept: concept.clone(),
                gaps: vec![Gap {
                    concept: concept.clone(),
                    reason: GapReason::AllExcluded,
                }],
            });
        }

        // Exploration is suppressed above pure effects. Trying an unproven
        // realization to learn from it is reasonable for a computation and
        // irresponsible for something that spends money or deletes data.
        let explorable = !self.budget.is_deterministic()
            && ranked.iter().all(|s| s.realization.effect == Effect::Pure);
        let first = select::choose_first(&ranked, explorable, &mut self.rng);

        let order: Vec<usize> = std::iter::once(first)
            .chain((0..ranked.len()).filter(|i| *i != first))
            .collect();

        let mut last_error = None;
        for (attempt, index) in order.into_iter().enumerate() {
            let scored = &ranked[index];
            let realization = scored.realization.clone();

            if let Some(limit) = self.budget.charge_node() {
                return Err(EvalError::Exhausted {
                    concept: concept.clone(),
                    limit,
                });
            }

            let effect = self.effective_effect(&realization)?;
            match self.permission.verdict(effect) {
                Verdict::Allow => {}
                Verdict::Confirm => {
                    return Err(EvalError::NeedsPermission {
                        concept: concept.clone(),
                        effect,
                        realization: realization.name.clone(),
                    });
                }
            }

            let alternatives: Vec<(Arc<str>, f64)> = ranked
                .iter()
                .filter(|s| s.realization.name != realization.name)
                .map(|s| (s.realization.name.clone(), s.score))
                .collect();

            match self.apply(concept, &realization, effect) {
                Ok((value, applied_effect)) => {
                    self.trace.steps.push(Step {
                        concept: concept.clone(),
                        realization: Some(realization.name.clone()),
                        alternatives,
                        explored: attempt == 0 && index != 0,
                        effect: applied_effect,
                        depth: self.depth,
                        outcome: StepOutcome::Reduced(value.clone()),
                    });
                    self.record_use(&realization, true);
                    return Ok((value, head_effect.join(applied_effect)));
                }
                Err(err) => {
                    self.trace.steps.push(Step {
                        concept: concept.clone(),
                        realization: Some(realization.name.clone()),
                        alternatives,
                        explored: attempt == 0 && index != 0,
                        effect,
                        depth: self.depth,
                        outcome: StepOutcome::Failed(err.to_string()),
                    });
                    self.record_use(&realization, false);
                    if !err.is_retryable() {
                        return Err(err);
                    }
                    last_error = Some(err);
                }
            }
        }

        // Every realization was tried and each one failed. That is a genuine
        // "I should have been able to do this", unlike having none at all.
        Err(match last_error {
            Some(EvalError::Stuck { gaps, .. }) if !gaps.is_empty() => EvalError::Stuck {
                concept: concept.clone(),
                gaps,
            },
            _ => EvalError::Stuck {
                concept: concept.clone(),
                gaps: vec![Gap {
                    concept: concept.clone(),
                    reason: GapReason::AllFailed,
                }],
            },
        })
    }

    fn apply(
        &mut self,
        concept: &Concept,
        realization: &Realization,
        declared_effect: Effect,
    ) -> Result<(Concept, Effect), EvalError> {
        match &realization.spec {
            RealizationSpec::Native { native } => {
                let entry =
                    self.registry
                        .get(native)
                        .cloned()
                        .ok_or_else(|| EvalError::MissingNative {
                            realization: realization.name.clone(),
                            native: native.clone(),
                        })?;
                if !entry.arity.accepts(concept.arity()) {
                    return Err(crate::native::type_error(
                        &realization.name,
                        &entry.arity.describe(),
                        concept,
                    ));
                }
                let (args, args_effect) = self.reduce_args(concept.args(), entry.args)?;
                let func = entry.func;
                self.depth += 1;
                let result = func(self, &args);
                self.depth -= 1;
                let value = result?;
                self.check_size(&value)?;
                Ok((value, declared_effect.join(args_effect)))
            }

            RealizationSpec::Composed { body } => {
                let (args, args_effect) = self.reduce_args(concept.args(), ArgStrategy::Eager)?;
                let bound = substitute_positional(body, &args);
                self.check_size(&bound)?;
                let (value, body_effect) = self.descend(&bound)?;
                Ok((value, declared_effect.join(args_effect).join(body_effect)))
            }

            RealizationSpec::Rule {
                pattern,
                condition,
                produce,
            } => {
                let bindings = generalizes(pattern, concept).ok_or_else(|| {
                    crate::native::native_error(
                        &realization.name,
                        "rule pattern does not match this concept",
                    )
                })?;
                let mut effect = declared_effect;
                if let Some(condition) = condition {
                    let instantiated = substitute(condition, &bindings);
                    let (holds, cond_effect) = self.condition_holds(&instantiated)?;
                    effect = effect.join(cond_effect);
                    if !holds {
                        return Err(crate::native::native_error(
                            &realization.name,
                            "rule condition does not hold",
                        ));
                    }
                }
                let produced = substitute(produce, &bindings);
                self.check_size(&produced)?;
                let (value, produced_effect) = self.descend(&produced)?;
                Ok((value, effect.join(produced_effect)))
            }

            RealizationSpec::Neural { prompt, parse } => {
                let seat = self.neural.ok_or_else(|| {
                    crate::native::native_error(
                        &realization.name,
                        "no neural seat is configured for this brain",
                    )
                })?;
                let (args, args_effect) = self.reduce_args(concept.args(), ArgStrategy::Eager)?;
                let bound_prompt = substitute_positional(prompt, &args);
                self.depth += 1;
                let result = seat.invoke(&bound_prompt, parse, self);
                self.depth -= 1;
                let value = result?;
                self.check_size(&value)?;
                Ok((value, declared_effect.join(args_effect)))
            }

            RealizationSpec::External { spec } => {
                let runner = self.external.ok_or_else(|| {
                    crate::native::native_error(
                        &realization.name,
                        "no external runner is configured for this brain",
                    )
                })?;
                let (args, args_effect) = self.reduce_args(concept.args(), ArgStrategy::Eager)?;
                let spec = spec.clone();
                self.depth += 1;
                let result = runner.run(&spec, &args, self);
                self.depth -= 1;
                let value = result?;
                self.check_size(&value)?;
                Ok((value, declared_effect.join(args_effect)))
            }
        }
    }

    /// A rule condition holds when it reduces to `true`, or when it is a
    /// concept the store actively asserts. The second form is what lets
    /// `Symmetric<FriendWith>` act as a condition without needing a realization
    /// of its own.
    fn condition_holds(&mut self, condition: &Concept) -> Result<(bool, Effect), EvalError> {
        if self.store.holds(condition)? {
            return Ok((true, Effect::Read));
        }
        match self.descend(condition) {
            Ok((value, effect)) => {
                let truthy = value.as_ground().and_then(|g| g.as_bool()).unwrap_or(false);
                Ok((truthy, effect.join(Effect::Read)))
            }
            Err(err) if err.is_retryable() => Ok((false, Effect::Read)),
            Err(err) => Err(err),
        }
    }

    fn reduce_args(
        &mut self,
        args: &[Concept],
        strategy: ArgStrategy,
    ) -> Result<(Vec<Concept>, Effect), EvalError> {
        let mut out = Vec::with_capacity(args.len());
        let mut effect = Effect::Pure;
        for (index, arg) in args.iter().enumerate() {
            if strategy.evaluates(index) {
                let (value, arg_effect) = self.descend(arg)?;
                // An eager native handed a half-reduced argument would compute
                // over garbage, so the gap propagates outward instead.
                out.push(value);
                effect = effect.join(arg_effect);
            } else {
                out.push(arg.clone());
            }
        }
        Ok((out, effect))
    }

    fn descend(&mut self, concept: &Concept) -> Result<(Concept, Effect), EvalError> {
        self.depth += 1;
        let result = self.reduce(concept);
        self.depth -= 1;
        result
    }

    fn check_size(&self, concept: &Concept) -> Result<(), EvalError> {
        match self.budget.check_size(concept.size()) {
            Some(limit) => Err(EvalError::Exhausted {
                concept: concept.clone(),
                limit,
            }),
            None => Ok(()),
        }
    }

    /// The authority an application actually needs: the maximum of what the
    /// stored realization claims and what its native declares.
    ///
    /// A realization claiming `Pure` must not be able to smuggle in a native
    /// that opens a socket, and a composition is as dangerous as its most
    /// dangerous step.
    fn effective_effect(&self, realization: &Realization) -> Result<Effect, EvalError> {
        let declared = realization.effect;
        if let RealizationSpec::Native { native } = &realization.spec {
            let entry = self
                .registry
                .get(native)
                .ok_or_else(|| EvalError::MissingNative {
                    realization: realization.name.clone(),
                    native: native.clone(),
                })?;
            return Ok(declared.join(entry.effect));
        }
        Ok(declared)
    }

    fn candidates_for(&mut self, head: &Concept) -> Result<Vec<Arc<Realization>>, EvalError> {
        let id = head.content_id();
        if let Some(hit) = self.realizations.get(&id) {
            return Ok(hit.clone());
        }
        let loaded: Vec<Arc<Realization>> = self
            .store
            .realizations_for(head)?
            .into_iter()
            .filter(select::is_selectable)
            .filter(|r| self.is_runnable(r))
            .map(Arc::new)
            .collect();
        self.realizations.insert(id, loaded.clone());
        Ok(loaded)
    }

    /// A realization whose machinery this brain does not have is not selectable.
    ///
    /// A brain with no LLM seat has no neural realizations, which is a coherent
    /// configuration rather than a failure. Excluding them during selection
    /// beats picking one and failing at apply time, because the alternatives
    /// still get their turn.
    fn is_runnable(&self, realization: &Realization) -> bool {
        match &realization.spec {
            RealizationSpec::Neural { .. } => self.neural.is_some(),
            RealizationSpec::External { .. } => self.external.is_some(),
            RealizationSpec::Native { native } => self.registry.contains(native),
            RealizationSpec::Composed { .. } | RealizationSpec::Rule { .. } => true,
        }
    }

    fn rank(&mut self, candidates: Vec<Arc<Realization>>) -> Vec<Scored> {
        let now = self.now;
        let scored: Vec<Scored> = candidates
            .into_iter()
            .map(|r| {
                let evidence = self.evidence_for(&r);
                let fit = select::context_fit(&evidence, &self.situation);
                select::score(r, fit, now)
            })
            .collect();
        select::rank(scored)
    }

    /// Stored `WorksWellWith` / `WorksPoorlyWith` claims about a realization.
    ///
    /// These are ordinary concepts in the store, so which contexts a
    /// realization suits is something Spoon learns rather than something baked
    /// into the ranker.
    fn evidence_for(&mut self, realization: &Realization) -> Vec<(FitKind, Concept)> {
        if let Some(hit) = self.fit_evidence.get(&realization.name) {
            return hit.clone();
        }
        let subject = Concept::named(&realization.name);
        let well = SymbolId::of("works-well-with");
        let poorly = SymbolId::of("works-poorly-with");

        let mut evidence = Vec::new();
        if let Ok(mentions) = self.store.concepts_containing(subject.content_id(), 64) {
            for claim in mentions {
                let Some(head) = claim.head_symbol() else {
                    continue;
                };
                let kind = if head == well {
                    FitKind::Well
                } else if head == poorly {
                    FitKind::Poorly
                } else {
                    continue;
                };
                // WorksWellWith<realization, context>
                if claim.arg(0) == Some(&subject)
                    && let Some(context) = claim.arg(1)
                {
                    evidence.push((kind, context.clone()));
                }
            }
        }
        self.fit_evidence
            .insert(realization.name.clone(), evidence.clone());
        evidence
    }

    /// Evidence only moves in memory during evaluation. Persisting every use
    /// mid-loop would put a write on the hot path and, worse, would record
    /// outcomes for an evaluation that might still be abandoned.
    fn record_use(&mut self, realization: &Realization, succeeded: bool) {
        self.trace.notes.push(Note {
            depth: self.depth,
            message: format!(
                "{} {}",
                realization.name,
                if succeeded { "succeeded" } else { "failed" }
            ),
        });
    }

    /// Write the evaluation's outcomes back to the store.
    ///
    /// Separate from evaluation so the caller decides when a run counts. A
    /// speculative evaluation that gets thrown away should not teach Spoon
    /// anything.
    pub fn commit_evidence(&self) -> Result<usize, EvalError> {
        let mut written = 0;
        for step in &self.trace.steps {
            let Some(name) = &step.realization else {
                continue;
            };
            let succeeded = matches!(step.outcome, StepOutcome::Reduced(_));
            match self.store.record_realization_use(name, succeeded, self.now) {
                Ok(()) => written += 1,
                // A realization removed mid-run is not worth failing the commit
                // over; the trace still records what happened.
                Err(spoon_store::StoreError::MissingRecord { .. }) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(written)
    }
}

impl Ctx for Evaluator<'_> {
    fn eval(&mut self, concept: &Concept) -> EvalResult {
        self.reduce(concept).map(|(c, _)| c)
    }

    fn store(&self) -> &Store {
        self.store
    }

    fn now(&self) -> DateTime<Utc> {
        self.now
    }

    fn situation(&self) -> &[Concept] {
        &self.situation
    }

    fn note(&mut self, message: &str) {
        self.trace.notes.push(Note {
            depth: self.depth,
            message: message.to_string(),
        });
    }
}
