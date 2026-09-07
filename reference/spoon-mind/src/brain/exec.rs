//! Plan + execute handling for the Brain.

use spoon_core::can::Can;
use spoon_core::kernel::{eval, Budget, Ctx, EvalError};
use spoon_core::types::*;

use crate::dispatch::Present;
use crate::plan::{ExecOutcome, ExecState, Executor, Planner};

use super::respond::{elicited_text, parse_choice, parse_permission, present_result};
use super::session::{Pending, PendingExecKind};
use super::{Brain, BrainHost, TurnFlags};

pub(super) enum ResumeResult {
    Done(ResponsePlan),
    NotAnAnswer,
}

fn make_call_fn<'a>(
    can: &'a Can,
    brain: &'a Brain,
    host: &'a BrainHost,
) -> impl FnMut(&ActionId, &[Value]) -> Result<Value, EvalError> + 'a {
    move |action_id: &ActionId, args: &[Value]| {
        let mut ctx = Ctx::new(can, &brain.kernel, host);
        ctx.budget = Budget::generous();
        if brain.kernel.has(action_id) {
            brain.kernel.call(&mut ctx, action_id, args)
        } else if let Some(Impl::Program { program }) = can.action(action_id).map(|a| &a.imp) {
            eval::eval_program(&mut ctx, program, args)
        } else {
            Err(EvalError::UnknownAction(action_id.clone()))
        }
    }
}

impl Brain {
    pub(super) fn handle_plan(
        &self,
        intent: &Intent,
        moves_before: Vec<Move>,
        present: &Present,
        session_id: &str,
        flags: &mut TurnFlags,
        trace: &mut Vec<String>,
    ) -> anyhow::Result<ResponsePlan> {
        let outcome = Planner::new(&self.can.lock()).plan(intent, &Default::default());
        match outcome {
            PlanOutcome::Plan { plan } => {
                if self.cfg.debug {
                    trace.push(format!(
                        "plan: {} nodes, cost={:.1}, effect={:?}",
                        plan.nodes.len(),
                        plan.cost,
                        plan.effect
                    ));
                }
                Ok(self.run_plan(intent, plan, ExecState::default(), present, moves_before, session_id, flags, trace))
            }
            PlanOutcome::NoProducer { .. } | PlanOutcome::UnknownAction { .. } => {
                let mut moves = moves_before;
                moves.push(Move::CannotDo { what: intent.sce.clone(), reason: format!("{:?}", outcome) });
                Ok(ResponsePlan::new(moves))
            }
            PlanOutcome::Timeout => {
                let mut moves = moves_before;
                moves.push(Move::Error { message: "planning timed out".into() });
                Ok(ResponsePlan::new(moves))
            }
        }
    }

    /// Run (or continue) a plan. The host locks the store itself, so the
    /// brain holds only the CAN lock across the executor call. When the
    /// executor needs the user, the state is parked in the session.
    #[allow(clippy::too_many_arguments)]
    fn run_plan(
        &self,
        intent: &Intent,
        plan: Plan,
        mut state: ExecState,
        present: &Present,
        moves_before: Vec<Move>,
        session_id: &str,
        flags: &mut TurnFlags,
        trace: &mut Vec<String>,
    ) -> ResponsePlan {
        let outcome = {
            let can = self.can.lock();
            let host = BrainHost::with_store(self.cfg.permission_mode, &self.store);
            let executor = Executor::new(self.cfg.permission_mode);
            let mut call_fn = make_call_fn(&can, self, &host);
            executor.run(&can, &plan, &mut state, &mut call_fn)
        };

        let mut moves = moves_before;
        let park = |kind: PendingExecKind| Pending::Exec {
            intent: intent.clone(),
            plan: plan.clone(),
            state,
            kind,
            present: present.clone(),
        };
        let pending = match outcome {
            ExecOutcome::Done { value, trace: steps } => {
                if self.cfg.debug {
                    trace.push(format!("exec: Done, {} steps", steps.len()));
                }
                let mut can = self.can.lock();
                for aid in plan.actions() {
                    if can.action(&aid).is_some_and(|a| a.tier != Tier::Kernel) {
                        flags.used_learned_action = true;
                    }
                    can.touch(&aid, true);
                }
                flags.plan_steps = plan.action_count();
                let goal = plan.actions().into_iter().last().unwrap_or_else(|| ActionId("unknown".into()));
                return present_result(present, value, &goal, plan.action_count(), moves);
            }
            ExecOutcome::NeedInput { node, ty, input_name, for_action, .. } => {
                moves.push(Move::Elicit { input_name, ty: ty.clone(), for_action });
                park(PendingExecKind::Input { node, ty })
            }
            ExecOutcome::NeedPermission { node, action, effect, description, .. } => {
                moves.push(Move::AskPermission { action, effect, description });
                park(PendingExecKind::Permission { node })
            }
            ExecOutcome::NeedChoice { node, options, .. } => {
                let labels: Vec<String> = options.iter().map(|v| v.render()).collect();
                moves.push(Move::Clarify { question: "which option?".into(), options: labels, slot_type: None });
                park(PendingExecKind::Choice { node })
            }
            ExecOutcome::Failed { error, .. } => {
                moves.push(Move::Error { message: error });
                return ResponsePlan::new(moves);
            }
        };
        self.with_session(session_id, |s| s.pending = Some(pending));
        ResponsePlan::new(moves)
    }

    /// The user replied while a plan was paused. Feed the answer in and run on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resume_exec(
        &self,
        text: &str,
        intent: &Intent,
        plan: Plan,
        mut state: ExecState,
        kind: &PendingExecKind,
        present: &Present,
        session_id: &str,
        flags: &mut TurnFlags,
        trace: &mut Vec<String>,
    ) -> ResumeResult {
        match kind {
            PendingExecKind::Permission { node } => match parse_permission(text) {
                Some(true) => {
                    state.granted.insert(*node);
                }
                Some(false) => {
                    return ResumeResult::Done(ResponsePlan::single(Move::Ack { summary: "cancelled".into() }));
                }
                None => return ResumeResult::NotAnAnswer,
            },
            PendingExecKind::Input { node, ty } => {
                let raw = text.trim();
                let clean = elicited_text(text);
                let value = match ty {
                    Type::Int => raw.parse::<i64>().map(Value::Int).unwrap_or_else(|_| Value::Text(clean)),
                    Type::Float => raw.parse::<f64>().map(Value::Float).unwrap_or_else(|_| Value::Text(clean)),
                    Type::Bool => match clean.to_lowercase().as_str() {
                        "true" | "yes" => Value::Bool(true),
                        "false" | "no" => Value::Bool(false),
                        _ => Value::Text(clean),
                    },
                    Type::Path => Value::Path(clean),
                    Type::Url => Value::Url(clean),
                    Type::Name | Type::Concept(_) => Value::Name(clean),
                    _ => Value::Text(clean),
                };
                state.answers.insert(*node, value);
            }
            PendingExecKind::Choice { node } => match parse_choice(text) {
                Some(idx) => {
                    state.chosen.insert(*node, idx);
                }
                None => return ResumeResult::NotAnAnswer,
            },
        }
        ResumeResult::Done(self.run_plan(intent, plan, state, present, vec![], session_id, flags, trace))
    }
}
