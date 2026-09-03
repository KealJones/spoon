//! Intent and Plan: what the planner consumes and produces.
//!
//! An `Intent` is a Viv-style goal plus signals (typed known values). The
//! planner returns a `Plan`: a DAG of action nodes connecting signals to the
//! goal, with placeholders where a required input has no producer (those
//! become prompts to the user at execution time).

use serde::{Deserialize, Serialize};

use super::can::{ActionId, Effect};
use super::value::{Type, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "goal", rename_all = "snake_case")]
pub enum Goal {
    /// Produce a value of this type.
    Type { ty: Type },
    /// Run this action (its output is the result).
    Action { action: ActionId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub ty: Type,
    pub value: Value,
    /// Optional hint of which input name this should bind to.
    #[serde(default)]
    pub name_hint: Option<String>,
    /// Discourse variable this came from, if any.
    #[serde(default)]
    pub var: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub goal: Goal,
    pub signals: Vec<Signal>,
    /// Actions the utterance explicitly named that must appear in the plan.
    #[serde(default)]
    pub routes: Vec<ActionId>,
    /// Source clause text.
    #[serde(default)]
    pub sce: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum PlanNode {
    /// A known value entering the plan.
    Signal { value: Value, ty: Type },
    /// Run an action. `args` are node indices in the plan, one per input,
    /// in input order.
    Action { action: ActionId, args: Vec<usize> },
    /// Missing required input: prompt the user for a value of `ty`.
    Placeholder { ty: Type, input_name: String, for_action: ActionId },
    /// Apply `action` to each element of the list at node `over`, with the
    /// other args fixed. Inserted when a One input receives a Many value.
    Map { action: ActionId, over: usize, other_args: Vec<Option<usize>> },
    /// Several producers were possible; runtime picks one (or asks).
    Choice { alternatives: Vec<usize>, ty: Type },
    /// Read a property of a struct-valued node.
    Project { of: usize, property: String, ty: Type },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub nodes: Vec<PlanNode>,
    /// Index of the node whose value is the result.
    pub goal: usize,
    /// Sum of step costs; fewer, more-used actions are cheaper.
    pub cost: f64,
    /// Highest effect level of any action in the plan.
    pub effect: Effect,
}

impl Plan {
    pub fn action_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| matches!(n, PlanNode::Action { .. } | PlanNode::Map { .. }))
            .count()
    }
    pub fn placeholders(&self) -> Vec<(usize, &Type, &str)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                PlanNode::Placeholder { ty, input_name, .. } => Some((i, ty, input_name.as_str())),
                _ => None,
            })
            .collect()
    }
    pub fn actions(&self) -> Vec<ActionId> {
        self.nodes
            .iter()
            .filter_map(|n| match n {
                PlanNode::Action { action, .. } | PlanNode::Map { action, .. } => Some(action.clone()),
                _ => None,
            })
            .collect()
    }
    /// Topological order of node indices (nodes reference only lower indices
    /// by construction, but we do not rely on that).
    pub fn topo(&self) -> Vec<usize> {
        let n = self.nodes.len();
        let mut indeg = vec![0usize; n];
        let mut out: Vec<Vec<usize>> = vec![vec![]; n];
        for (i, node) in self.nodes.iter().enumerate() {
            for d in node.deps() {
                indeg[i] += 1;
                out[d].push(i);
            }
        }
        let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop() {
            order.push(i);
            for &j in &out[i] {
                indeg[j] -= 1;
                if indeg[j] == 0 {
                    ready.push(j);
                }
            }
        }
        order
    }
}

impl PlanNode {
    pub fn deps(&self) -> Vec<usize> {
        match self {
            PlanNode::Signal { .. } | PlanNode::Placeholder { .. } => vec![],
            PlanNode::Action { args, .. } => args.clone(),
            PlanNode::Map { over, other_args, .. } => {
                let mut v = vec![*over];
                v.extend(other_args.iter().flatten().copied());
                v
            }
            PlanNode::Choice { alternatives, .. } => alternatives.clone(),
            PlanNode::Project { of, .. } => vec![*of],
        }
    }
}

/// Result of trying to plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PlanOutcome {
    Plan { plan: Plan },
    /// No action produces the goal type from anything reachable.
    NoProducer { goal: Goal },
    /// A verb in the utterance is not in the CAN.
    UnknownAction { verb: String },
    /// Budget exhausted.
    Timeout,
}
