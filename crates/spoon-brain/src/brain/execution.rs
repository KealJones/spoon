//! Execute a turn's moves and keep the evidence needed to revise it later.

use super::*;
use spoon_concept::Effect;

#[derive(Default)]
pub(super) struct Attempt {
    pub result: Option<Concept>,
    pub goal: Option<Concept>,
    pub gaps: Vec<Concept>,
    pub realizations: Vec<(String, bool)>,
    pub trace: Vec<crate::TraceStep>,
    pub rules: Vec<String>,
    pub assertions: Vec<(i64, Concept)>,
    pub external_effects: bool,
}

impl Attempt {
    fn record(&mut self, trace: &spoon_eval::Trace, symbols: &SymbolTable, metrics: &mut TurnMetrics) {
        metrics.eval_nodes += trace.nodes_used;
        for step in &trace.steps {
            self.external_effects |= step.effect >= Effect::Write;
            if let Some(name) = &step.realization {
                self.realizations.push((name.to_string(), matches!(step.outcome, spoon_eval::StepOutcome::Reduced(_))));
            }
            self.trace.push(crate::TraceStep {
                concept: render(&step.concept, symbols),
                realization: step.realization.as_ref().map(|r| r.to_string()),
                alternatives: step.alternatives.iter().map(|(n, s)| (n.to_string(), *s)).collect(),
                explored: step.explored,
                effect: step.effect.as_str().to_string(),
                depth: step.depth,
                outcome: match &step.outcome {
                    spoon_eval::StepOutcome::Reduced(c) => format!("reduced to {}", render(c, symbols)),
                    spoon_eval::StepOutcome::Irreducible => "irreducible".into(),
                    spoon_eval::StepOutcome::Failed(why) => format!("failed: {why}"),
                },
            });
        }
        for concept in trace.irreducible().into_iter().chain(trace.failures().into_iter().map(|(c, _, _)| c)) {
            if self.gaps.len() < 8 && !self.gaps.contains(concept) {
                self.gaps.push(concept.clone());
            }
        }
    }
}

impl Brain {
    pub(super) fn execute(&mut self, moves: &[Move], metrics: &mut TurnMetrics) -> spoon_store::Result<Attempt> {
        let started = Instant::now();
        let mut attempt = Attempt::default();
        for m in moves {
            attempt.goal = Some(m.concept().clone());
            let value = self.act(m, metrics, &mut attempt)?;
            if value.is_some() { attempt.result = value; }
        }
        metrics.millis_interior += started.elapsed().as_millis() as u64;
        Ok(attempt)
    }

    fn act(&mut self, m: &Move, metrics: &mut TurnMetrics, attempt: &mut Attempt) -> spoon_store::Result<Option<Concept>> {
        match m {
            Move::Assert(c) => {
                let id = self.store.assert_concept(c, Provenance::User { episode: Some(self.next_episode) }, Some(self.next_episode), None)?;
                attempt.assertions.push((id.0, c.clone()));
                self.remember_names(c);
                Ok(Some(c.clone()))
            }
            Move::Chat(c) => Ok(Some(c.clone())),
            Move::Do(c) if holes(c).is_empty() && is_declarative(c) && self.is_irreducible_data(c) => {
                self.act(&Move::Assert(c.clone()), metrics, attempt)
            }
            Move::Ask(goal) => {
                let index = DiscriminationTree::from_store(&self.store)
                    .map_err(|_| spoon_store::StoreError::HoleNotStorable)?;
                let mut engine = Engine::new(&self.store, &index).with_budget(self.config.derive_budget);
                let derived = engine.derive(goal);
                metrics.derive_steps += engine.steps_used();
                let derived = match derived {
                    Ok(d) => d,
                    Err(err) => return Ok(Some(Concept::call("error", [Concept::text(err.to_string())]))),
                };
                for d in &derived { attempt.rules.extend(d.rules_used().iter().map(|r| r.to_string())); }
                if !derived.is_empty() {
                    return Ok(Some(if holes(goal).is_empty() { Concept::bool(true) } else {
                        Concept::call("list-of", derived.iter().map(|d| d.goal.clone()).collect::<Vec<_>>())
                    }));
                }
                let outcome = self.evaluate_for_turn(goal, metrics, attempt)?;
                Ok(Some(match outcome {
                    Outcome::Value(v) if v != *goal && is_answer(&self.store, &v) => v,
                    Outcome::Value(_) => Concept::call("unknown", [goal.clone()]),
                    other => outcome_concept(other),
                }))
            }
            Move::Do(c) => Ok(Some(outcome_concept(self.evaluate_for_turn(c, metrics, attempt)?))),
        }
    }

    fn evaluate_for_turn(&self, c: &Concept, metrics: &mut TurnMetrics, attempt: &mut Attempt) -> spoon_store::Result<Outcome> {
        let before = self.interior_seat_calls();
        let mut evaluator = Evaluator::new(&self.store, &self.registry)
            .with_budget(self.config.eval_budget).with_permission(self.config.permission);
        let outcome = evaluator.evaluate(c);
        // This is measured at the seat boundary rather than a permanently zero field.
        metrics.interior_model_calls += self.interior_seat_calls().saturating_sub(before);
        attempt.record(evaluator.trace(), &self.symbols, metrics);
        if let Err(err) = evaluator.commit_evidence() { return Ok(Outcome::Failed(err)); }
        Ok(outcome)
    }

    fn interior_seat_calls(&self) -> u64 {
        [Seat::Ears, Seat::Mouth, Seat::Teacher].iter().map(|s| self.counters.get(*s)).sum()
    }

    pub(super) fn is_irreducible_data(&self, expr: &Concept) -> bool {
        let mut probe = Evaluator::new(&self.store, &self.registry)
            .with_budget(self.config.eval_budget).with_permission(PermissionMode::AlwaysAsk);
        matches!(probe.evaluate(expr), Outcome::Value(ref v) if v == expr && probe.trace().steps.iter().all(|s| s.realization.is_none()))
    }
}

fn outcome_concept(outcome: Outcome) -> Concept {
    match outcome {
        Outcome::Value(v) => v,
        Outcome::Stuck { concept, .. } | Outcome::Exhausted { concept, .. } => Concept::call("cannot-yet", [concept]),
        Outcome::NeedsPermission { concept, effect, .. } => Concept::call("needs-permission", [concept, Concept::text(effect.as_str())]),
        Outcome::Failed(err) => Concept::call("error", [Concept::text(err.to_string())]),
    }
}
