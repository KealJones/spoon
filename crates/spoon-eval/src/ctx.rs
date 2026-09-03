//! The handle a native gets while it runs.

use chrono::{DateTime, Utc};
use spoon_concept::Concept;
use spoon_store::Store;

use crate::error::EvalResult;

/// What a native can reach.
///
/// Deliberately narrow. A native can reduce concepts, read the store, ask the
/// time, and leave a note in the trace. It cannot adjust the budget, choose a
/// realization, or reach into the evaluator's state, because those are the
/// decisions the interior has to own.
///
/// Object-safe on purpose: natives are plain function pointers held in a
/// registry, so they receive `&mut dyn Ctx` rather than a generic parameter.
pub trait Ctx {
    /// Reduce a concept, charging the same budget as any other work.
    ///
    /// Lazy natives use this to reduce only what they need. `If` reduces its
    /// condition and exactly one branch.
    fn eval(&mut self, concept: &Concept) -> EvalResult;

    /// The persistent store, for natives whose effect level admits reading or
    /// writing it.
    fn store(&self) -> &Store;

    /// Current time. Injected rather than read from the clock directly so that
    /// tests can pin it and get reproducible activation numbers.
    fn now(&self) -> DateTime<Utc>;

    /// Concepts describing the current situation, matched against a
    /// realization's `WorksWellWith` evidence during selection.
    fn situation(&self) -> &[Concept];

    /// Leave a note in the evaluation trace. Credit assignment reads these
    /// later to work out which stage produced a wrong answer.
    fn note(&mut self, message: &str);
}
