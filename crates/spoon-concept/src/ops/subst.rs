//! Holes, and what happens when you fill them.
//!
//! A hole is the one shape that is not a concept: a gap in a rule pattern or a
//! partially resolved expression. Filling holes is the single mechanism behind
//! rule firing, `Composed` realization expansion, and synthesis, so all three
//! share the code here rather than each writing their own tree walk.

use std::collections::{BTreeMap, BTreeSet, btree_map};
use std::sync::Arc;

use crate::concept::Concept;
use crate::id::HoleId;
use crate::ops::traverse::pre_order;

/// Every distinct hole in the term.
///
/// Ordered, because callers make decisions from it (allocating fresh ids,
/// deciding whether a pattern is linear) and those decisions must not vary run
/// to run with hash seeding.
pub fn holes(term: &Concept) -> BTreeSet<HoleId> {
    pre_order(term).filter_map(|node| node.as_hole()).collect()
}

/// The highest hole id present, if any.
///
/// The cheap way to pick a fresh id: anything above this cannot collide.
/// Used by [`rename_holes`] callers and by anti-unification.
pub fn max_hole(term: &Concept) -> Option<HoleId> {
    pre_order(term).filter_map(|node| node.as_hole()).max()
}

/// How many hole *occurrences* the term contains.
///
/// Occurrences, not distinct ids: `Add<Hole(0), Hole(0)>` is 2. That is the
/// number synthesis budgets and pattern-cost heuristics care about. For the
/// distinct count use `holes(term).len()`.
pub fn hole_count(term: &Concept) -> usize {
    pre_order(term).filter(|node| node.is_hole()).count()
}

/// True when the term contains no holes anywhere: it is fully built out.
///
/// Not the same question as [`Concept::is_ground`], and the two are easy to
/// confuse. `Concept::is_ground` asks about *identity*: is this one atomic
/// concept identified by a literal value rather than a name? `is_ground_term`
/// asks about *completeness*: does this whole tree still have gaps in it?
/// `Concept::named("greg")` is a ground term but is not `is_ground`;
/// `Add<42, Hole(0)>` contains a ground concept but is not a ground term.
pub fn is_ground_term(term: &Concept) -> bool {
    !pre_order(term).any(|node| node.is_hole())
}

/// What each hole stands for.
///
/// Produced by matching, consumed by substitution. Ordered by hole id so that
/// two runs over the same inputs yield byte-identical output, which matters as
/// soon as bindings are hashed, logged, or persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    map: BTreeMap<HoleId, Concept>,
}

impl Bindings {
    pub fn new() -> Self {
        Bindings::default()
    }

    /// Bind a hole, overwriting any previous value. Use [`try_bind`] when an
    /// existing conflicting binding should be a failure instead.
    ///
    /// [`try_bind`]: Bindings::try_bind
    pub fn bind(&mut self, hole: HoleId, value: Concept) -> Option<Concept> {
        self.map.insert(hole, value)
    }

    /// Bind a hole only if that is consistent with what is already bound.
    ///
    /// Returns `false` when the hole is already bound to something else. This
    /// is what makes a non-linear pattern like `Same<Hole(0), Hole(0)>` mean
    /// "both arguments are the same term" instead of "two arguments".
    pub fn try_bind(&mut self, hole: HoleId, value: Concept) -> bool {
        match self.map.entry(hole) {
            btree_map::Entry::Vacant(slot) => {
                slot.insert(value);
                true
            }
            btree_map::Entry::Occupied(slot) => slot.get() == &value,
        }
    }

    pub fn get(&self, hole: HoleId) -> Option<&Concept> {
        self.map.get(&hole)
    }

    pub fn contains(&self, hole: HoleId) -> bool {
        self.map.contains_key(&hole)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (HoleId, &Concept)> {
        self.map.iter().map(|(hole, value)| (*hole, value))
    }

    /// Combine two binding sets, or fail if they disagree.
    ///
    /// Returns `None` when both bind the same hole to different terms. A
    /// conflict is not an error to report, it is a match that did not happen:
    /// conjunctive rule conditions each produce bindings, and the rule only
    /// fires if they all agree.
    pub fn merge(&self, other: &Bindings) -> Option<Bindings> {
        let mut merged = self.clone();
        for (hole, value) in other.iter() {
            if !merged.try_bind(hole, value.clone()) {
                return None;
            }
        }
        Some(merged)
    }
}

impl FromIterator<(HoleId, Concept)> for Bindings {
    fn from_iter<I: IntoIterator<Item = (HoleId, Concept)>>(iter: I) -> Self {
        Bindings {
            map: iter.into_iter().collect(),
        }
    }
}

/// Replace every bound hole with its binding.
///
/// Holes with no binding stay holes rather than becoming an error: partial
/// substitution is normal. A rule applied against an incomplete match still
/// produces a usable pattern, and synthesis fills a template one hole at a
/// time.
///
/// Substitution is single-pass. Bound values are inserted as they are and are
/// not re-substituted, so a binding that mentions a hole leaves that hole
/// standing. That keeps the operation terminating without an occurs check.
pub fn substitute(term: &Concept, bindings: &Bindings) -> Concept {
    if bindings.is_empty() {
        return term.clone();
    }
    subst_rec(term, &|hole| bindings.get(hole).cloned()).unwrap_or_else(|| term.clone())
}

/// Bind holes by position: `Hole(0)` takes `args[0]`, `Hole(1)` takes
/// `args[1]`, and so on.
///
/// This is how a `Composed` realization body meets a call. `double` is stored
/// as `Add<Hole(0), Hole(0)>`; applied to `[21]` it becomes `Add<21, 21>`.
/// Holes past the end of `args` are left alone, which is what makes partial
/// application fall out for free instead of needing its own code path.
pub fn substitute_positional(term: &Concept, args: &[Concept]) -> Concept {
    if args.is_empty() {
        return term.clone();
    }
    subst_rec(term, &|hole| args.get(hole.0 as usize).cloned()).unwrap_or_else(|| term.clone())
}

/// Shift every hole id up by `offset`.
///
/// Hole numbering is local to one pattern, so `Hole(0)` in two different rules
/// means two different things. Whenever two patterns are considered together
/// (chaining rules, anti-unifying two bodies, matching a rule against another
/// rule's output) one side gets renamed clear of the other first, usually by
/// `max_hole(other).0 + 1`.
///
/// Ids saturate at `u32::MAX` rather than wrapping, so an absurd offset
/// collapses holes together instead of silently aliasing low ids. Offsets in
/// practice are the size of a pattern, nowhere near the limit.
pub fn rename_holes(term: &Concept, offset: u32) -> Concept {
    if offset == 0 {
        return term.clone();
    }
    subst_rec(term, &|hole| {
        Some(Concept::Hole(HoleId(hole.0.saturating_add(offset))))
    })
    .unwrap_or_else(|| term.clone())
}

/// Shared engine for every substitution above.
///
/// Returns `None` for "nothing under here changed", which is what lets callers
/// hand back the original `Arc` instead of rebuilding an identical subtree.
/// Substituting one hole in a thousand-node term touches only the spine.
fn subst_rec(term: &Concept, lookup: &dyn Fn(HoleId) -> Option<Concept>) -> Option<Concept> {
    match term {
        Concept::Atomic(_) => None,
        Concept::Hole(hole) => lookup(*hole),
        Concept::Compound { head, args } => {
            let new_head = subst_rec(head, lookup);
            let mut new_args: Option<Vec<Concept>> = None;
            for (i, arg) in args.iter().enumerate() {
                if let Some(replaced) = subst_rec(arg, lookup) {
                    new_args.get_or_insert_with(|| args.to_vec())[i] = replaced;
                }
            }
            if new_head.is_none() && new_args.is_none() {
                return None;
            }
            Some(Concept::Compound {
                head: match new_head {
                    Some(built) => Arc::new(built),
                    None => head.clone(),
                },
                args: match new_args {
                    Some(built) => Arc::from(built),
                    None => args.clone(),
                },
            })
        }
    }
}
