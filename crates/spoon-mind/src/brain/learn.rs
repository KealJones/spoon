//! Learning: synthesize new actions from examples, persist, re-run.

use spoon_core::can::Can;
use spoon_core::kernel::Kernel;
use spoon_core::store::Store;
use spoon_core::types::*;

use crate::discourse::{self, ground_all};
use crate::grow::{self, SynthBudget, SynthOutcome};

use spoon_lang::ears::Gate;

use super::Brain;
use super::session::{Pending, Session};

/// Attempt to learn verb `v` from stored property facts `rel.V(x, y)`.
///
/// Returns the new Action and program description on success, or a
/// human-readable reason on failure.
pub fn learn_from_examples(
    verb: &str,
    can: &Can,
    store: &Store,
    kernel: &Kernel,
) -> Result<(Action, String), String> {
    let rel_id = ActionId(format!("rel.{}", verb));
    let facts = store
        .query_facts(&rel_id, &[])
        .map_err(|e| format!("store error: {e}"))?;
    let positive: Vec<&Fact> = facts.iter().filter(|f| f.truth).collect();
    if positive.len() < 2 {
        return Err(format!(
            "need at least 2 examples for '{}', have {}",
            verb,
            positive.len()
        ));
    }

    let mut examples = Vec::new();
    let mut param_types: Option<Vec<Type>> = None;
    let mut ret_type: Option<Type> = None;

    for fact in &positive {
        if fact.args.len() < 2 {
            continue;
        }
        let inputs: Vec<Value> = fact.args[..fact.args.len() - 1].to_vec();
        let output = fact.args.last().unwrap().clone();

        let input_tys: Vec<Type> = inputs.iter().map(|v| v.type_of()).collect();
        let out_ty = output.type_of();

        match &param_types {
            None => {
                param_types = Some(input_tys);
                ret_type = Some(out_ty);
            }
            Some(existing) => {
                if existing.len() != input_tys.len() {
                    continue;
                }
            }
        }

        examples.push(Example { inputs, output });
    }

    if examples.len() < 2 {
        return Err(format!(
            "could not build 2 consistent examples for '{}'",
            verb
        ));
    }

    let param_types = param_types.unwrap();
    let ret_type = ret_type.unwrap();

    let spec = Spec {
        id: format!("user:{}:{}", verb, now_ms()),
        name_hint: verb.to_string(),
        verbs: vec![verb.to_string()],
        phrasings: vec![],
        params: param_types,
        param_names: vec![],
        ret: ret_type,
        examples,
        description: String::new(),
        source: "user".to_string(),
    };

    if let Err(e) = spec.validate() {
        return Err(format!("spec validation: {e}"));
    }

    let budget = SynthBudget::default();
    match grow::synthesize(&spec, can, kernel, &budget) {
        SynthOutcome::Found { program, .. } => {
            let desc = grow::describe(&program);
            let action = grow::action_from_program(&spec, &program);
            Ok((action, desc))
        }
        SynthOutcome::Exhausted { tried, millis } => {
            Err(format!(
                "synthesis exhausted after {tried} programs ({millis}ms) - try different examples"
            ))
        }
        SynthOutcome::Budget { reason, .. } => {
            Err(format!("synthesis budget: {reason}"))
        }
    }
}

impl Brain {
    pub(super) fn handle_unknown_verb_followup(
        &self,
        text: &str,
        verb: &str,
        original_sce: &str,
        signals: &[Signal],
        session: &mut Session,
        trace: &mut Vec<String>,
    ) -> Option<ResponsePlan> {
        let gate = self.gate.lock();
        let clauses = match gate.parse(text) {
            Ok(c) if !c.is_empty() => c,
            _ => {
                let ears = self.ears.lock();
                match ears.hear_native(text, &*gate) {
                    Some(r) if !r.clauses.is_empty() => r.clauses,
                    _ => return None,
                }
            }
        };
        drop(gate);

        let has_assert = clauses.iter().any(|c| matches!(c.act, Act::Assert));
        if !has_assert {
            return None;
        }

        {
            let mut can = self.can.lock();
            let store = self.store.lock();
            let gs = ground_all(&mut session.discourse, &clauses, &can);
            let mut fw = discourse::FactWriter { can: &mut can, store: &store };
            for g in &gs {
                let _ = discourse::assert_grounded(&mut fw, g, "user", None);
            }
        }

        let learn_result = {
            let can = self.can.lock();
            let store = self.store.lock();
            learn_from_examples(verb, &can, &store, &self.kernel)
        };

        match learn_result {
            Ok((action, desc)) => {
                self.register_learned_action(&action).ok()?;
                self.counters.synth_attempted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.counters.synth_succeeded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                if self.cfg.debug {
                    trace.push(format!("learned '{}': {}", verb, desc));
                }

                let intent = Intent {
                    goal: Goal::Action { action: action.id.clone() },
                    signals: signals.to_vec(),
                    routes: vec![action.id.clone()],
                    sce: original_sce.to_string(),
                };
                let exec_plan = self.handle_plan(&intent, vec![], "", trace).ok()?;
                let mut plan = exec_plan;
                plan.push(Move::Learned {
                    what: format!("learned '{}': {}", verb, desc),
                });
                Some(plan)
            }
            Err(reason) => {
                if self.cfg.debug {
                    trace.push(format!("learn failed: {}", reason));
                }
                session.pending = Some(Pending::UnknownVerb {
                    verb: verb.to_string(),
                    sce: original_sce.to_string(),
                    signals: signals.to_vec(),
                });
                self.gate.lock().lex.add_noun(verb);
                Some(ResponsePlan::single(Move::AskExamples {
                    capability: verb.to_string(),
                    signature: format!("need more examples: {}", reason),
                }))
            }
        }
    }

    pub(super) fn register_learned_action(&self, action: &Action) -> anyhow::Result<()> {
        let mut can = self.can.lock();
        let store = self.store.lock();
        for v in &action.verbs {
            let rel_id = ActionId(format!("rel.{}", v));
            if can.action(&rel_id).map_or(false, |a| a.tier == Tier::Provisional) {
                can.remove_action(&rel_id);
                let _ = store.delete_action(&rel_id);
            }
        }
        can.add_action(action.clone());
        store.save_action(action)?;

        let mut gate = self.gate.lock();
        for v in &action.verbs {
            gate.lex.add_verb(v);
        }
        Ok(())
    }
}
