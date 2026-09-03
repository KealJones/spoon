//! Limits on how much work one evaluation may do.

use std::time::{Duration, Instant};

use crate::error::Limit;

/// Nodes, time, and depth together.
///
/// Depth alone is not a budget: a shallow rule that fans out can exhaust memory
/// without ever getting deep, and a deep-but-narrow composition is perfectly
/// reasonable. Every counter is enforced in the evaluator core, so a realization
/// cannot opt out of them.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Realization applications attempted.
    pub max_nodes: u64,
    pub max_millis: u64,
    /// Rewrite nesting. Guards against a rule that never terminates.
    pub max_depth: u32,
    /// Node count of any intermediate concept, so a rewrite cannot balloon into
    /// something that will not fit in memory.
    pub max_result_size: usize,
    /// When set, exploration is disabled and ties break by a stable key.
    /// Benchmarks and regression tests need to reproduce exactly; live
    /// conversation does not.
    pub deterministic: bool,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_nodes: 100_000,
            max_millis: 5_000,
            max_depth: 256,
            max_result_size: 100_000,
            deterministic: false,
        }
    }
}

impl Budget {
    /// A budget suitable for tests: small, and reproducible.
    pub fn deterministic() -> Self {
        Budget {
            deterministic: true,
            ..Budget::default()
        }
    }

    pub fn with_nodes(mut self, nodes: u64) -> Self {
        self.max_nodes = nodes;
        self
    }

    pub fn with_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth;
        self
    }

    pub fn with_millis(mut self, millis: u64) -> Self {
        self.max_millis = millis;
        self
    }
}

/// Live tracking against a [`Budget`].
#[derive(Debug)]
pub struct BudgetState {
    budget: Budget,
    started: Instant,
    nodes: u64,
    /// Checking the clock on every node is measurable overhead, so the time
    /// check runs on a stride. The overshoot is bounded by however long
    /// `TIME_CHECK_STRIDE` applications take.
    since_time_check: u64,
}

const TIME_CHECK_STRIDE: u64 = 64;

impl BudgetState {
    pub fn new(budget: Budget) -> Self {
        BudgetState {
            budget,
            started: Instant::now(),
            nodes: 0,
            since_time_check: 0,
        }
    }

    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    pub fn nodes_used(&self) -> u64 {
        self.nodes
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Charge one realization application. Returns the limit that was breached,
    /// if any.
    pub fn charge_node(&mut self) -> Option<Limit> {
        self.nodes += 1;
        if self.nodes > self.budget.max_nodes {
            return Some(Limit::Nodes);
        }
        self.since_time_check += 1;
        if self.since_time_check >= TIME_CHECK_STRIDE {
            self.since_time_check = 0;
            if self.started.elapsed().as_millis() as u64 > self.budget.max_millis {
                return Some(Limit::Time);
            }
        }
        None
    }

    pub fn check_depth(&self, depth: u32) -> Option<Limit> {
        (depth > self.budget.max_depth).then_some(Limit::Depth)
    }

    pub fn check_size(&self, size: usize) -> Option<Limit> {
        (size > self.budget.max_result_size).then_some(Limit::ResultSize)
    }

    pub fn is_deterministic(&self) -> bool {
        self.budget.deterministic
    }
}
