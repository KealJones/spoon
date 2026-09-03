//! Structural operation tests. Names are kebab-case per AGENTS.md.

use std::sync::Arc;

use spoon_concept::{
    Bindings, Concept, HoleId, Path, PathStep, alpha_equivalent, anti_unify, at_path, find_first,
    flatten_spine, generalizes, hole_count, holes, is_ground_term, map_args, max_hole, positions,
    post_order, pre_order, rename_holes, replace_at, substitute, substitute_positional, subterms,
    try_map_args, walk,
};

fn f(args: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("f", args)
}

fn add(args: impl IntoIterator<Item = Concept>) -> Concept {
    Concept::call("add", args)
}

/// `add<f<1>, 2>`
fn nested() -> Concept {
    add([f([Concept::int(1)]), Concept::int(2)])
}

// ---- traversal ----

#[test]
fn pre_order_visits_node_then_head_then_args() {
    let term = nested();
    let seen: Vec<&Concept> = pre_order(&term).collect();
    let expect = [
        term.clone(),
        Concept::named("add"),
        f([Concept::int(1)]),
        Concept::named("f"),
        Concept::int(1),
        Concept::int(2),
    ];
    assert_eq!(seen.len(), expect.len());
    for (got, want) in seen.iter().zip(expect.iter()) {
        assert_eq!(*got, want);
    }
}

#[test]
fn post_order_visits_children_before_parents() {
    let term = nested();
    let seen: Vec<Concept> = post_order(&term).cloned().collect();
    assert_eq!(
        seen,
        vec![
            Concept::named("add"),
            Concept::named("f"),
            Concept::int(1),
            f([Concept::int(1)]),
            Concept::int(2),
            term.clone(),
        ]
    );
}

#[test]
fn traversals_cover_every_node() {
    let term = nested();
    assert_eq!(pre_order(&term).count(), term.size());
    assert_eq!(post_order(&term).count(), term.size());
}

#[test]
fn atomic_traversal_is_a_single_node() {
    let greg = Concept::named("greg");
    assert_eq!(pre_order(&greg).count(), 1);
    assert_eq!(post_order(&greg).count(), 1);
}

#[test]
fn subterms_drops_structural_duplicates() {
    // add<x, x> mentions x twice but contains one distinct x.
    let term = add([Concept::named("x"), Concept::named("x")]);
    let distinct: Vec<&Concept> = subterms(&term);
    assert_eq!(distinct.len(), 3); // the compound, add, x
    assert!(distinct.contains(&&Concept::named("x")));
    assert!(distinct.contains(&&Concept::named("add")));
}

#[test]
fn walk_reports_depth_and_can_prune() {
    let term = nested();
    let mut depths = Vec::new();
    walk(&term, &mut |node: &Concept, depth: usize| {
        depths.push((node.clone(), depth));
        true
    });
    assert_eq!(depths[0].1, 0);
    assert_eq!(depths[1], (Concept::named("add"), 1));
    assert_eq!(depths[2].1, 1); // f<1>
    assert_eq!(depths[3].1, 2); // f

    // Returning false stops the descent into f<1>.
    let mut pruned = Vec::new();
    walk(&term, &mut |node: &Concept, _depth: usize| {
        pruned.push(node.clone());
        node.head_symbol() != Concept::named("f").as_symbol()
    });
    assert!(!pruned.contains(&Concept::int(1)));
    assert!(pruned.contains(&Concept::int(2)));
}

#[test]
fn find_first_is_leftmost_outermost() {
    let term = add([f([f([Concept::int(1)])]), f([Concept::int(9)])]);
    let found = find_first(&term, |node| {
        node.head_symbol() == Concept::named("f").as_symbol()
    })
    .unwrap();
    assert_eq!(*found, f([f([Concept::int(1)])]));
    assert!(find_first(&term, |node| node.is_hole()).is_none());
}

// ---- paths ----

#[test]
fn path_round_trips_through_at_path() {
    let term = add([f([Concept::int(1)]), Concept::hole(0)]);
    let sites = positions(&term);
    assert_eq!(sites.len(), term.size());
    for (path, sub) in &sites {
        assert_eq!(
            at_path(&term, path),
            Some(*sub),
            "path {path} did not resolve"
        );
    }
}

#[test]
fn positions_match_pre_order() {
    let term = nested();
    let by_path: Vec<&Concept> = positions(&term).into_iter().map(|(_, sub)| sub).collect();
    let by_iter: Vec<&Concept> = pre_order(&term).collect();
    assert_eq!(by_path.len(), by_iter.len());
    for (a, b) in by_path.iter().zip(by_iter.iter()) {
        assert_eq!(a, b);
    }
}

#[test]
fn at_path_rejects_paths_that_do_not_fit() {
    let term = nested();
    let mut bogus = Path::root();
    bogus.push(PathStep::Arg(7));
    assert!(at_path(&term, &bogus).is_none());

    let mut into_atom = Path::root();
    into_atom.push(PathStep::Head);
    into_atom.push(PathStep::Head);
    assert!(at_path(&term, &into_atom).is_none());
}

#[test]
fn path_displays_readably() {
    assert_eq!(Path::root().to_string(), ".");
    let path: Path = vec![PathStep::Head, PathStep::Arg(1), PathStep::Arg(0)]
        .into_iter()
        .collect();
    assert_eq!(path.to_string(), "head/1/0");
    assert_eq!(path.len(), 3);
    assert!(!path.is_root());
    assert!(Path::root().is_empty());
    assert_eq!(Path::root().child(PathStep::Arg(2)).to_string(), "2");
}

#[test]
fn replace_at_works_at_every_position() {
    let term = nested();
    let mark = Concept::named("mark");
    for (path, sub) in positions(&term) {
        let rebuilt = replace_at(&term, &path, mark.clone())
            .unwrap_or_else(|| panic!("replace failed at {path}"));
        assert_eq!(at_path(&rebuilt, &path), Some(&mark), "at {path}");
        assert_eq!(
            rebuilt.size(),
            term.size() - sub.size() + mark.size(),
            "size wrong at {path}"
        );
        if path.is_root() {
            assert_eq!(rebuilt, mark);
        }
    }
}

#[test]
fn replace_at_rejects_a_stale_path() {
    let term = nested();
    let mut bogus = Path::root();
    bogus.push(PathStep::Arg(9));
    assert!(replace_at(&term, &bogus, Concept::named("mark")).is_none());
}

#[test]
fn replace_at_keeps_untouched_siblings_shared() {
    let big = f([Concept::int(1), Concept::int(2)]);
    let term = add([big.clone(), Concept::int(3)]);
    let path: Path = vec![PathStep::Arg(1)].into_iter().collect();
    let rebuilt = replace_at(&term, &path, Concept::int(4)).unwrap();

    let (Concept::Compound { args: before, .. }, Concept::Compound { args: after, .. }) =
        (&term, &rebuilt)
    else {
        panic!("expected compounds");
    };
    let (
        Concept::Compound {
            args: inner_before, ..
        },
        Concept::Compound {
            args: inner_after, ..
        },
    ) = (&before[0], &after[0])
    else {
        panic!("expected nested compounds");
    };
    assert!(
        Arc::ptr_eq(inner_before, inner_after),
        "untouched subtree was rebuilt instead of shared"
    );
}

// ---- holes ----

#[test]
fn hole_queries_agree_with_the_term() {
    let term = add([Concept::hole(0), f([Concept::hole(3), Concept::hole(0)])]);
    assert_eq!(
        holes(&term).into_iter().collect::<Vec<_>>(),
        vec![HoleId(0), HoleId(3)]
    );
    assert_eq!(max_hole(&term), Some(HoleId(3)));
    assert_eq!(hole_count(&term), 3); // occurrences, not distinct ids
    assert!(!is_ground_term(&term));

    let closed = add([Concept::int(1), Concept::int(2)]);
    assert!(holes(&closed).is_empty());
    assert_eq!(max_hole(&closed), None);
    assert_eq!(hole_count(&closed), 0);
    assert!(is_ground_term(&closed));
}

#[test]
fn ground_term_is_not_ground_identity() {
    // A named concept has no holes, so it is a ground term, but its identity
    // is a name, so it is not `is_ground`.
    let greg = Concept::named("greg");
    assert!(is_ground_term(&greg));
    assert!(!greg.is_ground());

    // A term containing a ground concept is not a ground term while a hole
    // remains.
    let partial = add([Concept::int(42), Concept::hole(0)]);
    assert!(!is_ground_term(&partial));
    assert!(partial.arg(0).unwrap().is_ground());
}

// ---- bindings ----

#[test]
fn bindings_basics() {
    let mut bindings = Bindings::new();
    assert!(bindings.is_empty());
    assert_eq!(bindings.len(), 0);
    bindings.bind(HoleId(0), Concept::int(1));
    bindings.bind(HoleId(2), Concept::int(3));
    assert_eq!(bindings.len(), 2);
    assert!(!bindings.is_empty());
    assert!(bindings.contains(HoleId(2)));
    assert_eq!(bindings.get(HoleId(0)), Some(&Concept::int(1)));
    assert_eq!(bindings.get(HoleId(1)), None);
    // Ordered by hole id, so output is reproducible.
    let ids: Vec<HoleId> = bindings.iter().map(|(hole, _)| hole).collect();
    assert_eq!(ids, vec![HoleId(0), HoleId(2)]);

    let collected: Bindings = vec![(HoleId(0), Concept::int(1)), (HoleId(2), Concept::int(3))]
        .into_iter()
        .collect();
    assert_eq!(collected, bindings);
}

#[test]
fn try_bind_accepts_agreement_and_rejects_conflict() {
    let mut bindings = Bindings::new();
    assert!(bindings.try_bind(HoleId(0), Concept::int(1)));
    assert!(bindings.try_bind(HoleId(0), Concept::int(1)));
    assert!(!bindings.try_bind(HoleId(0), Concept::int(2)));
    assert_eq!(bindings.get(HoleId(0)), Some(&Concept::int(1)));
}

#[test]
fn merge_returns_none_on_conflict() {
    let left: Bindings = vec![(HoleId(0), Concept::int(1))].into_iter().collect();
    let agrees: Bindings = vec![(HoleId(0), Concept::int(1)), (HoleId(1), Concept::int(9))]
        .into_iter()
        .collect();
    let conflicts: Bindings = vec![(HoleId(0), Concept::int(2))].into_iter().collect();

    let merged = left
        .merge(&agrees)
        .expect("compatible bindings should merge");
    assert_eq!(merged.len(), 2);
    assert_eq!(merged.get(HoleId(1)), Some(&Concept::int(9)));
    assert!(left.merge(&conflicts).is_none());
    assert_eq!(left.merge(&Bindings::new()), Some(left.clone()));
}

// ---- substitution ----

#[test]
fn substitute_positional_duplicates_one_argument() {
    // `double` is Add<Hole(0), Hole(0)>. Applied to [21] it is Add<21, 21>.
    let double = add([Concept::hole(0), Concept::hole(0)]);
    let applied = substitute_positional(&double, &[Concept::int(21)]);
    assert_eq!(applied, add([Concept::int(21), Concept::int(21)]));
    assert!(is_ground_term(&applied));
}

#[test]
fn substitute_positional_leaves_extra_holes_alone() {
    let body = add([Concept::hole(0), Concept::hole(1)]);
    let partial = substitute_positional(&body, &[Concept::int(7)]);
    assert_eq!(partial, add([Concept::int(7), Concept::hole(1)]));
    assert_eq!(substitute_positional(&body, &[]), body);
}

#[test]
fn unbound_holes_survive_substitution() {
    let pattern = add([Concept::hole(0), f([Concept::hole(1)])]);
    let bindings: Bindings = vec![(HoleId(0), Concept::named("greg"))]
        .into_iter()
        .collect();
    let filled = substitute(&pattern, &bindings);
    assert_eq!(filled, add([Concept::named("greg"), f([Concept::hole(1)])]));
    assert_eq!(
        holes(&filled).into_iter().collect::<Vec<_>>(),
        vec![HoleId(1)]
    );
}

#[test]
fn substitute_is_single_pass() {
    // A binding that mentions a hole does not get re-substituted.
    let pattern = Concept::hole(0);
    let bindings: Bindings = vec![(HoleId(0), f([Concept::hole(0)]))]
        .into_iter()
        .collect();
    assert_eq!(substitute(&pattern, &bindings), f([Concept::hole(0)]));
}

#[test]
fn substitute_with_nothing_to_do_shares_the_original() {
    let term = add([f([Concept::int(1)]), Concept::int(2)]);
    let bindings: Bindings = vec![(HoleId(4), Concept::int(0))].into_iter().collect();
    let same = substitute(&term, &bindings);
    assert_eq!(same, term);

    let (Concept::Compound { args: before, .. }, Concept::Compound { args: after, .. }) =
        (&term, &same)
    else {
        panic!("expected compounds");
    };
    assert!(Arc::ptr_eq(before, after), "unchanged term was rebuilt");
}

#[test]
fn rename_holes_avoids_collisions() {
    let left = f([Concept::hole(0), Concept::hole(1)]);
    let right = Concept::call("g", [Concept::hole(0)]);
    let offset = max_hole(&left).map(|h| h.0 + 1).unwrap_or(0);
    let shifted = rename_holes(&right, offset);

    assert_eq!(shifted, Concept::call("g", [Concept::hole(2)]));
    let overlap: Vec<HoleId> = holes(&left)
        .intersection(&holes(&shifted))
        .copied()
        .collect();
    assert!(
        overlap.is_empty(),
        "renamed holes still collide: {overlap:?}"
    );
    assert_eq!(rename_holes(&left, 0), left);
    assert_eq!(rename_holes(&Concept::int(1), 5), Concept::int(1));
}

// ---- structural comparison ----

#[test]
fn alpha_equivalent_accepts_consistent_renaming() {
    let a = f([Concept::hole(0), Concept::hole(1)]);
    let b = f([Concept::hole(3), Concept::hole(7)]);
    assert!(alpha_equivalent(&a, &b));
    assert!(alpha_equivalent(&b, &a));
    assert!(alpha_equivalent(&a, &a));
}

#[test]
fn alpha_equivalent_rejects_a_collapsed_renaming() {
    let distinct = f([Concept::hole(0), Concept::hole(1)]);
    let shared = f([Concept::hole(0), Concept::hole(0)]);
    assert!(!alpha_equivalent(&distinct, &shared));
    assert!(!alpha_equivalent(&shared, &distinct));
}

#[test]
fn alpha_equivalent_still_cares_about_structure() {
    assert!(!alpha_equivalent(
        &f([Concept::int(1)]),
        &f([Concept::int(2)])
    ));
    assert!(!alpha_equivalent(
        &f([Concept::hole(0)]),
        &f([Concept::hole(0), Concept::hole(1)])
    ));
    assert!(!alpha_equivalent(&Concept::hole(0), &Concept::int(1)));
    assert!(alpha_equivalent(
        &Concept::named("greg"),
        &Concept::named("greg")
    ));
}

#[test]
fn generalizes_binds_pattern_holes() {
    let pattern = Concept::call("friend-with", [Concept::hole(0), Concept::named("keal")]);
    let target = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let bindings = generalizes(&pattern, &target).expect("pattern should cover target");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings.get(HoleId(0)), Some(&Concept::named("greg")));
}

#[test]
fn generalizes_fails_on_structural_mismatch() {
    let pattern = Concept::call("friend-with", [Concept::hole(0), Concept::named("keal")]);
    assert!(
        generalizes(
            &pattern,
            &Concept::call("friend-with", [Concept::named("greg")])
        )
        .is_none(),
        "arity mismatch must not match"
    );
    assert!(
        generalizes(
            &pattern,
            &Concept::call(
                "works-with",
                [Concept::named("greg"), Concept::named("keal")]
            )
        )
        .is_none(),
        "different head must not match"
    );
    assert!(generalizes(&Concept::int(1), &Concept::int(2)).is_none());
}

#[test]
fn generalizes_fails_when_one_hole_needs_two_bindings() {
    let pattern = Concept::call("same", [Concept::hole(0), Concept::hole(0)]);
    assert!(
        generalizes(
            &pattern,
            &Concept::call("same", [Concept::named("greg"), Concept::named("greg")])
        )
        .is_some()
    );
    assert!(
        generalizes(
            &pattern,
            &Concept::call("same", [Concept::named("greg"), Concept::named("keal")])
        )
        .is_none()
    );
}

#[test]
fn generalizes_treats_target_holes_as_opaque() {
    // A pattern hole absorbs a target hole.
    let bindings = generalizes(&f([Concept::hole(0)]), &f([Concept::hole(3)])).unwrap();
    assert_eq!(bindings.get(HoleId(0)), Some(&Concept::hole(3)));
    // But nothing binds on the target side.
    assert!(generalizes(&f([Concept::named("greg")]), &f([Concept::hole(3)])).is_none());
}

#[test]
fn generalizes_round_trips_through_substitute() {
    let pattern = add([Concept::hole(0), f([Concept::hole(1), Concept::hole(0)])]);
    let target = add([
        Concept::int(5),
        f([Concept::named("greg"), Concept::int(5)]),
    ]);
    let bindings = generalizes(&pattern, &target).expect("should match");
    assert_eq!(substitute(&pattern, &bindings), target);
}

// ---- anti-unification ----

#[test]
fn anti_unify_keeps_shared_structure() {
    let left = add([Concept::int(1), Concept::int(2)]);
    let right = add([Concept::int(1), Concept::int(3)]);
    let (general, from_left, from_right) = anti_unify(&left, &right);

    assert_eq!(general, add([Concept::int(1), Concept::hole(0)]));
    assert_eq!(from_left.get(HoleId(0)), Some(&Concept::int(2)));
    assert_eq!(from_right.get(HoleId(0)), Some(&Concept::int(3)));
    assert_eq!(substitute(&general, &from_left), left);
    assert_eq!(substitute(&general, &from_right), right);
}

#[test]
fn anti_unify_reuses_a_hole_for_a_repeated_difference() {
    let left = Concept::call("pair", [Concept::int(7), Concept::int(7)]);
    let right = Concept::call("pair", [Concept::int(9), Concept::int(9)]);
    let (general, from_left, from_right) = anti_unify(&left, &right);

    assert_eq!(
        general,
        Concept::call("pair", [Concept::hole(0), Concept::hole(0)])
    );
    assert_eq!(from_left.len(), 1);
    assert_eq!(substitute(&general, &from_left), left);
    assert_eq!(substitute(&general, &from_right), right);
}

#[test]
fn anti_unify_of_identical_terms_introduces_no_holes() {
    let term = nested();
    let (general, from_left, from_right) = anti_unify(&term, &term);
    assert_eq!(general, term);
    assert!(from_left.is_empty());
    assert!(from_right.is_empty());
}

#[test]
fn anti_unify_falls_back_to_a_bare_hole() {
    // Different arity has no shared spine to keep.
    let left = f([Concept::int(1)]);
    let right = f([Concept::int(1), Concept::int(2)]);
    let (general, from_left, from_right) = anti_unify(&left, &right);
    assert_eq!(general, Concept::hole(0));
    assert_eq!(substitute(&general, &from_left), left);
    assert_eq!(substitute(&general, &from_right), right);
}

#[test]
fn anti_unify_does_not_capture_existing_holes() {
    let left = f([Concept::hole(5), Concept::int(1)]);
    let right = f([Concept::hole(5), Concept::int(2)]);
    let (general, from_left, from_right) = anti_unify(&left, &right);

    assert_eq!(general, f([Concept::hole(5), Concept::hole(6)]));
    assert_eq!(substitute(&general, &from_left), left);
    assert_eq!(substitute(&general, &from_right), right);
}

#[test]
fn anti_unify_generalizes_the_head_too() {
    let left = f([Concept::int(1)]);
    let right = Concept::call("g", [Concept::int(1)]);
    let (general, from_left, from_right) = anti_unify(&left, &right);
    assert_eq!(
        general,
        Concept::apply(Concept::hole(0), vec![Concept::int(1)])
    );
    assert_eq!(substitute(&general, &from_left), left);
    assert_eq!(substitute(&general, &from_right), right);
}

// ---- builders ----

#[test]
fn map_args_keeps_the_head() {
    let term = add([Concept::int(1), Concept::int(2)]);
    let doubled = map_args(&term, |arg| {
        match arg.as_ground().and_then(|g| g.as_i64()) {
            Some(v) => Concept::int(v * 2),
            None => arg.clone(),
        }
    });
    assert_eq!(doubled, add([Concept::int(2), Concept::int(4)]));

    // Non-compounds have no arguments, so they come back untouched.
    let greg = Concept::named("greg");
    assert_eq!(map_args(&greg, |_| Concept::int(0)), greg);
    assert_eq!(
        map_args(&Concept::hole(1), |_| Concept::int(0)),
        Concept::hole(1)
    );
}

#[test]
fn try_map_args_stops_at_the_first_error() {
    let term = add([Concept::int(1), Concept::named("greg"), Concept::int(3)]);
    let mut visited = 0usize;
    let result: Result<Concept, &'static str> = try_map_args(&term, |arg| {
        visited += 1;
        match arg.as_ground().and_then(|g| g.as_i64()) {
            Some(v) => Ok(Concept::int(v + 1)),
            None => Err("not an int"),
        }
    });
    assert_eq!(result, Err("not an int"));
    assert_eq!(visited, 2, "should not have evaluated the third argument");

    let ok: Result<Concept, &'static str> =
        try_map_args(&add([Concept::int(1)]), |arg| Ok(arg.clone()));
    assert_eq!(ok, Ok(add([Concept::int(1)])));
    let atom: Result<Concept, &'static str> =
        try_map_args(&Concept::named("greg"), |_| Err("never called"));
    assert_eq!(atom, Ok(Concept::named("greg")));
}

#[test]
fn flatten_spine_normalizes_partial_application() {
    // ((f a) b)
    let inner = Concept::apply(Concept::named("f"), vec![Concept::named("a")]);
    let outer = Concept::apply(inner, vec![Concept::named("b")]);

    let (head, args) = flatten_spine(&outer);
    assert_eq!(*head, Concept::named("f"));
    assert_eq!(
        args.into_iter().cloned().collect::<Vec<_>>(),
        vec![Concept::named("a"), Concept::named("b")]
    );
}

#[test]
fn flatten_spine_on_flat_and_atomic_terms() {
    let flat = add([Concept::int(1), Concept::int(2)]);
    let (head, args) = flatten_spine(&flat);
    assert_eq!(*head, Concept::named("add"));
    assert_eq!(args.len(), 2);

    let greg = Concept::named("greg");
    let (head, args) = flatten_spine(&greg);
    assert_eq!(*head, greg);
    assert!(args.is_empty());
}
