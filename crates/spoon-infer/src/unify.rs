//! Two-way unification over concepts.
//!
//! Stage 1's `generalizes` is one-way: holes in the pattern bind, holes in the
//! target are opaque. That is the right operation for rewriting, because a rule
//! must not silently complete a term Spoon has not finished resolving.
//!
//! Derivation needs the other operation. Asking whether `FriendWith<Keal, ?x>`
//! holds means matching a goal that has its own holes against a rule conclusion
//! that has its own holes, and letting both sides bind. That is unification,
//! and it needs machinery one-way matching does not: chains of bindings have to
//! be followed, and the occurs check has to stop a hole being bound to a term
//! containing itself.

use std::collections::BTreeMap;

use spoon_concept::{Concept, HoleId};

/// A set of hole bindings that may chain.
///
/// Unification can bind `?0` to `?1` and later bind `?1` to `Greg`, at which
/// point `?0` is `Greg` too. Nothing collapses those chains eagerly, so every
/// read goes through [`Substitution::resolve`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Substitution {
    bindings: BTreeMap<HoleId, Concept>,
}

impl Substitution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&HoleId, &Concept)> {
        self.bindings.iter()
    }

    /// What a hole is bound to right now, without following the chain.
    pub fn get(&self, hole: HoleId) -> Option<&Concept> {
        self.bindings.get(&hole)
    }

    /// Follow a term to the furthest thing currently known about it.
    ///
    /// Only walks the top of the term: `?0` bound to `f<?1>` resolves to
    /// `f<?1>`, not to `f<Greg>`. Unification only ever needs the head to
    /// decide what to do next, and resolving deeply on every step would turn a
    /// linear walk into a quadratic one. Use [`Substitution::apply`] when the
    /// fully resolved term is what is wanted.
    pub fn resolve<'a>(&'a self, term: &'a Concept) -> &'a Concept {
        let mut current = term;
        // Depth-bounded because a malformed substitution built by hand could
        // still contain a cycle; unification itself cannot create one.
        for _ in 0..1024 {
            match current {
                Concept::Hole(hole) => match self.bindings.get(hole) {
                    Some(next) => current = next,
                    None => return current,
                },
                _ => return current,
            }
        }
        current
    }

    /// Substitute everywhere, following chains to the end.
    pub fn apply(&self, term: &Concept) -> Concept {
        match self.resolve(term) {
            Concept::Compound { head, args } => {
                let new_head = self.apply(head);
                let new_args: Vec<Concept> = args.iter().map(|a| self.apply(a)).collect();
                Concept::apply(new_head, new_args)
            }
            other => other.clone(),
        }
    }

    /// Bind a hole, refusing bindings that would build an infinite term.
    ///
    /// Without the occurs check, unifying `?0` with `f<?0>` succeeds and
    /// produces a term that is its own subterm. Every later attempt to print,
    /// hash, or walk it runs forever. Prolog omits this check for speed and
    /// documents the resulting unsoundness; Spoon is not fast enough for that
    /// trade to be worth it.
    pub fn bind(&mut self, hole: HoleId, term: Concept) -> bool {
        if let Concept::Hole(other) = self.resolve(&term)
            && *other == hole
        {
            // Binding a hole to itself is a no-op, not a failure.
            return true;
        }
        if self.occurs(hole, &term) {
            return false;
        }
        self.bindings.insert(hole, term);
        true
    }

    /// Does this hole appear anywhere inside the term, after resolution?
    pub fn occurs(&self, hole: HoleId, term: &Concept) -> bool {
        match self.resolve(term) {
            Concept::Hole(other) => *other == hole,
            Concept::Compound { head, args } => {
                self.occurs(hole, head) || args.iter().any(|a| self.occurs(hole, a))
            }
            Concept::Atomic(_) => false,
        }
    }
}

/// Unify two terms, extending `subst` in place.
///
/// Returns false on failure, and `subst` is left partially extended. Callers
/// that need to try several alternatives should clone the substitution first;
/// backtracking by undoing bindings would need a trail, and cloning a
/// pattern-sized map is cheaper than maintaining one.
pub fn unify(left: &Concept, right: &Concept, subst: &mut Substitution) -> bool {
    let l = subst.resolve(left).clone();
    let r = subst.resolve(right).clone();

    match (&l, &r) {
        (Concept::Hole(h), _) => subst.bind(*h, r.clone()),
        (_, Concept::Hole(h)) => subst.bind(*h, l.clone()),
        (Concept::Atomic(a), Concept::Atomic(b)) => a == b,
        (Concept::Compound { head: lh, args: la }, Concept::Compound { head: rh, args: ra }) => {
            if la.len() != ra.len() {
                return false;
            }
            if !unify(lh, rh, subst) {
                return false;
            }
            la.iter().zip(ra.iter()).all(|(a, b)| unify(a, b, subst))
        }
        _ => false,
    }
}

/// Unify from a clean slate.
pub fn unify_terms(left: &Concept, right: &Concept) -> Option<Substitution> {
    let mut subst = Substitution::new();
    unify(left, right, &mut subst).then_some(subst)
}
