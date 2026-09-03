//! Comparing two terms structurally.
//!
//! Three questions get asked constantly and are easy to conflate, so they are
//! three separate functions with three different answers:
//!
//! - [`alpha_equivalent`]: are these the same pattern, ignoring how the holes
//!   happen to be numbered?
//! - [`generalizes`]: does this pattern cover that term, and how?
//! - [`anti_unify`]: what is the most specific pattern that covers both?
//!
//! None of these is unification. Nothing here ever binds a hole on the
//! right-hand side. Holes in a target are opaque terms, equal only to
//! themselves. Full two-way unification belongs to `spoon-infer`, where the
//! occurs check and the rest of the machinery it needs can live.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::concept::{Concept, ContentId};
use crate::id::HoleId;
use crate::ops::subst::{Bindings, max_hole};

/// Are two terms the same up to a consistent renaming of holes?
///
/// Hole numbering is local to a pattern, so the same rule written by two
/// different producers can be structurally identical and still fail `==`.
/// Deduplicating learned rules, and deciding whether a synthesized body is
/// something already known, both need this rather than plain equality.
///
/// The renaming must be a bijection. `f<Hole(0), Hole(1)>` is alpha-equivalent
/// to `f<Hole(3), Hole(7)>` but not to `f<Hole(0), Hole(0)>`: the second says
/// its two arguments are the same term, which is a different claim.
pub fn alpha_equivalent(left: &Concept, right: &Concept) -> bool {
    let mut forward = BTreeMap::new();
    let mut backward = BTreeMap::new();
    alpha_rec(left, right, &mut forward, &mut backward)
}

fn alpha_rec(
    left: &Concept,
    right: &Concept,
    forward: &mut BTreeMap<HoleId, HoleId>,
    backward: &mut BTreeMap<HoleId, HoleId>,
) -> bool {
    match (left, right) {
        (Concept::Atomic(a), Concept::Atomic(b)) => a == b,
        (Concept::Hole(a), Concept::Hole(b)) => {
            match (forward.get(a).copied(), backward.get(b).copied()) {
                (None, None) => {
                    forward.insert(*a, *b);
                    backward.insert(*b, *a);
                    true
                }
                (Some(mapped), Some(back)) => mapped == *b && back == *a,
                // One side is already spoken for: not a bijection.
                _ => false,
            }
        }
        (
            Concept::Compound {
                head: lh,
                args: la,
            },
            Concept::Compound {
                head: rh,
                args: ra,
            },
        ) => {
            la.len() == ra.len()
                && alpha_rec(lh, rh, forward, backward)
                && la
                    .iter()
                    .zip(ra.iter())
                    .all(|(l, r)| alpha_rec(l, r, forward, backward))
        }
        _ => false,
    }
}

/// Does `pattern` cover `target`, and with what hole bindings?
///
/// One-directional matching, not unification. Holes in `pattern` bind to
/// whatever sits at that position in `target`; holes in `target` are opaque
/// terms that only a pattern hole can absorb. That asymmetry is deliberate:
/// rule application asks "does this rule apply to this term", and a term with
/// gaps in it must not be silently completed to make a rule fire.
///
/// A hole appearing twice must bind to the same term both times, so
/// `Same<Hole(0), Hole(0)>` matches `Same<Greg, Greg>` and rejects
/// `Same<Greg, Keal>`.
///
/// `Some(bindings)` on success, and `substitute(pattern, &bindings)` is then
/// equal to `target`.
pub fn generalizes(pattern: &Concept, target: &Concept) -> Option<Bindings> {
    let mut bindings = Bindings::new();
    if match_rec(pattern, target, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn match_rec(pattern: &Concept, target: &Concept, bindings: &mut Bindings) -> bool {
    match pattern {
        Concept::Hole(hole) => bindings.try_bind(*hole, target.clone()),
        Concept::Atomic(id) => matches!(target, Concept::Atomic(other) if id == other),
        Concept::Compound { head, args } => match target {
            Concept::Compound {
                head: target_head,
                args: target_args,
            } => {
                args.len() == target_args.len()
                    && match_rec(head, target_head, bindings)
                    && args
                        .iter()
                        .zip(target_args.iter())
                        .all(|(p, t)| match_rec(p, t, bindings))
            }
            _ => false,
        },
    }
}

/// The most specific pattern that covers both terms (Plotkin's least general
/// generalization).
///
/// Where the two terms agree, the shared structure is kept. Where they differ,
/// a fresh hole stands in. Returns the generalized term plus the bindings that
/// recover each input, so `substitute(&general, &left_bindings) == left` and
/// likewise for the right.
///
/// This is what consolidation runs on repeated structure: seeing `Add<1, 2>`
/// and `Add<1, 3>` many times, the shared shape `Add<1, Hole(n)>` is the
/// abstraction worth naming. Doing it by hand would mean guessing which
/// differences matter; anti-unification computes the answer.
///
/// A given pair of differing subterms always maps to the *same* fresh hole, so
/// `Pair<7, 7>` against `Pair<9, 9>` generalizes to `Pair<Hole(n), Hole(n)>`
/// and not to two independent holes. That is what makes the result least
/// general rather than merely a generalization.
///
/// Fresh ids start above every hole already present in either input, so the
/// result never captures a hole the inputs were carrying.
pub fn anti_unify(left: &Concept, right: &Concept) -> (Concept, Bindings, Bindings) {
    let next = [max_hole(left), max_hole(right)]
        .into_iter()
        .flatten()
        .map(|hole| hole.0.saturating_add(1))
        .max()
        .unwrap_or(0);
    let mut state = AntiUnifier {
        next,
        seen: HashMap::new(),
        left: Bindings::new(),
        right: Bindings::new(),
    };
    let general = state.generalize(left, right);
    (general, state.left, state.right)
}

struct AntiUnifier {
    next: u32,
    /// Difference pairs already assigned a hole, keyed by structural digest so
    /// the same disagreement is never given two names.
    seen: HashMap<(ContentId, ContentId), HoleId>,
    left: Bindings,
    right: Bindings,
}

impl AntiUnifier {
    fn generalize(&mut self, left: &Concept, right: &Concept) -> Concept {
        if left == right {
            // Identical subtrees need no hole, and cloning is O(1).
            return left.clone();
        }
        if let (
            Concept::Compound {
                head: lh,
                args: la,
            },
            Concept::Compound {
                head: rh,
                args: ra,
            },
        ) = (left, right)
            && la.len() == ra.len()
        {
            let head = self.generalize(lh, rh);
            let args: Vec<Concept> = la
                .iter()
                .zip(ra.iter())
                .map(|(l, r)| self.generalize(l, r))
                .collect();
            return Concept::Compound {
                head: Arc::new(head),
                args: Arc::from(args),
            };
        }
        Concept::Hole(self.fresh_for(left, right))
    }

    fn fresh_for(&mut self, left: &Concept, right: &Concept) -> HoleId {
        let key = (left.content_id(), right.content_id());
        if let Some(existing) = self.seen.get(&key) {
            return *existing;
        }
        let hole = HoleId(self.next);
        self.next = self.next.saturating_add(1);
        self.seen.insert(key, hole);
        self.left.bind(hole, left.clone());
        self.right.bind(hole, right.clone());
        hole
    }
}
