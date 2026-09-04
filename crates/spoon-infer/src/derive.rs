//! Backward chaining: answering a question from facts nobody wrote down in the
//! shape it was asked.

use std::collections::HashSet;
use std::sync::Arc;

use spoon_concept::{Concept, ContentId, SymbolId, holes, rename_holes};
use spoon_store::Store;

use crate::error::{InferError, Result};
use crate::rule::{RuleIndex, StoredRule};
use crate::unify::{Substitution, unify};

/// Why a goal holds.
///
/// A bare yes is not enough. Credit assignment has to be able to ask which rule
/// produced a wrong answer, and a user asking "how do you know that" deserves
/// the chain rather than an assertion of confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Derivation {
    /// The goal, with everything the derivation learned already substituted in.
    pub goal: Concept,
    /// What the holes in the original goal turned out to be. This is the answer
    /// to a question like "who owns a dog".
    pub substitution: Substitution,
    pub support: Support,
    /// How deep this sits in the chain. Shallower derivations are better
    /// supported, all else equal.
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Support {
    /// Directly asserted, and still live.
    Asserted,
    /// Followed from a rule, with the derivations of everything it needed.
    Rule {
        name: Arc<str>,
        premises: Vec<Derivation>,
    },
}

impl Derivation {
    /// Every rule that participated, outermost first, deduplicated.
    pub fn rules_used(&self) -> Vec<Arc<str>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.collect_rules(&mut out, &mut seen);
        out
    }

    fn collect_rules(&self, out: &mut Vec<Arc<str>>, seen: &mut HashSet<Arc<str>>) {
        if let Support::Rule { name, premises } = &self.support {
            if seen.insert(name.clone()) {
                out.push(name.clone());
            }
            for premise in premises {
                premise.collect_rules(out, seen);
            }
        }
    }

    /// True when nothing but stored facts was needed.
    pub fn is_direct(&self) -> bool {
        matches!(self.support, Support::Asserted)
    }
}

/// Limits on a derivation.
///
/// Backward chaining over a rule set that mentions itself can run forever, and
/// unlike evaluation there is no natural notion of "reduced" to stop at. The
/// budget is the only thing that guarantees termination.
#[derive(Debug, Clone, Copy)]
pub struct DeriveBudget {
    /// Rule applications attempted.
    pub max_steps: u64,
    /// Chain depth.
    pub max_depth: u32,
    /// Distinct answers to collect before stopping.
    pub max_results: usize,
    /// Stored facts examined per goal when the goal has holes in it.
    pub max_scan: usize,
}

impl Default for DeriveBudget {
    fn default() -> Self {
        DeriveBudget {
            max_steps: 10_000,
            max_depth: 32,
            max_results: 64,
            max_scan: 10_000,
        }
    }
}

impl DeriveBudget {
    pub fn with_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth;
        self
    }

    pub fn with_results(mut self, results: usize) -> Self {
        self.max_results = results;
        self
    }

    pub fn with_steps(mut self, steps: u64) -> Self {
        self.max_steps = steps;
        self
    }
}

/// Concept name for negation as failure. See [`Engine::condition_holds`].
const UNLESS: &str = "unless";

/// Backward chaining over a rule index and a store.
pub struct Engine<'a> {
    store: &'a Store,
    index: &'a dyn RuleIndex,
    budget: DeriveBudget,
    steps: u64,
    /// Goals currently being derived, so a rule cannot re-enter its own goal.
    ///
    /// Keyed on the resolved goal, so `Ancestor<A, C>` derived through
    /// `Ancestor<A, B>` is fine while `Ancestor<A, C>` through itself is cut.
    in_progress: Vec<ContentId>,
    /// Hole renaming offset, bumped per rule application so a rule's holes
    /// never collide with the goal's or with another instance of itself.
    next_offset: u32,
}

impl<'a> Engine<'a> {
    pub fn new(store: &'a Store, index: &'a dyn RuleIndex) -> Self {
        Engine {
            store,
            index,
            budget: DeriveBudget::default(),
            steps: 0,
            in_progress: Vec::new(),
            next_offset: 0,
        }
    }

    pub fn with_budget(mut self, budget: DeriveBudget) -> Self {
        self.budget = budget;
        self
    }

    pub fn steps_used(&self) -> u64 {
        self.steps
    }

    /// Everything that can be established about this goal.
    ///
    /// A goal with holes is a question: `Owns<?0, Dog>` asks who owns a dog,
    /// and each derivation's substitution carries one answer.
    pub fn derive(&mut self, goal: &Concept) -> Result<Vec<Derivation>> {
        // A goal that is nothing but a hole matches every fact in the brain.
        // Answering it means reading everything, which is a hang dressed up as
        // an answer.
        if goal.is_hole() {
            return Err(InferError::Unanchored { goal: goal.clone() });
        }
        self.steps = 0;
        self.derive_goal(goal, 0)
    }

    /// Whether the goal holds at all. Stops at the first derivation, since one
    /// is as good as many for a yes or no question.
    pub fn holds(&mut self, goal: &Concept) -> Result<bool> {
        let saved = self.budget.max_results;
        self.budget.max_results = 1;
        let result = self.derive(goal);
        self.budget.max_results = saved;
        Ok(!result?.is_empty())
    }

    fn derive_goal(&mut self, goal: &Concept, depth: u32) -> Result<Vec<Derivation>> {
        if depth > self.budget.max_depth {
            return Err(InferError::Exhausted { limit: "depth" });
        }

        let goal_id = goal.content_id();
        if self.in_progress.contains(&goal_id) {
            // Re-entering a goal already being derived proves nothing: the
            // chain would be assuming what it is trying to establish. Cutting
            // the branch is not an error, it just yields no derivations here.
            return Ok(Vec::new());
        }

        let mut found = self.direct_support(goal, depth)?;
        if found.len() >= self.budget.max_results {
            return Ok(found);
        }

        self.in_progress.push(goal_id);
        let result = self.derive_via_rules(goal, depth, &mut found);
        self.in_progress.pop();
        result?;

        dedupe(&mut found);
        Ok(found)
    }

    /// Derivations that need no rule: the goal is a stored fact.
    fn direct_support(&mut self, goal: &Concept, depth: u32) -> Result<Vec<Derivation>> {
        let mut out = Vec::new();

        if holes(goal).is_empty() {
            if self.store.holds(goal)? {
                out.push(Derivation {
                    goal: goal.clone(),
                    substitution: Substitution::new(),
                    support: Support::Asserted,
                    depth,
                });
            }
            return Ok(out);
        }

        // The goal has holes, so it is a question rather than a proposition.
        // Narrow with whatever index the goal's shape allows before unifying.
        for candidate in self.candidate_facts(goal)? {
            if out.len() >= self.budget.max_results {
                break;
            }
            let mut subst = Substitution::new();
            if unify(goal, &candidate, &mut subst) && self.store.holds(&candidate)? {
                out.push(Derivation {
                    goal: subst.apply(goal),
                    substitution: subst,
                    support: Support::Asserted,
                    depth,
                });
            }
        }
        Ok(out)
    }

    /// Stored concepts that could possibly unify with a goal containing holes.
    ///
    /// Anchored on the goal's head when it has one, and otherwise on its first
    /// concrete subterm. A goal with neither would mean reading the whole
    /// brain, so it is refused rather than answered slowly.
    fn candidate_facts(&self, goal: &Concept) -> Result<Vec<Concept>> {
        let limit = self.budget.max_scan;
        if let Some(head) = goal.head_symbol() {
            return Ok(self.store.concepts_by_head(head, limit)?);
        }
        if let Some(anchor) = first_concrete_subterm(goal) {
            return Ok(self.store.concepts_containing(anchor.content_id(), limit)?);
        }
        Err(InferError::Unanchored { goal: goal.clone() })
    }

    fn derive_via_rules(
        &mut self,
        goal: &Concept,
        depth: u32,
        found: &mut Vec<Derivation>,
    ) -> Result<()> {
        for rule in self.index.candidates(goal) {
            if found.len() >= self.budget.max_results {
                return Ok(());
            }
            self.steps += 1;
            if self.steps > self.budget.max_steps {
                return Err(InferError::Exhausted { limit: "steps" });
            }
            if !rule.is_usable() {
                continue;
            }
            self.apply_rule(&rule, goal, depth, found)?;
        }
        Ok(())
    }

    fn apply_rule(
        &mut self,
        rule: &StoredRule,
        goal: &Concept,
        depth: u32,
        found: &mut Vec<Derivation>,
    ) -> Result<()> {
        // The rule's holes are numbered independently of the goal's. Without
        // renaming, a rule written with ?0 would capture a goal's ?0 and derive
        // nonsense that happens to unify.
        let offset = self.fresh_offset(goal, rule);
        let produce = rename_holes(&rule.produce, offset);
        let pattern = rename_holes(&rule.pattern, offset);
        let condition = rule.condition.as_ref().map(|c| rename_holes(c, offset));

        let mut subst = Substitution::new();
        if !unify(goal, &produce, &mut subst) {
            return Ok(());
        }

        let subgoal = subst.apply(&pattern);
        if subgoal.is_hole() {
            // The antecedent came out completely unconstrained, so establishing
            // it would mean proving something about everything.
            return Ok(());
        }

        for premise in self.derive_goal(&subgoal, depth + 1)? {
            if found.len() >= self.budget.max_results {
                break;
            }
            let mut combined = subst.clone();
            for (hole, term) in premise.substitution.iter() {
                if !combined.bind(*hole, term.clone()) {
                    // The premise bound something incompatibly with the
                    // conclusion. That branch simply does not hold.
                    continue;
                }
            }

            if let Some(condition) = &condition {
                let instantiated = combined.apply(condition);
                if !self.condition_holds(&instantiated, depth + 1)? {
                    continue;
                }
            }

            found.push(Derivation {
                goal: combined.apply(goal),
                substitution: restrict_to(&combined, goal),
                support: Support::Rule {
                    name: rule.name.clone(),
                    premises: vec![premise],
                },
                depth,
            });
        }
        Ok(())
    }

    /// A rule's side condition.
    ///
    /// `Unless<X>` holds when `X` is NOT derivable, which is what makes a
    /// default defeasible: `DefaultExpectation<Person, HasArms, 2>` should stop
    /// applying to Greg the moment Spoon learns Greg has one arm.
    ///
    /// This is negation as failure, and it means exactly what it says: not
    /// derivable with what is known right now, which is weaker than false. A
    /// brain that has not yet learned something will happily conclude the
    /// default, and should, because that is what a default is for. The
    /// weakness is real and is why `Unless` is spelled out rather than written
    /// as `Not`.
    fn condition_holds(&mut self, condition: &Concept, depth: u32) -> Result<bool> {
        if condition.head_symbol() == Some(SymbolId::of(UNLESS))
            && let Some(inner) = condition.arg(0)
        {
            return Ok(self.derive_goal(inner, depth)?.is_empty());
        }
        Ok(!self.derive_goal(condition, depth)?.is_empty())
    }

    /// A renaming offset guaranteed not to collide with the goal or the rule.
    fn fresh_offset(&mut self, goal: &Concept, rule: &StoredRule) -> u32 {
        let highest = holes(goal)
            .iter()
            .chain(holes(&rule.produce).iter())
            .chain(holes(&rule.pattern).iter())
            .map(|h| h.0)
            .max()
            .unwrap_or(0);
        self.next_offset = self.next_offset.max(highest).saturating_add(1);
        self.next_offset
    }
}

/// Keep only the bindings for holes the caller actually asked about.
///
/// A derivation accumulates bindings for every hole introduced along the way,
/// including renamed ones from rules the caller never saw. Handing those back
/// would leak the engine's bookkeeping into the answer.
fn restrict_to(subst: &Substitution, goal: &Concept) -> Substitution {
    let mut out = Substitution::new();
    for hole in holes(goal) {
        let resolved = subst.apply(&Concept::Hole(hole));
        if resolved != Concept::Hole(hole) {
            out.bind(hole, resolved);
        }
    }
    out
}

/// The first subterm that is not a hole, used to anchor a store query.
fn first_concrete_subterm(goal: &Concept) -> Option<&Concept> {
    match goal {
        Concept::Hole(_) => None,
        Concept::Atomic(_) => Some(goal),
        Concept::Compound { head, args } => {
            if !head.is_hole() {
                return Some(head);
            }
            args.iter().find_map(first_concrete_subterm)
        }
    }
}

/// Two derivations of the same goal with the same bindings are one answer, even
/// when different rules produced them.
fn dedupe(found: &mut Vec<Derivation>) {
    let mut seen = HashSet::new();
    found.retain(|d| seen.insert(d.goal.content_id()));
    found.sort_by_key(|d| (d.depth, d.goal.content_id()));
}
