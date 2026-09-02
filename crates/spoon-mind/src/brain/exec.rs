//! Plan + execute handling for the Brain.

use spoon_core::can::Can;
use spoon_core::kernel::{Budget, Ctx};
use spoon_core::kernel::eval;
use spoon_core::types::*;

use crate::plan::{ExecOutcome, ExecState, Executor, Planner};

use super::{Brain, BrainHost};
use super::respond::exec_result_plan;
use super::session::{Pending, PendingExecKind, Session};

pub(super) enum ResumeResult {
    Done(ResponsePlan),
    StillPending(ResponsePlan, Pending),
    NotAnAnswer,
}

fn make_call_fn<'a>(
    can: &'a Can,
    brain: &'a Brain,
    host: &'a BrainHost,
) -> impl FnMut(&ActionId, &[Value]) -> Result<Value, spoon_core::kernel::EvalError> + 'a {
    move |action_id: &ActionId, args: &[Value]| {
        let mut ctx = Ctx::new(can, &brain.kernel, host);
        ctx.budget = Budget::generous();
        if brain.kernel.has(action_id) {
            brain.kernel.call(&mut ctx, action_id, args)
        } else if let Some(action) = can.action(action_id) {
            if let Impl::Program { program } = &action.imp {
                eval::eval_program(&mut ctx, program, args)
            } else {
                Err(spoon_core::kernel::EvalError::UnknownAction(action_id.clone()))
            }
        } else {
            Err(spoon_core::kernel::EvalError::UnknownAction(action_id.clone()))
        }
    }
}

impl Brain {
    pub(super) fn handle_plan(
        &self,
        intent: &Intent,
        moves_before: Vec<Move>,
        session_id: &str,
        trace: &mut Vec<String>,
    ) -> anyhow::Result<ResponsePlan> {
        let can = self.can.lock();
        let planner = Planner::new(&can);
        let outcome = planner.plan(intent, &Default::default());

        match outcome {
            PlanOutcome::Plan { plan } => {
                if self.cfg.debug {
                    trace.push(format!(
                        "plan: {} nodes, cost={:.1}, effect={:?}",
                        plan.nodes.len(), plan.cost, plan.effect
                    ));
                }

                let host = BrainHost { permission_mode: self.cfg.permission_mode };
                let executor = Executor::new(self.cfg.permission_mode);
                let mut exec_state = ExecState::default();
                let mut call_fn = make_call_fn(&can, self, &host);
                let exec_result = executor.run(&can, &plan, &mut exec_state, &mut call_fn);
                drop(call_fn);
                drop(can);

                self.handle_exec_outcome(
                    exec_result, intent, plan, exec_state, moves_before, session_id, trace,
                )
            }
            PlanOutcome::NoProducer { .. } | PlanOutcome::UnknownAction { .. } => {
                let mut moves = moves_before;
                moves.push(Move::CannotDo {
                    what: intent.sce.clone(),
                    reason: format!("{:?}", outcome),
                });
                Ok(ResponsePlan::new(moves))
            }
            PlanOutcome::Timeout => {
                let mut moves = moves_before;
                moves.push(Move::Error { message: "planning timed out".into() });
                Ok(ResponsePlan::new(moves))
            }
        }
    }

    fn handle_exec_outcome(
        &self,
        outcome: ExecOutcome,
        intent: &Intent,
        plan: Plan,
        state: ExecState,
        moves_before: Vec<Move>,
        session_id: &str,
        trace: &mut Vec<String>,
    ) -> anyhow::Result<ResponsePlan> {
        match outcome {
            ExecOutcome::Done { value, trace: steps } => {
                if self.cfg.debug {
                    trace.push(format!("exec: Done, {} steps", steps.len()));
                }
                let mut can = self.can.lock();
                for aid in plan.actions() {
                    can.touch(&aid, true);
                }
                let goal_action = plan.actions().into_iter().last()
                    .unwrap_or_else(|| ActionId("unknown".into()));
                Ok(exec_result_plan(value, &goal_action, plan.action_count(), moves_before))
            }
            ExecOutcome::NeedInput { node, ty, input_name, for_action, .. } => {
                let mut moves = moves_before;
                moves.push(Move::Elicit {
                    input_name: input_name.clone(),
                    ty: ty.clone(),
                    for_action: for_action.clone(),
                });
                let mut sessions = self.sessions.lock();
                let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
                session.pending = Some(Pending::Exec {
                    intent: intent.clone(), plan, state,
                    kind: PendingExecKind::Input { node, input_name, ty },
                });
                Ok(ResponsePlan::new(moves))
            }
            ExecOutcome::NeedPermission { node, action, effect, description, .. } => {
                let mut moves = moves_before;
                moves.push(Move::AskPermission { action: action.clone(), effect, description });
                let mut sessions = self.sessions.lock();
                let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
                session.pending = Some(Pending::Exec {
                    intent: intent.clone(), plan, state,
                    kind: PendingExecKind::Permission { node },
                });
                Ok(ResponsePlan::new(moves))
            }
            ExecOutcome::NeedChoice { node, options, .. } => {
                let labels: Vec<String> = options.iter().map(|v| v.render()).collect();
                let mut moves = moves_before;
                moves.push(Move::Clarify {
                    question: "which option?".into(), options: labels, slot_type: None,
                });
                let mut sessions = self.sessions.lock();
                let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
                session.pending = Some(Pending::Exec {
                    intent: intent.clone(), plan, state,
                    kind: PendingExecKind::Choice { node },
                });
                Ok(ResponsePlan::new(moves))
            }
            ExecOutcome::Failed { error, .. } => {
                let mut moves = moves_before;
                moves.push(Move::Error { message: error });
                Ok(ResponsePlan::new(moves))
            }
        }
    }

    pub(super) fn resume_exec(
        &self,
        text: &str,
        intent: &Intent,
        plan: &Plan,
        state: &mut ExecState,
        kind: &PendingExecKind,
        _session: &mut Session,
        trace: &mut Vec<String>,
    ) -> ResumeResult {
        use super::respond::{parse_permission, parse_choice};

        match kind {
            PendingExecKind::Permission { node } => {
                match parse_permission(text) {
                    Some(true) => { state.granted.insert(*node); }
                    Some(false) => {
                        return ResumeResult::Done(ResponsePlan::single(Move::Ack {
                            summary: "cancelled".into(),
                        }));
                    }
                    None => return ResumeResult::NotAnAnswer,
                }
            }
            PendingExecKind::Input { node, ty, .. } => {
                let value = match ty {
                    Type::Text => Value::Text(text.to_string()),
                    Type::Int => text.trim().parse::<i64>()
                        .map(Value::Int).unwrap_or(Value::Text(text.to_string())),
                    Type::Float => text.trim().parse::<f64>()
                        .map(Value::Float).unwrap_or(Value::Text(text.to_string())),
                    _ => Value::Text(text.to_string()),
                };
                state.answers.insert(*node, value);
            }
            PendingExecKind::Choice { node } => {
                match parse_choice(text) {
                    Some(idx) => { state.chosen.insert(*node, idx); }
                    None => return ResumeResult::NotAnAnswer,
                }
            }
        }

        let can = self.can.lock();
        let host = BrainHost { permission_mode: self.cfg.permission_mode };
        let executor = Executor::new(self.cfg.permission_mode);
        let mut call_fn = make_call_fn(&can, self, &host);
        let result = executor.run(&can, plan, state, &mut call_fn);
        drop(call_fn);
        drop(can);

        match result {
            ExecOutcome::Done { value, trace: steps } => {
                if self.cfg.debug {
                    trace.push(format!("resume exec: Done, {} steps", steps.len()));
                }
                let goal_action = plan.actions().into_iter().last()
                    .unwrap_or_else(|| ActionId("unknown".into()));
                ResumeResult::Done(exec_result_plan(value, &goal_action, plan.action_count(), vec![]))
            }
            ExecOutcome::NeedInput { node, ty, input_name, .. } => {
                let new_pending = Pending::Exec {
                    intent: intent.clone(), plan: plan.clone(),
                    state: std::mem::take(state),
                    kind: PendingExecKind::Input { node, input_name: input_name.clone(), ty: ty.clone() },
                };
                let rp = ResponsePlan::single(Move::Elicit {
                    input_name, ty,
                    for_action: intent.routes.first().cloned()
                        .unwrap_or_else(|| ActionId("unknown".into())),
                });
                ResumeResult::StillPending(rp, new_pending)
            }
            ExecOutcome::NeedPermission { node, action, effect, description, .. } => {
                let new_pending = Pending::Exec {
                    intent: intent.clone(), plan: plan.clone(),
                    state: std::mem::take(state),
                    kind: PendingExecKind::Permission { node },
                };
                let rp = ResponsePlan::single(Move::AskPermission { action, effect, description });
                ResumeResult::StillPending(rp, new_pending)
            }
            ExecOutcome::NeedChoice { node, options, .. } => {
                let labels: Vec<String> = options.iter().map(|v| v.render()).collect();
                let new_pending = Pending::Exec {
                    intent: intent.clone(), plan: plan.clone(),
                    state: std::mem::take(state),
                    kind: PendingExecKind::Choice { node },
                };
                let rp = ResponsePlan::single(Move::Clarify {
                    question: "which option?".into(), options: labels, slot_type: None,
                });
                ResumeResult::StillPending(rp, new_pending)
            }
            ExecOutcome::Failed { error, .. } => {
                ResumeResult::Done(ResponsePlan::single(Move::Error { message: error }))
            }
        }
    }
}
