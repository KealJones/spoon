//! Orchestrator review: property checks over the ops layer.
//!
//! These are deliberately not the same tests the implementer wrote. They assert
//! the algebraic laws the rest of the system will lean on, over a generated
//! corpus rather than hand-picked cases.

use spoon_concept::{
    Concept, HoleId, alpha_equivalent, anti_unify, at_path, generalizes, holes, is_ground_term,
    positions, pre_order, rename_holes, substitute, substitute_positional,
};

/// Deterministic term generator. No external crates, seeded, reproducible.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn term(&mut self, depth: u32, allow_holes: bool) -> Concept {
        let leaf_only = depth == 0;
        let choice = self.pick(if leaf_only { 4 } else { 6 });
        match choice {
            0 => Concept::named(["greg", "keal", "math-add", "list-sort"][self.pick(4)]),
            1 => Concept::int((self.pick(5) as i64) - 2),
            2 => Concept::text(["a", "b", ""][self.pick(3)]),
            3 => {
                if allow_holes {
                    Concept::hole(self.pick(3) as u32)
                } else {
                    Concept::bool(self.pick(2) == 0)
                }
            }
            4 => {
                let arity = self.pick(3);
                let args: Vec<Concept> = (0..arity)
                    .map(|_| self.term(depth - 1, allow_holes))
                    .collect();
                Concept::call(["f", "g", "pair"][self.pick(3)], args)
            }
            _ => {
                // Compound head, so partial application shows up in the corpus.
                let head = self.term(depth - 1, allow_holes);
                let args: Vec<Concept> = (0..=self.pick(2))
                    .map(|_| self.term(depth - 1, allow_holes))
                    .collect();
                Concept::apply(head, args)
            }
        }
    }
}

fn corpus(count: usize, allow_holes: bool) -> Vec<Concept> {
    let mut g = Gen(0x5EED_1234_ABCD_0001);
    (0..count).map(|_| g.term(3, allow_holes)).collect()
}

// ---------------------------------------------------------------------------
// Paths address every node, and addressing is exact
// ---------------------------------------------------------------------------

#[test]
fn every_position_resolves_to_the_node_it_names() {
    for term in corpus(200, true) {
        for (path, node) in positions(&term) {
            let resolved = at_path(&term, &path)
                .unwrap_or_else(|| panic!("path {path} did not resolve in {term:?}"));
            assert_eq!(resolved, node, "path {path} resolved to the wrong node");
        }
    }
}

#[test]
fn positions_covers_exactly_the_nodes_preorder_visits() {
    for term in corpus(200, true) {
        let by_path: Vec<&Concept> = positions(&term).into_iter().map(|(_, n)| n).collect();
        let by_iter: Vec<&Concept> = pre_order(&term).collect();
        assert_eq!(by_path.len(), by_iter.len());
        assert_eq!(by_path, by_iter);
        assert_eq!(by_iter.len(), term.size(), "size disagrees with traversal");
    }
}

// ---------------------------------------------------------------------------
// Matching: the law that makes rule application sound
// ---------------------------------------------------------------------------

#[test]
fn a_successful_match_reconstructs_the_target() {
    // If a rule's pattern matches a term, substituting the resulting bindings
    // back into the pattern must give exactly that term. Rule application is
    // unsound without this.
    let mut checked = 0;
    for term in corpus(150, false) {
        for (_, sub) in positions(&term) {
            if let Some(bindings) = generalizes(sub, &term) {
                assert_eq!(substitute(sub, &bindings), term);
                checked += 1;
            }
        }
        // A term always matches itself with empty bindings.
        let self_match = generalizes(&term, &term).expect("term must match itself");
        assert_eq!(substitute(&term, &self_match), term);
    }
    assert!(checked > 0, "generated corpus exercised no sub-matches");
}

#[test]
fn a_bare_hole_matches_anything_and_recovers_it() {
    for term in corpus(100, true) {
        let bindings = generalizes(&Concept::hole(0), &term).expect("hole must match");
        assert_eq!(bindings.get(HoleId(0)), Some(&term));
        assert_eq!(substitute(&Concept::hole(0), &bindings), term);
    }
}

#[test]
fn a_repeated_hole_demands_the_same_term_twice() {
    let pattern = Concept::call("same", [Concept::hole(0), Concept::hole(0)]);
    let agrees = Concept::call("same", [Concept::named("greg"), Concept::named("greg")]);
    let differs = Concept::call("same", [Concept::named("greg"), Concept::named("keal")]);
    assert!(generalizes(&pattern, &agrees).is_some());
    assert!(generalizes(&pattern, &differs).is_none());
}

#[test]
fn matching_does_not_complete_a_gappy_target() {
    // A concrete pattern must not match a target that still has holes in it.
    // Otherwise a rule would fire on a term Spoon has not finished resolving,
    // silently inventing the missing part.
    let pattern = Concept::call("f", [Concept::named("greg")]);
    let gappy = Concept::call("f", [Concept::hole(0)]);
    assert!(generalizes(&pattern, &gappy).is_none());
}

// ---------------------------------------------------------------------------
// Anti-unification: least general, and it recovers both inputs
// ---------------------------------------------------------------------------

#[test]
fn anti_unification_recovers_both_inputs() {
    let terms = corpus(60, false);
    for left in terms.iter().take(30) {
        for right in terms.iter().skip(30) {
            let (general, lb, rb) = anti_unify(left, right);
            assert_eq!(&substitute(&general, &lb), left, "left recovery failed");
            assert_eq!(&substitute(&general, &rb), right, "right recovery failed");
        }
    }
}

#[test]
fn the_generalization_covers_both_inputs() {
    let terms = corpus(40, false);
    for left in terms.iter().take(20) {
        for right in terms.iter().skip(20) {
            let (general, _, _) = anti_unify(left, right);
            assert!(generalizes(&general, left).is_some(), "does not cover left");
            assert!(
                generalizes(&general, right).is_some(),
                "does not cover right"
            );
        }
    }
}

#[test]
fn the_same_disagreement_gets_one_name() {
    // Pair<7, 7> against Pair<9, 9> is Pair<H, H>, not Pair<H0, H1>. Two names
    // would lose the fact that both positions move together, which is exactly
    // the structure consolidation is trying to capture.
    let left = Concept::call("pair", [Concept::int(7), Concept::int(7)]);
    let right = Concept::call("pair", [Concept::int(9), Concept::int(9)]);
    let (general, _, _) = anti_unify(&left, &right);
    assert_eq!(holes(&general).len(), 1, "got {general:?}");
    assert_eq!(general.arity(), 2);
    assert_eq!(general.arg(0), general.arg(1));
}

#[test]
fn independent_disagreements_get_separate_names() {
    let left = Concept::call("pair", [Concept::int(7), Concept::int(8)]);
    let right = Concept::call("pair", [Concept::int(9), Concept::int(10)]);
    let (general, _, _) = anti_unify(&left, &right);
    assert_eq!(holes(&general).len(), 2, "got {general:?}");
}

#[test]
fn identical_inputs_generalize_to_themselves() {
    for term in corpus(60, false) {
        let (general, lb, rb) = anti_unify(&term, &term);
        assert_eq!(general, term);
        assert!(lb.is_empty() && rb.is_empty(), "no holes were needed");
    }
}

#[test]
fn fresh_holes_never_capture_existing_ones() {
    // Both inputs already carry Hole(0). The generalization must not reuse it
    // for a new disagreement, or substitution would rebind the input's own hole.
    let left = Concept::call("f", [Concept::hole(0), Concept::int(1)]);
    let right = Concept::call("f", [Concept::hole(0), Concept::int(2)]);
    let (general, lb, rb) = anti_unify(&left, &right);
    assert_eq!(substitute(&general, &lb), left);
    assert_eq!(substitute(&general, &rb), right);
    // The shared Hole(0) stayed shared; only the differing position got a hole.
    assert_eq!(general.arg(0), Some(&Concept::hole(0)));
    assert_ne!(general.arg(1), Some(&Concept::hole(0)));
}

#[test]
fn differing_heads_generalize_to_a_hole_head() {
    // Rules like Symmetric<R> need a hole in head position, so anti-unification
    // has to be willing to put one there.
    let left = Concept::call("f", [Concept::int(1)]);
    let right = Concept::call("g", [Concept::int(1)]);
    let (general, lb, rb) = anti_unify(&left, &right);
    assert!(general.head().unwrap().is_hole(), "got {general:?}");
    assert_eq!(substitute(&general, &lb), left);
    assert_eq!(substitute(&general, &rb), right);
}

// ---------------------------------------------------------------------------
// Alpha equivalence is an equivalence relation
// ---------------------------------------------------------------------------

#[test]
fn alpha_equivalence_is_reflexive_symmetric_and_transitive() {
    let terms = corpus(80, true);
    for t in &terms {
        assert!(alpha_equivalent(t, t), "not reflexive");
    }
    for (i, a) in terms.iter().enumerate() {
        for b in terms.iter().skip(i + 1) {
            assert_eq!(
                alpha_equivalent(a, b),
                alpha_equivalent(b, a),
                "not symmetric"
            );
        }
    }
    // Transitivity through a renaming: a ~ shift(a) ~ shift(shift(a)).
    for t in &terms {
        let once = rename_holes(t, 10);
        let twice = rename_holes(&once, 10);
        assert!(alpha_equivalent(t, &once));
        assert!(alpha_equivalent(&once, &twice));
        assert!(alpha_equivalent(t, &twice), "transitivity broken");
    }
}

#[test]
fn renaming_holes_preserves_shape_but_not_identity() {
    for term in corpus(80, true) {
        let shifted = rename_holes(&term, 100);
        assert!(alpha_equivalent(&term, &shifted));
        if !holes(&term).is_empty() {
            assert_ne!(term, shifted, "shift should change the ids");
            assert!(holes(&shifted).iter().all(|h| h.0 >= 100));
        } else {
            assert_eq!(term, shifted, "hole-free term should be untouched");
        }
    }
}

#[test]
fn sharing_a_hole_is_a_different_claim_from_using_two() {
    let shared = Concept::call("f", [Concept::hole(0), Concept::hole(0)]);
    let distinct = Concept::call("f", [Concept::hole(0), Concept::hole(1)]);
    assert!(!alpha_equivalent(&shared, &distinct));
    assert!(!alpha_equivalent(&distinct, &shared));
}

// ---------------------------------------------------------------------------
// Positional substitution: how a Composed realization binds its call
// ---------------------------------------------------------------------------

#[test]
fn composed_bodies_bind_positionally() {
    // `double` is Add<Hole(0), Hole(0)>. Applied to [21] it becomes Add<21, 21>.
    let body = Concept::call("math-add", [Concept::hole(0), Concept::hole(0)]);
    let bound = substitute_positional(&body, &[Concept::int(21)]);
    assert_eq!(
        bound,
        Concept::call("math-add", [Concept::int(21), Concept::int(21)])
    );
    assert!(is_ground_term(&bound));
}

#[test]
fn missing_positional_arguments_leave_holes_standing() {
    // Under-applying is not an error at this layer. The result is a concept
    // that is still meaningful and still has a visible gap, which is what lets
    // the evaluator report a placeholder instead of inventing a value.
    let body = Concept::call("math-add", [Concept::hole(0), Concept::hole(1)]);
    let bound = substitute_positional(&body, &[Concept::int(21)]);
    assert_eq!(bound.arg(0), Some(&Concept::int(21)));
    assert_eq!(bound.arg(1), Some(&Concept::hole(1)));
    assert!(!is_ground_term(&bound));
}

#[test]
fn ground_term_asks_about_completeness_not_identity() {
    // The easy confusion: Concept::is_ground asks whether identity is a value,
    // is_ground_term asks whether the tree has any holes left.
    let forty_two = Concept::int(42);
    assert!(forty_two.is_ground());
    assert!(is_ground_term(&forty_two));

    let named = Concept::named("greg");
    assert!(!named.is_ground());
    assert!(is_ground_term(&named), "no holes means ground as a term");

    let gappy = Concept::call("math-add", [Concept::int(1), Concept::hole(0)]);
    assert!(!gappy.is_ground());
    assert!(!is_ground_term(&gappy));
}
