//! The enumeration itself.
//!
//! Bottom-up, smallest first, with a bank of observationally distinct
//! candidates grouped by node count. See the module docs on `synth` for why it
//! is shaped this way.

use std::collections::HashSet;
use std::time::Instant;

use spoon_concept::{Concept, ContentId, pre_order, substitute_positional};
use spoon_eval::{Budget, Evaluator, NativeRegistry, Outcome, PermissionMode};
use spoon_store::Store;

use super::grammar::{Operator, Problem, terminals};
use super::{SynthBudget, SynthLimit, SynthOutcome, SynthStats};

/// Ceiling on `max_size`, so an absurd budget cannot ask for a bank with
/// billions of levels before the search has evaluated anything.
const MAX_SIZE_CEILING: usize = 32;

/// Candidates between wall-clock checks. Reading the clock per candidate is
/// measurable when candidates are cheap.
const TIME_CHECK_STRIDE: u64 = 128;

/// A candidate that survived, with the one bit the next level needs to know
/// about it.
struct Entry {
    body: Concept,
    /// Whether the body mentions any argument. A compound built entirely from
    /// hole-free parts is a constant, and constants are already terminals.
    has_hole: bool,
}

/// What to do after considering one candidate.
enum Step {
    Continue,
    Found(Concept),
    Budget(SynthLimit),
}

pub(super) struct Search<'a> {
    store: &'a Store,
    registry: &'a NativeRegistry,
    problem: &'a Problem,
    budget: SynthBudget,
    max_size: usize,
    /// `bank[n]` holds the observationally distinct candidates of exactly `n`
    /// nodes.
    bank: Vec<Vec<Entry>>,
    /// Output signatures already seen. This is the whole pruning mechanism: a
    /// signature that is already present means an earlier, no-larger candidate
    /// behaves identically on every example, so this one and everything built
    /// on it would be redundant.
    seen: HashSet<Vec<Option<ContentId>>>,
    stats: SynthStats,
    started: Instant,
    since_time_check: u64,
}

impl<'a> Search<'a> {
    pub(super) fn new(
        store: &'a Store,
        registry: &'a NativeRegistry,
        problem: &'a Problem,
        budget: SynthBudget,
    ) -> Self {
        let max_size = budget.max_size.min(MAX_SIZE_CEILING);
        Search {
            store,
            registry,
            problem,
            budget,
            max_size,
            bank: (0..=max_size).map(|_| Vec::new()).collect(),
            seen: HashSet::new(),
            stats: SynthStats::default(),
            started: Instant::now(),
            since_time_check: 0,
        }
    }

    pub(super) fn run(mut self, operators: &[Operator]) -> SynthOutcome {
        for terminal in terminals(self.problem) {
            match self.consider(terminal, 1) {
                Step::Continue => {}
                Step::Found(body) => return self.found(body),
                Step::Budget(limit) => return self.out_of_budget(limit),
            }
        }

        for size in 2..=self.max_size {
            for operator in operators {
                // A call needs its head, itself, and one node per argument, so
                // anything wider than this cannot fit in `size`.
                if operator.arity + 2 > size {
                    continue;
                }
                for shape in compositions(size - 2, operator.arity) {
                    match self.build(operator, &shape, size) {
                        Step::Continue => {}
                        Step::Found(body) => return self.found(body),
                        Step::Budget(limit) => return self.out_of_budget(limit),
                    }
                }
            }
        }

        self.stats.millis = self.elapsed_millis();
        SynthOutcome::Exhausted { stats: self.stats }
    }

    /// Every way to fill one operator with arguments of the given sizes.
    ///
    /// Walked as an odometer over bank positions so the order is fixed, which
    /// keeps two runs over the same store byte-identical.
    fn build(&mut self, operator: &Operator, shape: &[usize], size: usize) -> Step {
        let lengths: Vec<usize> = shape.iter().map(|s| self.bank[*s].len()).collect();
        if lengths.contains(&0) {
            return Step::Continue;
        }

        let mut cursor = vec![0usize; shape.len()];
        loop {
            let mut args: Vec<Concept> = Vec::with_capacity(shape.len());
            let mut has_hole = false;
            for (position, &bank_size) in shape.iter().enumerate() {
                let entry = &self.bank[bank_size][cursor[position]];
                has_hole |= entry.has_hole;
                args.push(entry.body.clone());
            }

            if has_hole {
                let body = Concept::apply(operator.head.clone(), args);
                match self.consider(body, size) {
                    Step::Continue => {}
                    other => return other,
                }
            }

            // Odometer, least significant position last.
            let mut position = cursor.len();
            loop {
                if position == 0 {
                    return Step::Continue;
                }
                position -= 1;
                cursor[position] += 1;
                if cursor[position] < lengths[position] {
                    break;
                }
                cursor[position] = 0;
            }
        }
    }

    /// Run one candidate on every example and decide what becomes of it.
    fn consider(&mut self, body: Concept, size: usize) -> Step {
        if let Some(limit) = self.charge() {
            return Step::Budget(limit);
        }

        let outputs = self.run_examples(&body);

        // A candidate that produces nothing anywhere is not a building block:
        // whatever is wrapped around it will fail the same way. One that works
        // on some examples and not others is kept, because a lazy realization
        // like `if` can hold a branch that only makes sense sometimes.
        if outputs.iter().all(Option::is_none) {
            self.stats.discarded += 1;
            return Step::Continue;
        }

        if self.satisfies(&outputs) {
            return Step::Found(body);
        }

        let signature: Vec<Option<ContentId>> = outputs
            .iter()
            .map(|o| o.as_ref().map(Concept::content_id))
            .collect();
        if !self.seen.insert(signature) {
            self.stats.pruned += 1;
            return Step::Continue;
        }

        self.stats.kept += 1;
        let has_hole = pre_order(&body).any(|node| node.is_hole());
        self.bank[size].push(Entry { body, has_hole });
        Step::Continue
    }

    /// Evaluate `body` against each example's inputs.
    ///
    /// One evaluator serves all the examples for a single candidate so the
    /// per-head realization lookups are paid once, and it is dropped
    /// afterwards so the trace it accumulates cannot grow across the search.
    ///
    /// The whole substituted term is evaluated rather than combining the
    /// cached outputs of the candidate's parts. Combining cached outputs is
    /// much faster and quietly wrong for lazy realizations: a body like
    /// `if<is-zero<?1>, 0, div<?0, ?1>>` needs a branch that fails on some
    /// example to still count, and only real evaluation gets that right.
    fn run_examples(&self, body: &Concept) -> Vec<Option<Concept>> {
        let mut evaluator = Evaluator::new(self.store, self.registry)
            .with_budget(candidate_budget())
            .with_permission(PermissionMode::AlwaysAsk);

        self.problem
            .inputs
            .iter()
            .map(|inputs| {
                let term = substitute_positional(body, inputs);
                match evaluator.evaluate(&term) {
                    Outcome::Value(value) => Some(value),
                    _ => None,
                }
            })
            .collect()
    }

    fn satisfies(&self, outputs: &[Option<Concept>]) -> bool {
        outputs.len() == self.problem.expected.len()
            && outputs
                .iter()
                .zip(self.problem.expected.iter())
                .all(|(got, want)| got.as_ref().is_some_and(|c| c.content_id() == *want))
    }

    /// Charge one candidate against the budget, before doing the work rather
    /// than after, so the reported node count never exceeds what was allowed.
    fn charge(&mut self) -> Option<SynthLimit> {
        if self.stats.nodes >= self.budget.max_nodes {
            return Some(SynthLimit::Nodes);
        }
        self.since_time_check += 1;
        if self.since_time_check >= TIME_CHECK_STRIDE {
            self.since_time_check = 0;
            if self.elapsed_millis() > self.budget.max_millis {
                return Some(SynthLimit::Time);
            }
        }
        self.stats.nodes += 1;
        None
    }

    fn elapsed_millis(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn found(mut self, body: Concept) -> SynthOutcome {
        self.stats.millis = self.elapsed_millis();
        SynthOutcome::Found {
            size: body.size(),
            body,
            stats: self.stats,
        }
    }

    fn out_of_budget(mut self, limit: SynthLimit) -> SynthOutcome {
        self.stats.millis = self.elapsed_millis();
        SynthOutcome::OutOfBudget {
            limit,
            stats: self.stats,
        }
    }
}

/// What one candidate is allowed to cost.
///
/// Deliberately small and deterministic. A single guess that loops or fans out
/// must not be able to spend the whole search, and exploration has to be off or
/// the same candidate could score differently on two examples and make the
/// search irreproducible.
fn candidate_budget() -> Budget {
    Budget::deterministic()
        .with_nodes(512)
        .with_depth(32)
        .with_millis(250)
}

/// Every ordered way to split `total` nodes among `parts` arguments, each
/// getting at least one. Lexicographic, so search order is stable.
fn compositions(total: usize, parts: usize) -> Vec<Vec<usize>> {
    if parts == 0 {
        return if total == 0 {
            vec![Vec::new()]
        } else {
            Vec::new()
        };
    }
    if total < parts {
        return Vec::new();
    }
    if parts == 1 {
        return vec![vec![total]];
    }
    let mut out = Vec::new();
    for first in 1..=(total - parts + 1) {
        for rest in compositions(total - first, parts - 1) {
            let mut shape = Vec::with_capacity(parts);
            shape.push(first);
            shape.extend(rest);
            out.push(shape);
        }
    }
    out
}
