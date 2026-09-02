//! Backward-chaining AND/OR planner over the Concept Action Network.
//!
//! ## Signal consumption
//! Each signal is consumed at most once. If two required inputs need the same
//! type and only one signal is available, the second becomes a Placeholder.
//!
//! ## Optional inputs
//! A skipped optional (no signal, no configured default) is represented as
//! `PlanNode::Signal { value: Value::Null, ty }`. The executor receives Null
//! for those inputs. An optional WITH a configured default uses that default.
//!
//! ## Cardinality / Map
//! When a One-cardinality required input has no scalar producer but a
//! `List[T]` signal IS available (where T fits the input type), the planner
//! inserts `PlanNode::Map` and the result becomes `List[output]`.
//!
//! When a Many (expects list) input receives a single scalar, no special node
//! is inserted. The executor promotes scalar values to one-element lists for
//! Many inputs at runtime.

use std::collections::HashSet;
use std::time::Instant;

use spoon_core::{
    Action, ActionId, Can, Cardinality, ConceptId, ConceptKind, Effect, Goal, Intent, Plan,
    PlanNode, PlanOutcome, Type, Value,
};

use super::cost::{action_cost, MAP_PENALTY, PLACEHOLDER_COST};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Budget controls for a single planning call.
pub struct PlanBudget {
    /// Maximum number of action-expansion attempts before giving up.
    pub max_expansions: usize,
    /// Wall-clock ceiling in milliseconds.
    pub max_millis: u64,
    /// Maximum recursion depth per branch.
    pub max_depth: usize,
}

impl Default for PlanBudget {
    fn default() -> Self {
        Self { max_expansions: 5000, max_millis: 200, max_depth: 6 }
    }
}

pub struct Planner<'a> {
    can: &'a Can,
}

impl<'a> Planner<'a> {
    pub fn new(can: &'a Can) -> Self {
        Self { can }
    }

    /// Return the single best plan, or an informative failure variant.
    pub fn plan(&self, intent: &Intent, budget: &PlanBudget) -> PlanOutcome {
        let mut ctx = PlanCtx::new(self.can, intent, budget);
        ctx.run(&intent.goal)
    }

    /// Return up to `k` distinct plans, best first (lowest cost, then fewest
    /// nodes). Used for parse ranking and choice points.
    ///
    /// For `Goal::Type`, each top-level producer gets its own isolated attempt.
    /// Plans are deduplicated by top-level action id.
    pub fn plans(&self, intent: &Intent, k: usize, budget: &PlanBudget) -> Vec<Plan> {
        let ty = match &intent.goal {
            Goal::Type { ty } => ty,
            Goal::Action { .. } => {
                return match self.plan(intent, budget) {
                    PlanOutcome::Plan { plan } => vec![plan],
                    _ => vec![],
                };
            }
        };

        let producers = sorted_producers(self.can, ty);
        let mut results: Vec<Plan> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for action_id in &producers {
            if results.len() >= k {
                break;
            }
            if seen.contains(&action_id.0) {
                continue;
            }
            let action = match self.can.action(action_id) {
                Some(a) => a.clone(),
                None => continue,
            };
            let mut ctx = PlanCtx::new(self.can, intent, budget);
            ctx.action_stack.insert(action_id.clone());
            ctx.expansions += 1;
            if let Some((cost, idx)) = ctx.try_expand_action(&action, 0) {
                seen.insert(action_id.0.clone());
                results.push(ctx.build_plan(idx, cost));
            }
        }

        results.sort_by(|a, b| {
            a.cost
                .partial_cmp(&b.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.nodes.len().cmp(&b.nodes.len()))
        });
        results.truncate(k);
        results
    }

    /// Cheap feasibility probe: is there ANY plan with at most `max_placeholders`
    /// missing inputs? Used by the ears to rank parses.
    pub fn feasible(&self, intent: &Intent, max_placeholders: usize) -> bool {
        match self.plan(intent, &PlanBudget::default()) {
            PlanOutcome::Plan { plan } => plan.placeholders().len() <= max_placeholders,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sorted_producers(can: &Can, ty: &Type) -> Vec<ActionId> {
    let mut p: Vec<_> = can
        .producers_of(ty)
        .into_iter()
        .map(|a| {
            let cost = action_cost(can, a);
            // Break ties by total input count: fewer inputs = simpler action.
            let inputs = a.inputs.len();
            (cost, inputs, a.id.clone())
        })
        .collect();
    p.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    p.into_iter().map(|(_, _, id)| id).collect()
}

// ---------------------------------------------------------------------------
// Internal planning context
// ---------------------------------------------------------------------------

struct PlanCtx<'a, 'b> {
    can: &'a Can,
    budget: &'b PlanBudget,
    nodes: Vec<PlanNode>,
    /// Signal nodes that exist at the start and are never truncated away.
    initial_nodes: Vec<PlanNode>,
    /// Consumable signal pool. Each entry mirrors an initial signal node.
    /// Set to None once the signal has been used as an action input.
    sig_pool: Vec<Option<(usize, Type)>>,
    initial_sigs: Vec<Option<(usize, Type)>>,
    action_stack: HashSet<ActionId>,
    expansions: usize,
    timed_out: bool,
    start: Instant,
}

impl<'a, 'b> PlanCtx<'a, 'b> {
    fn new(can: &'a Can, intent: &Intent, budget: &'b PlanBudget) -> Self {
        let mut nodes: Vec<PlanNode> = Vec::new();
        let mut sig_pool: Vec<Option<(usize, Type)>> = Vec::new();
        for sig in &intent.signals {
            let idx = nodes.len();
            nodes.push(PlanNode::Signal { value: sig.value.clone(), ty: sig.ty.clone() });
            sig_pool.push(Some((idx, sig.ty.clone())));
        }
        let initial_nodes = nodes.clone();
        let initial_sigs = sig_pool.clone();
        Self {
            can,
            budget,
            nodes,
            initial_nodes,
            sig_pool,
            initial_sigs,
            action_stack: HashSet::new(),
            expansions: 0,
            timed_out: false,
            start: Instant::now(),
        }
    }

    fn reset(&mut self) {
        self.nodes = self.initial_nodes.clone();
        self.sig_pool = self.initial_sigs.clone();
        self.action_stack.clear();
    }

    fn check_budget(&mut self) -> bool {
        if self.timed_out {
            return true;
        }
        if self.expansions >= self.budget.max_expansions
            || self.start.elapsed().as_millis() as u64 >= self.budget.max_millis
        {
            self.timed_out = true;
        }
        self.timed_out
    }

    fn run(&mut self, goal: &Goal) -> PlanOutcome {
        match goal {
            Goal::Type { ty } => match self.expand_goal_type(ty) {
                Some((cost, idx)) => PlanOutcome::Plan { plan: self.build_plan(idx, cost) },
                None if self.timed_out => PlanOutcome::Timeout,
                None => PlanOutcome::NoProducer { goal: goal.clone() },
            },
            Goal::Action { action } => {
                let def = match self.can.action(action) {
                    Some(a) => a.clone(),
                    None => return PlanOutcome::NoProducer { goal: goal.clone() },
                };
                match self.try_expand_action(&def, 0) {
                    Some((cost, idx)) => PlanOutcome::Plan { plan: self.build_plan(idx, cost) },
                    None if self.timed_out => PlanOutcome::Timeout,
                    None => PlanOutcome::NoProducer { goal: goal.clone() },
                }
            }
        }
    }

    /// Top-level expansion: try every producer and struct projection, keep best
    /// by (cost, node_count). Signals are only used as a last resort.
    fn expand_goal_type(&mut self, ty: &Type) -> Option<(f64, usize)> {
        type Snap = (f64, usize, Vec<PlanNode>, Vec<Option<(usize, Type)>>);
        let mut candidates: Vec<Snap> = Vec::new();

        let producers = sorted_producers(self.can, ty);

        for action_id in &producers {
            if self.check_budget() {
                break;
            }
            self.reset();
            let action = match self.can.action(action_id) {
                Some(a) => a.clone(),
                None => continue,
            };
            self.action_stack.insert(action_id.clone());
            self.expansions += 1;
            if let Some((cost, idx)) = self.try_expand_action(&action, 0) {
                candidates.push((cost, idx, self.nodes.clone(), self.sig_pool.clone()));
            }
            self.action_stack.remove(action_id);
        }

        // Struct projection from producible structs
        if !self.check_budget() {
            self.reset();
            if let Some((cost, idx)) = self.try_produced_projection(ty, 0) {
                candidates.push((cost, idx, self.nodes.clone(), self.sig_pool.clone()));
            }
        }

        // Signal projection (struct signal -> project property)
        if !self.check_budget() {
            self.reset();
            if let Some(r) = self.try_signal_projection(ty) {
                candidates.push((r.0, r.1, self.nodes.clone(), self.sig_pool.clone()));
            }
        }

        candidates.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.2.len().cmp(&b.2.len()))
        });

        if let Some((cost, idx, nodes, sigs)) = candidates.into_iter().next() {
            self.nodes = nodes;
            self.sig_pool = sigs;
            return Some((cost, idx));
        }

        // Last resort: return an existing signal directly
        self.reset();
        self.consume_signal(ty)
    }

    /// Recursive input expansion: signal first, then producers (first success).
    fn expand_type(&mut self, ty: &Type, depth: usize) -> Option<(f64, usize)> {
        if self.check_budget() {
            return None;
        }

        // Direct signal consumption
        if let Some(r) = self.consume_signal(ty) {
            return Some(r);
        }

        // Project from an available struct signal
        if let Some(r) = self.try_signal_projection(ty) {
            return Some(r);
        }

        if depth >= self.budget.max_depth {
            return None;
        }

        // Try each producer in cost order; return on first success. Zero-input
        // producers (now, now-ms) are goals, not fillers: a missing input must
        // come from what the user said or become a Placeholder, never from
        // "whatever value the kernel can conjure" (concat "ab" + timestamp).
        let producers = sorted_producers(self.can, ty);
        for action_id in &producers {
            if self.action_stack.contains(action_id) {
                continue; // cycle guard
            }
            if self.can.action(action_id).is_some_and(|a| a.inputs.is_empty()) {
                continue;
            }
            if self.check_budget() {
                break;
            }
            let snap_nodes = self.nodes.len();
            let snap_sigs = self.sig_pool.clone();
            let action = match self.can.action(action_id) {
                Some(a) => a.clone(),
                None => continue,
            };
            self.action_stack.insert(action_id.clone());
            self.expansions += 1;
            if let Some(result) = self.try_expand_action(&action, depth) {
                self.action_stack.remove(action_id);
                return Some(result);
            }
            self.action_stack.remove(action_id);
            self.nodes.truncate(snap_nodes);
            self.sig_pool = snap_sigs;
        }

        // Struct projection via a producible struct
        self.try_produced_projection(ty, depth)
    }

    /// Expand all inputs for `action` and build the appropriate plan node.
    /// Returns None only if a required input is impossible AND placeholder
    /// insertion is NOT attempted here (callers decide). In practice, required
    /// inputs always succeed: failing scalar expansion falls back to Placeholder
    /// (or Map if a list signal is available for a One input).
    fn try_expand_action(
        &mut self,
        action: &Action,
        depth: usize,
    ) -> Option<(f64, usize)> {
        let mut args: Vec<usize> = Vec::new();
        let mut total_cost = action_cost(self.can, action);
        let mut map_over_pos: Option<usize> = None; // input-position index for Map

        for (i, input) in action.inputs.iter().enumerate() {
            if input.required {
                if input.max == Cardinality::One {
                    // Try to expand; if the expansion is more expensive than a
                    // Placeholder (cost >= PLACEHOLDER_COST) we prefer Placeholder.
                    // This prevents chains like wx.forecast->project from beating
                    // the simpler "ask the user" path.
                    let snap_nodes = self.nodes.len();
                    let snap_sigs = self.sig_pool.clone();
                    let expanded = self.expand_type(&input.ty, depth + 1);
                    let use_expansion = match &expanded {
                        Some((c, _)) if *c < PLACEHOLDER_COST => true,
                        Some(_) => {
                            // Rollback - expansion is too costly, prefer Placeholder
                            self.nodes.truncate(snap_nodes);
                            self.sig_pool = snap_sigs;
                            false
                        }
                        None => false,
                    };

                    if use_expansion {
                        let (c, node_idx) = expanded.unwrap();
                        total_cost += c;
                        args.push(node_idx);
                    } else {
                        // If a List[T] signal is available, use Map
                        if map_over_pos.is_none() {
                            if let Some((c, list_idx)) =
                                self.consume_list_signal(&input.ty)
                            {
                                total_cost += c + MAP_PENALTY;
                                map_over_pos = Some(i);
                                args.push(list_idx);
                                continue;
                            }
                        }
                        // Placeholder
                        let ph = self.nodes.len();
                        self.nodes.push(PlanNode::Placeholder {
                            ty: input.ty.clone(),
                            input_name: input.name.clone(),
                            for_action: action.id.clone(),
                        });
                        total_cost += PLACEHOLDER_COST;
                        args.push(ph);
                    }
                } else {
                    // Many: prefer a list signal, fall back to single (executor promotes)
                    let list_ty = Type::list(input.ty.clone());
                    let node_idx = match self.expand_type(&list_ty, depth + 1) {
                        Some((c, idx)) => {
                            total_cost += c;
                            idx
                        }
                        None => match self.expand_type(&input.ty, depth + 1) {
                            Some((c, idx)) => {
                                total_cost += c;
                                idx
                            }
                            None => {
                                let ph = self.nodes.len();
                                self.nodes.push(PlanNode::Placeholder {
                                    ty: list_ty,
                                    input_name: input.name.clone(),
                                    for_action: action.id.clone(),
                                });
                                total_cost += PLACEHOLDER_COST;
                                ph
                            }
                        },
                    };
                    args.push(node_idx);
                }
            } else {
                // Optional: try to find a value; fall back to default or Null
                let node_idx = match self.expand_type(&input.ty, depth + 1) {
                    Some((c, idx)) => {
                        total_cost += c;
                        idx
                    }
                    None => {
                        let fill = input.default.clone().unwrap_or(Value::Null);
                        let sig = self.nodes.len();
                        self.nodes.push(PlanNode::Signal {
                            value: fill,
                            ty: input.ty.clone(),
                        });
                        sig
                    }
                };
                args.push(node_idx);
            }
        }

        if let Some(over_pos) = map_over_pos {
            let over_node = args[over_pos];
            let other_args: Vec<Option<usize>> = args
                .iter()
                .enumerate()
                .map(|(i, &n)| if i == over_pos { None } else { Some(n) })
                .collect();
            let map_idx = self.nodes.len();
            self.nodes.push(PlanNode::Map {
                action: action.id.clone(),
                over: over_node,
                other_args,
            });
            Some((total_cost, map_idx))
        } else {
            let action_idx = self.nodes.len();
            self.nodes.push(PlanNode::Action { action: action.id.clone(), args });
            Some((total_cost, action_idx))
        }
    }

    /// Consume the first available signal whose type is assignable to `ty`.
    fn consume_signal(&mut self, ty: &Type) -> Option<(f64, usize)> {
        let can = self.can;
        let pos = self
            .sig_pool
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|(_, st)| can.assignable(ty, st)))?;
        let (node_idx, _) = self.sig_pool[pos].take()?;
        Some((0.0, node_idx))
    }

    /// Consume the first available signal of type `List[T]` where T fits `elem_ty`.
    fn consume_list_signal(&mut self, elem_ty: &Type) -> Option<(f64, usize)> {
        let can = self.can;
        let pos = self.sig_pool.iter().position(|slot| {
            if let Some((_, Type::List(inner))) = slot {
                can.assignable(elem_ty, inner)
            } else {
                false
            }
        })?;
        let (node_idx, _) = self.sig_pool[pos].take()?;
        Some((0.0, node_idx))
    }

    /// Project from an available struct signal that has a projectable property
    /// of the needed type. Consumes the signal.
    fn try_signal_projection(&mut self, ty: &Type) -> Option<(f64, usize)> {
        let can = self.can;
        let struct_sigs: Vec<(usize, ConceptId)> = self
            .sig_pool
            .iter()
            .filter_map(|slot| {
                if let Some((ni, Type::Concept(cid))) = slot {
                    Some((*ni, cid.clone()))
                } else {
                    None
                }
            })
            .collect();

        for (node_idx, cid) in struct_sigs {
            let prop_name: Option<String> = can.concept(&cid).and_then(|c| {
                if let ConceptKind::Structure { properties } = &c.kind {
                    properties
                        .iter()
                        .find(|p| p.projectable && can.assignable(ty, &p.ty))
                        .map(|p| p.name.clone())
                } else {
                    None
                }
            });

            if let Some(name) = prop_name {
                let pool_pos = self
                    .sig_pool
                    .iter()
                    .position(|s| matches!(s, Some((ni, _)) if *ni == node_idx))?;
                self.sig_pool[pool_pos] = None;
                let proj = self.nodes.len();
                self.nodes.push(PlanNode::Project { of: node_idx, property: name, ty: ty.clone() });
                return Some((0.0, proj));
            }
        }
        None
    }

    /// Find a struct concept with a property of `ty`, produce the struct, then project.
    fn try_produced_projection(&mut self, ty: &Type, depth: usize) -> Option<(f64, usize)> {
        if depth >= self.budget.max_depth {
            return None;
        }
        let can = self.can;
        let candidates: Vec<(ConceptId, String)> = can
            .concepts()
            .filter_map(|c| {
                if let ConceptKind::Structure { properties } = &c.kind {
                    properties
                        .iter()
                        .find(|p| p.projectable && can.assignable(ty, &p.ty))
                        .map(|p| (c.id.clone(), p.name.clone()))
                } else {
                    None
                }
            })
            .collect();

        for (cid, prop_name) in candidates {
            let concept_ty = Type::Concept(cid);
            let snap_nodes = self.nodes.len();
            let snap_sigs = self.sig_pool.clone();
            if let Some((struct_cost, struct_idx)) = self.expand_type(&concept_ty, depth + 1) {
                let proj = self.nodes.len();
                self.nodes.push(PlanNode::Project {
                    of: struct_idx,
                    property: prop_name,
                    ty: ty.clone(),
                });
                return Some((struct_cost, proj));
            }
            self.nodes.truncate(snap_nodes);
            self.sig_pool = snap_sigs;
        }
        None
    }

    fn build_plan(&self, goal: usize, cost: f64) -> Plan {
        let effect = self.nodes.iter().fold(Effect::Pure, |acc, node| match node {
            PlanNode::Action { action, .. } | PlanNode::Map { action, .. } => {
                self.can.action(action).map(|a| acc.max(a.effect)).unwrap_or(acc)
            }
            _ => acc,
        });
        Plan { nodes: self.nodes.clone(), goal, cost, effect }
    }
}
