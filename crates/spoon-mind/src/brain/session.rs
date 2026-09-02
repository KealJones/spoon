//! Per-connection session state for the brain turn loop.

use spoon_core::types::*;

use crate::discourse::DiscourseState;

/// What the brain is waiting for from the user on the next turn.
#[derive(Debug)]
pub enum Pending {
    /// Executor paused - waiting for user input, permission, or choice.
    Exec {
        intent: Intent,
        plan: Plan,
        state: crate::plan::ExecState,
        kind: PendingExecKind,
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
    Input { node: usize, input_name: String, ty: Type },
    Permission { node: usize },
    Choice { node: usize },
}

/// Per-session state, shared across turns.
pub struct Session {
    pub discourse: DiscourseState,
    pub pending: Option<Pending>,
    pub prior_turns: Vec<(String, String)>,
}

impl Session {
    pub fn new() -> Self {
        Session {
            discourse: DiscourseState::default(),
            pending: None,
            prior_turns: Vec::new(),
        }
    }
}
