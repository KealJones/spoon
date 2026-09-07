//! Building a capability from worked examples.
//!
//! A learned skill is not code. It is a concept with holes in it, stored as a
//! `Composed` realization, whose holes bind positionally to the arguments of
//! the call being realized. `double` is `add<?0, ?0>`. Nothing downstream can
//! tell a synthesized body from one Spoon was born with, because there is
//! nothing to tell apart: both are concepts the evaluator reduces the same way.
//!
//! # How the search works
//!
//! Bottom-up enumeration by cost, smallest first, with observational
//! equivalence pruning. A bank holds the surviving candidate bodies grouped by
//! node count. Level 1 is the terminals: one hole per argument, every atomic
//! value that appears in the examples, and a tiny pool of stock values. Each
//! later level builds compounds out of earlier levels: a call of arity `k`
//! whose arguments have sizes summing to `n - 2` has size `n`, because a
//! compound costs its head plus itself plus its arguments.
//!
//! Every candidate is run on every example input. Two candidates that produce
//! the same outputs on every example are the same program as far as the
//! examples can tell, so the second one is discarded along with the entire
//! subtree of programs that would have been built on it. That is the pruning
//! that makes this tractable: without it the bank grows as fast as the grammar
//! does, and the grammar here is roughly a hundred concepts wide.
//!
//! Enumerating smallest-first is also what gives "prefer the smaller body" for
//! free. The first candidate that satisfies every example is returned, and no
//! larger candidate has been built yet, so the answer is minimal by
//! construction (within the pruning described below).
//!
//! # Where the search is deliberately incomplete
//!
//! A compound is only kept if it contains at least one hole. A hole-free
//! subexpression evaluates to the same value on every example, which means it
//! is a constant computed the long way round, and the constants that matter
//! are already terminals. Dropping them keeps each level small enough that the
//! search reaches interesting sizes at all. The cost is real and worth naming:
//! a body whose only route to some needed value is computing it from other
//! constants, like `mul<?0, sub<4, 1>>`, will not be found.
//!
//! # Purity
//!
//! Guessing is not a licence to act. The search runs thousands of programs it
//! has no reason to believe in, so nothing it runs may touch the world. Two
//! independent guards enforce that. Only realizations declaring
//! [`Effect::Pure`] enter the operator table, and every candidate is evaluated
//! under `PermissionMode::AlwaysAsk`, which suspends anything above pure
//! computation instead of running it. A realization that lies about its effect
//! gets caught by the second guard even though it slipped past the first.
//! Evidence is never committed during search either: a guess should not teach
//! Spoon anything.

mod grammar;
mod search;

use chrono::Utc;
use spoon_concept::{Activation, Concept, Effect, Provenance, Realization, RealizationSpec, Tier};
use spoon_eval::NativeRegistry;
use spoon_seat::Spec;
use spoon_store::Store;

use grammar::{Problem, operators};
use search::Search;

/// Limits on a search. Nodes and time, never depth alone: a shallow search that
/// fans out exhausts memory without ever getting deep.
#[derive(Debug, Clone, Copy)]
pub struct SynthBudget {
    pub max_nodes: u64,
    pub max_millis: u64,
    pub max_size: usize,
}

impl Default for SynthBudget {
    fn default() -> Self {
        SynthBudget {
            max_nodes: 400_000,
            max_millis: 4_000,
            max_size: 7,
        }
    }
}

impl SynthBudget {
    pub fn with_nodes(mut self, nodes: u64) -> Self {
        self.max_nodes = nodes;
        self
    }

    pub fn with_millis(mut self, millis: u64) -> Self {
        self.max_millis = millis;
        self
    }

    pub fn with_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }
}

/// Which budget ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthLimit {
    Nodes,
    Time,
}

impl SynthLimit {
    pub fn as_str(self) -> &'static str {
        match self {
            SynthLimit::Nodes => "nodes",
            SynthLimit::Time => "time",
        }
    }
}

/// What the search did, whether or not it found anything.
///
/// `nodes` is every candidate that was built and run, and it always equals
/// `kept + pruned + discarded` plus one for a candidate that turned out to be
/// the answer. The three ways a candidate leaves the search are worth counting
/// separately: `pruned` measures how much work observational equivalence
/// saved, `discarded` measures how much of the grammar is nonsense at these
/// argument types, and `kept` is the part that actually grows the next level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SynthStats {
    /// Candidates built and evaluated on every example.
    pub nodes: u64,
    /// Candidates thrown away for behaving exactly like an earlier candidate.
    pub pruned: u64,
    /// Candidates that failed to produce a value on any example at all.
    pub discarded: u64,
    /// Candidates kept in the bank as building blocks.
    pub kept: u64,
    pub millis: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SynthOutcome {
    /// A body that satisfies every example.
    Found {
        body: Concept,
        size: usize,
        stats: SynthStats,
    },
    /// Searched the whole space within budget and nothing fit. Different from
    /// running out: it means no program of this size exists, so asking for more
    /// examples will not help but a larger budget might.
    Exhausted { stats: SynthStats },
    /// Ran out of budget with candidates left unexplored. The opposite advice
    /// applies: the space was never covered, so raising the limit is the first
    /// thing to try.
    OutOfBudget {
        limit: SynthLimit,
        stats: SynthStats,
    },
    /// The spec itself does not describe a learnable capability. Not a search
    /// result at all, which is why it is not reported as one.
    Malformed { reason: String },
}

impl SynthOutcome {
    pub fn body(&self) -> Option<&Concept> {
        match self {
            SynthOutcome::Found { body, .. } => Some(body),
            _ => None,
        }
    }

    pub fn stats(&self) -> Option<SynthStats> {
        match self {
            SynthOutcome::Found { stats, .. }
            | SynthOutcome::Exhausted { stats }
            | SynthOutcome::OutOfBudget { stats, .. } => Some(*stats),
            SynthOutcome::Malformed { .. } => None,
        }
    }

    pub fn is_found(&self) -> bool {
        matches!(self, SynthOutcome::Found { .. })
    }
}

/// Why learning from a spec could not even be attempted.
///
/// A search that finds nothing is not an error: it is an answer, and the
/// caller gets `Ok(None)`. These are the cases where the request itself does
/// not make sense, or where the store could not be written.
#[derive(Debug, thiserror::Error)]
pub enum SynthError {
    #[error("malformed spec: {0}")]
    Malformed(String),
    #[error(transparent)]
    Store(#[from] spoon_store::StoreError),
}

/// Search for a body that reproduces every example.
///
/// Never panics, whatever the spec contains. A request that cannot be turned
/// into a search comes back as [`SynthOutcome::Malformed`] with the reason
/// spelled out: a spec that constrains nothing, or a brain whose operator
/// table cannot be read.
pub fn synthesize(
    spec: &Spec,
    store: &Store,
    registry: &NativeRegistry,
    budget: SynthBudget,
) -> SynthOutcome {
    let problem = match Problem::from_spec(spec) {
        Ok(problem) => problem,
        Err(reason) => return SynthOutcome::Malformed { reason },
    };
    match search(&problem, store, registry, budget) {
        Ok(outcome) => outcome,
        Err(err) => SynthOutcome::Malformed {
            reason: format!("the operator table could not be read: {err}"),
        },
    }
}

/// Synthesize, and on success store the result so it can be used like anything
/// else Spoon knows.
///
/// Stored at [`Tier::Provisional`]: the Teacher proposed these examples and
/// experience has not weighed in yet. It earns consolidation by being selected
/// and working, the same as any other realization.
///
/// The realization name folds in the body's content id, so re-learning the
/// same body overwrites one row instead of forking the evidence that has
/// accumulated against it.
pub fn learn_from_spec(
    spec: &Spec,
    store: &Store,
    registry: &NativeRegistry,
    budget: SynthBudget,
) -> Result<Option<Realization>, SynthError> {
    let problem = Problem::from_spec(spec).map_err(SynthError::Malformed)?;
    // Finding nothing is an answer, not a failure: the caller gets `Ok(None)`
    // and can decide whether to ask for more examples or a bigger budget.
    let SynthOutcome::Found { body, .. } = search(&problem, store, registry, budget)? else {
        return Ok(None);
    };

    let label = store
        .symbol_name(problem.target)?
        .unwrap_or_else(|| format!("{:016x}", problem.target.as_u64()));

    let realization = Realization {
        target: Concept::symbol(problem.target),
        name: format!("synth-{}-{}", label, body.content_id().short()).into(),
        spec: RealizationSpec::Composed { body },
        // The body was found using pure realizations only, and the evaluator
        // takes the maximum of this and whatever the body actually reaches, so
        // claiming pure cannot smuggle anything in.
        effect: Effect::Pure,
        activation: Activation::new(Utc::now()),
        provenance: Provenance::Synthesized { episode: None },
        tier: Tier::Provisional,
    };
    store.put_realization(&realization)?;
    Ok(Some(realization))
}

/// The search proper, over a spec that has already been validated.
///
/// Both entry points go through here so neither can skip validation and
/// neither can disagree with the other about what a spec means.
fn search(
    problem: &Problem,
    store: &Store,
    registry: &NativeRegistry,
    budget: SynthBudget,
) -> spoon_store::Result<SynthOutcome> {
    // Two examples that disagree about the same input cannot both be satisfied
    // by any body, because evaluation is deterministic. That is a fact about
    // the space rather than about the budget, so it is exhaustion, and proving
    // it costs nothing.
    if problem.is_contradictory() {
        return Ok(SynthOutcome::Exhausted {
            stats: SynthStats::default(),
        });
    }

    let operators = operators(store, registry)?;
    Ok(Search::new(store, registry, problem, budget).run(&operators))
}
