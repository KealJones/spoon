//! Plan executor.
//!
//! Walks a `Plan`'s nodes in index order. Nodes whose values are already in
//! `ExecState::values` are skipped, enabling multi-round-trip execution:
//!
//!   1. Call `run()` -> NeedInput for the first Placeholder.
//!   2. Insert the answer into `state.answers[node]`, call `run()` again.
//!   3. Repeat until Done or NeedPermission (insert into `state.granted`, retry).
//!
//! ## Map execution
//! `PlanNode::Map` applies the action to each element of the list at `over`,
//! with other args fixed. `other_args[i] == None` marks the position that
//! receives the list element; `Some(idx)` marks a fixed arg from `values[idx]`.
//!
//! ## Many-cardinality promotion
//! When a Many-cardinality input receives a single scalar (not a list), the
//! executor wraps it in a one-element Vec before passing to the action closure.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use spoon_core::{ActionId, Can, Effect, EvalError, PermissionMode, Plan, PlanNode, Value};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub type CallFn<'c> = &'c mut dyn FnMut(&ActionId, &[Value]) -> Result<Value, EvalError>;

/// One step recorded in the execution trace.
#[derive(Debug, Clone)]
pub struct StepTrace {
    pub node: usize,
    pub action: Option<ActionId>,
    pub input: Vec<Value>,
    pub output: Option<Value>,
    pub millis: u64,
}

/// Mutable execution state that persists across NeedInput / NeedPermission
/// round trips. The caller creates one instance and passes it on every
/// `Executor::run` call for the same plan.
#[derive(Debug, Default)]
pub struct ExecState {
    /// Computed value at each node index. None = not yet computed.
    pub values: Vec<Option<Value>>,
    /// Answers supplied by the user for Placeholder nodes, keyed by node index.
    pub answers: HashMap<usize, Value>,
    /// Node indices for which the user has granted permission to run.
    pub granted: HashSet<usize>,
    /// For Choice nodes: caller-chosen alternative index (into `alternatives`).
    pub chosen: HashMap<usize, usize>,
}

/// Outcome of one `Executor::run` call.
#[derive(Debug)]
pub enum ExecOutcome {
    /// Execution completed. `trace` covers steps from this call only.
    Done { value: Value, trace: Vec<StepTrace> },
    /// Stopped at a Placeholder. Insert the answer and call `run` again.
    NeedInput {
        node: usize,
        ty: spoon_core::types::value::Type,
        input_name: String,
        for_action: ActionId,
        trace: Vec<StepTrace>,
    },
    /// Stopped before an effectful action that needs confirmation.
    /// Insert `node` into `state.granted` and call `run` again.
    NeedPermission {
        node: usize,
        action: ActionId,
        effect: Effect,
        description: String,
        trace: Vec<StepTrace>,
    },
    /// Choice node with multiple viable options. Insert the chosen alternative
    /// index into `state.chosen[node]` and call `run` again.
    NeedChoice { node: usize, options: Vec<Value>, trace: Vec<StepTrace> },
    /// Unrecoverable error.
    Failed { error: String, at_node: Option<usize>, trace: Vec<StepTrace> },
}

pub struct Executor {
    pub mode: PermissionMode,
}

impl Executor {
    pub fn new(mode: PermissionMode) -> Self {
        Self { mode }
    }

    /// Run or resume execution of `plan`.
    ///
    /// On the first call, `state` should be default-initialised.
    /// On subsequent calls after NeedInput/NeedPermission/NeedChoice, populate
    /// the relevant field (`answers`, `granted`, `chosen`) and call again.
    pub fn run(
        &self,
        can: &Can,
        plan: &Plan,
        state: &mut ExecState,
        call: CallFn<'_>,
    ) -> ExecOutcome {
        let n = plan.nodes.len();
        if state.values.len() < n {
            state.values.resize(n, None);
        }

        let mut trace: Vec<StepTrace> = Vec::new();

        for i in 0..n {
            if state.values[i].is_some() {
                continue; // already computed in a previous run
            }

            let t0 = Instant::now();

            match &plan.nodes[i] {
                PlanNode::Signal { value, .. } => {
                    state.values[i] = Some(value.clone());
                }

                PlanNode::Placeholder { ty, input_name, for_action } => {
                    if let Some(answer) = state.answers.get(&i) {
                        state.values[i] = Some(answer.clone());
                    } else {
                        return ExecOutcome::NeedInput {
                            node: i,
                            ty: ty.clone(),
                            input_name: input_name.clone(),
                            for_action: for_action.clone(),
                            trace,
                        };
                    }
                }

                PlanNode::Action { action: action_id, args } => {
                    let action = match can.action(action_id) {
                        Some(a) => a,
                        None => {
                            return ExecOutcome::Failed {
                                error: format!("unknown action {action_id}"),
                                at_node: Some(i),
                                trace,
                            }
                        }
                    };

                    if self.mode.needs_confirmation(action.effect) && !state.granted.contains(&i) {
                        let input_vals = self.gather_args(state, args);
                        let description = format!(
                            "{} with {}",
                            action.canonical_verb(),
                            input_vals
                                .iter()
                                .map(|v| v.render())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        return ExecOutcome::NeedPermission {
                            node: i,
                            action: action_id.clone(),
                            effect: action.effect,
                            description,
                            trace,
                        };
                    }

                    let input_vals = self.gather_args(state, args);
                    match call(action_id, &input_vals) {
                        Ok(result) => {
                            trace.push(StepTrace {
                                node: i,
                                action: Some(action_id.clone()),
                                input: input_vals,
                                output: Some(result.clone()),
                                millis: t0.elapsed().as_millis() as u64,
                            });
                            state.values[i] = Some(result);
                        }
                        Err(e) => {
                            return ExecOutcome::Failed {
                                error: e.to_string(),
                                at_node: Some(i),
                                trace,
                            };
                        }
                    }
                }

                PlanNode::Map { action: action_id, over, other_args } => {
                    let action = match can.action(action_id) {
                        Some(a) => a,
                        None => {
                            return ExecOutcome::Failed {
                                error: format!("unknown action {action_id}"),
                                at_node: Some(i),
                                trace,
                            }
                        }
                    };

                    if self.mode.needs_confirmation(action.effect) && !state.granted.contains(&i) {
                        return ExecOutcome::NeedPermission {
                            node: i,
                            action: action_id.clone(),
                            effect: action.effect,
                            description: format!("map {} over list", action.canonical_verb()),
                            trace,
                        };
                    }

                    let list_val = state.values[*over].clone().unwrap_or(Value::Null);
                    let items = match list_val {
                        Value::List(items) => items,
                        single => vec![single], // scalar promotion
                    };

                    let mut results = Vec::with_capacity(items.len());
                    for element in &items {
                        let full_args: Vec<Value> = other_args
                            .iter()
                            .map(|slot| match slot {
                                None => element.clone(),
                                Some(idx) => {
                                    state.values[*idx].clone().unwrap_or(Value::Null)
                                }
                            })
                            .collect();
                        match call(action_id, &full_args) {
                            Ok(v) => results.push(v),
                            Err(e) => {
                                return ExecOutcome::Failed {
                                    error: e.to_string(),
                                    at_node: Some(i),
                                    trace,
                                };
                            }
                        }
                    }

                    let out = Value::List(results);
                    trace.push(StepTrace {
                        node: i,
                        action: Some(action_id.clone()),
                        input: items,
                        output: Some(out.clone()),
                        millis: t0.elapsed().as_millis() as u64,
                    });
                    state.values[i] = Some(out);
                }

                PlanNode::Choice { alternatives, ty: _ } => {
                    if let Some(&chosen_alt) = state.chosen.get(&i) {
                        let val = alternatives
                            .get(chosen_alt)
                            .and_then(|&a| state.values.get(a))
                            .and_then(|v| v.clone());
                        state.values[i] = val;
                    } else {
                        let computed: Vec<(usize, Value)> = alternatives
                            .iter()
                            .filter_map(|&a| {
                                state.values.get(a).and_then(|v| v.clone()).map(|v| (a, v))
                            })
                            .collect();
                        if computed.len() == 1 {
                            state.values[i] = Some(computed.into_iter().next().unwrap().1);
                        } else if computed.len() > 1 {
                            let options = computed.into_iter().map(|(_, v)| v).collect();
                            return ExecOutcome::NeedChoice { node: i, options, trace };
                        } else {
                            return ExecOutcome::Failed {
                                error: format!("choice node {i} has no computed alternatives"),
                                at_node: Some(i),
                                trace,
                            };
                        }
                    }
                }

                PlanNode::Project { of, property, ty: _ } => {
                    let struct_val = state.values[*of].clone().unwrap_or(Value::Null);
                    let field_val = match struct_val {
                        Value::Struct { ref fields, .. } => {
                            fields.get(property).cloned().unwrap_or(Value::Null)
                        }
                        _ => {
                            return ExecOutcome::Failed {
                                error: format!("project on non-struct at node {of}"),
                                at_node: Some(i),
                                trace,
                            };
                        }
                    };
                    state.values[i] = Some(field_val);
                }
            }
        }

        let value = state.values.get(plan.goal).and_then(|v| v.clone()).unwrap_or(Value::Null);
        ExecOutcome::Done { value, trace }
    }

    fn gather_args(&self, state: &ExecState, arg_indices: &[usize]) -> Vec<Value> {
        arg_indices
            .iter()
            .map(|&idx| state.values.get(idx).and_then(|v| v.clone()).unwrap_or(Value::Null))
            .collect()
    }
}
