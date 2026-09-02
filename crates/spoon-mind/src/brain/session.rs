//! Per-connection session state for the brain turn loop.

use spoon_core::types::*;

use crate::discourse::DiscourseState;
use crate::dispatch::Present;

/// What the brain is waiting for from the user on the next turn.
#[derive(Debug)]
pub enum Pending {
    /// Executor paused - waiting for user input, permission, or choice.
    Exec {
        intent: Intent,
        plan: Plan,
        state: crate::plan::ExecState,
        kind: PendingExecKind,
        /// How to present the value once the plan completes.
        present: Present,
    },
    /// We asked for examples of an unknown verb.
    UnknownVerb {
        verb: String,
        sce: String,
        signals: Vec<Signal>,
    },
}

#[derive(Debug, Clone)]
pub enum PendingExecKind {
    Input { node: usize, ty: Type },
    Permission { node: usize },
    Choice { node: usize },
}

/// The last turn that stored facts, so "User means Mary." can retarget them.
#[derive(Debug, Clone)]
pub struct LastAssert {
    /// The Named entity the user talked about (never User/Assistant).
    pub target: String,
    /// Facts stored this turn have ids above this (facts are never deleted).
    pub facts_before: i64,
    pub at: i64,
    pub sce: String,
    pub ears_path: EarsPath,
}

/// Per-session state, shared across turns.
pub struct Session {
    pub discourse: DiscourseState,
    pub pending: Option<Pending>,
    pub prior_turns: Vec<(String, String)>,
    pub last_assert: Option<LastAssert>,
}

impl Session {
    pub fn new() -> Self {
        Session {
            discourse: DiscourseState::default(),
            pending: None,
            prior_turns: Vec::new(),
            last_assert: None,
        }
    }
}
