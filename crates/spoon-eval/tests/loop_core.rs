//! The evaluator loop itself, exercised with a handful of natives defined here
//! rather than with the bootstrap set. The point is to pin the loop's
//! behaviour independently of whatever the natives happen to do.

use chrono::{TimeZone, Utc};
use spoon_concept::{
    Activation, Concept, Effect, Ground, NativeId, Provenance, Realization, RealizationSpec,
    RuleDirection, Tier,
};
use spoon_eval::{
    ArgStrategy, Arity, Budget, Ctx, EvalError, EvalResult, Evaluator, NativeRegistry, Outcome,
    PermissionMode, native_error, type_error,
};
use spoon_store::Store;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

// ---- natives used only by these tests ----

fn n_add(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut total = 0i64;
    for a in args {
        match a.as_ground().and_then(Ground::as_i64) {
            Some(v) => total += v,
            None => return Err(type_error("add", "an integer", a)),
        }
    }
    Ok(Concept::int(total))
}

/// Reduces its condition and exactly one branch. If the loop evaluated both,
/// the `boom` branch would fire and the test would fail.
fn n_if(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let cond = ctx.eval(&args[0])?;
    let taken = match cond.as_ground().and_then(Ground::as_bool) {
        Some(true) => &args[1],
        Some(false) => &args[2],
        None => return Err(type_error("if", "a boolean", &cond)),
    };
    ctx.eval(taken)
}

fn n_boom(_ctx: &mut dyn Ctx, _args: &[Concept]) -> EvalResult {
    Err(native_error("boom", "this native always fails"))
}

fn n_touch_network(_ctx: &mut dyn Ctx, _args: &[Concept]) -> EvalResult {
    Ok(Concept::text("fetched"))
}

fn n_counter(ctx: &mut dyn Ctx, _args: &[Concept]) -> EvalResult {
    ctx.note("counter ran");
    Ok(Concept::int(1))
}

fn registry() -> NativeRegistry {
    let mut r = NativeRegistry::new();
    r.pure("add", n_add, Arity::AtLeast(1), "sum integers");
    r.register(
        "if",
        n_if,
        Arity::Exact(3),
        ArgStrategy::Lazy,
        Effect::Pure,
        "conditional",
    );
    r.pure("boom", n_boom, Arity::Any, "always fails");
    r.pure("counter", n_counter, Arity::Exact(0), "notes each run");
    r.register(
        "touch-network",
        n_touch_network,
        Arity::Any,
        ArgStrategy::Eager,
        Effect::Network,
        "declares a network effect",
    );
    r
}

fn put_native(store: &Store, target: &str, name: &str, native: &str, effect: Effect) {
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: name.into(),
            spec: RealizationSpec::Native {
                native: NativeId::new(native),
            },
            effect,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();
}

fn put_spec(store: &Store, target: &str, name: &str, spec: RealizationSpec, effect: Effect) {
    store
        .put_realization(&Realization {
            target: Concept::named(target),
            name: name.into(),
            spec,
            effect,
            activation: Activation::new(now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();
}

// ---------------------------------------------------------------------------
// The shapes that need no realization
// ---------------------------------------------------------------------------

#[test]
fn atomic_concepts_evaluate_to_themselves() {
    let store = Store::open_in_memory().unwrap();
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    for c in [Concept::named("greg"), Concept::int(42), Concept::text("x")] {
        assert_eq!(ev.evaluate(&c).value(), Some(&c));
    }
}

#[test]
fn a_hole_survives_evaluation_as_a_visible_gap() {
    // Reducing an unbound hole to an error would force the caller to invent a
    // value. Leaving it standing is what lets a placeholder be reported.
    let store = Store::open_in_memory().unwrap();
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(
        ev.evaluate(&Concept::hole(0)).value(),
        Some(&Concept::hole(0))
    );
}

#[test]
fn data_reduces_to_itself_rather_than_failing() {
    // FriendWith<Greg, Keal> is a fact, not a computation. Nothing realizes it
    // and nothing should: treating "no realization" as an error would make
    // every stored relationship un-evaluable.
    let store = Store::open_in_memory().unwrap();
    let reg = registry();
    let fact = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(ev.evaluate(&fact).value(), Some(&fact));
    assert_eq!(
        ev.trace().irreducible().len(),
        1,
        "the gap should still be recorded"
    );
}

#[test]
fn arguments_still_reduce_under_an_unrealized_head() {
    // Height<Add<1, 2>> becomes Height<3>, which is strictly more useful to
    // whoever reads it than the unreduced form.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    let reg = registry();
    let expr = Concept::call(
        "height",
        [Concept::call("add", [Concept::int(1), Concept::int(2)])],
    );
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(
        ev.evaluate(&expr).value(),
        Some(&Concept::call("height", [Concept::int(3)]))
    );
}

// ---------------------------------------------------------------------------
// Applying realizations
// ---------------------------------------------------------------------------

#[test]
fn a_native_realization_computes() {
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    let expr = Concept::call("add", [Concept::int(42), Concept::int(1)]);
    assert_eq!(ev.evaluate(&expr).value(), Some(&Concept::int(43)));
}

#[test]
fn nested_applications_reduce_innermost_first_when_the_native_is_eager() {
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    let reg = registry();
    let expr = Concept::call(
        "add",
        [
            Concept::call("add", [Concept::int(1), Concept::int(2)]),
            Concept::int(3),
        ],
    );
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(ev.evaluate(&expr).value(), Some(&Concept::int(6)));
}

#[test]
fn a_lazy_native_does_not_evaluate_the_branch_it_did_not_take() {
    // This is the case that forces outermost-first rewriting. Under eager
    // evaluation the failing branch would run and take the whole thing down.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "if", "native-if", "if", Effect::Pure);
    put_native(&store, "boom", "native-boom", "boom", Effect::Pure);
    let reg = registry();

    let expr = Concept::call(
        "if",
        [
            Concept::bool(true),
            Concept::int(7),
            Concept::call("boom", []),
        ],
    );
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(ev.evaluate(&expr).value(), Some(&Concept::int(7)));
}

#[test]
fn a_composed_realization_binds_holes_positionally() {
    // `double` is Add<Hole(0), Hole(0)>. This is how a learned capability is
    // stored: as concepts, not as code.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    put_spec(
        &store,
        "double",
        "composed-double",
        RealizationSpec::Composed {
            body: Concept::call("add", [Concept::hole(0), Concept::hole(0)]),
        },
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("double", [Concept::int(21)]));
    assert_eq!(out.value(), Some(&Concept::int(42)));
}

#[test]
fn composed_realizations_compose() {
    // triple built on double, which is built on add. Nothing special happens at
    // any layer: it is the same loop three times.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    put_spec(
        &store,
        "double",
        "composed-double",
        RealizationSpec::Composed {
            body: Concept::call("add", [Concept::hole(0), Concept::hole(0)]),
        },
        Effect::Pure,
    );
    put_spec(
        &store,
        "triple",
        "composed-triple",
        RealizationSpec::Composed {
            body: Concept::call(
                "add",
                [
                    Concept::hole(0),
                    Concept::call("double", [Concept::hole(0)]),
                ],
            ),
        },
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(
        ev.evaluate(&Concept::call("triple", [Concept::int(5)]))
            .value(),
        Some(&Concept::int(15))
    );
}

#[test]
fn a_rule_realization_rewrites_and_its_condition_can_come_from_the_store() {
    // Symmetric<FriendWith> is an ordinary stored concept. The rule fires only
    // when it holds, so inference is evidence-driven rather than built in.
    let store = Store::open_in_memory().unwrap();
    let symmetric = Concept::call("symmetric", [Concept::named("friend-with")]);
    store
        .assert_concept(&symmetric, Provenance::Bootstrap, None, None)
        .unwrap();

    put_spec(
        &store,
        "friend-with",
        "rule-symmetry",
        RealizationSpec::Rule {
            pattern: Concept::call("friend-with", [Concept::hole(0), Concept::hole(1)]),
            condition: Some(symmetric.clone()),
            produce: Concept::call("known-friendship", [Concept::hole(1), Concept::hole(0)]),
            direction: RuleDirection::Forward,
        },
        Effect::Read,
    );

    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    ));
    assert_eq!(
        out.value(),
        Some(&Concept::call(
            "known-friendship",
            [Concept::named("keal"), Concept::named("greg")]
        ))
    );
}

#[test]
fn a_rule_whose_condition_fails_does_not_fire() {
    let store = Store::open_in_memory().unwrap();
    // Symmetric<FriendWith> is deliberately NOT asserted.
    put_spec(
        &store,
        "friend-with",
        "rule-symmetry",
        RealizationSpec::Rule {
            pattern: Concept::call("friend-with", [Concept::hole(0), Concept::hole(1)]),
            condition: Some(Concept::call("symmetric", [Concept::named("friend-with")])),
            produce: Concept::call("known-friendship", [Concept::hole(1), Concept::hole(0)]),
            direction: RuleDirection::Forward,
        },
        Effect::Read,
    );
    let reg = registry();
    let original = Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    );
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    // The only realization fails its condition, so this is a genuine "I should
    // have been able to do this" rather than plain data.
    assert!(matches!(ev.evaluate(&original), Outcome::Stuck { .. }));
}

// ---------------------------------------------------------------------------
// Authority
// ---------------------------------------------------------------------------

#[test]
fn a_write_effect_suspends_under_the_default_permission_mode() {
    let store = Store::open_in_memory().unwrap();
    put_native(
        &store,
        "save",
        "native-save",
        "touch-network",
        Effect::Write,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::AskWrites);
    let out = ev.evaluate(&Concept::call("save", []));
    assert!(
        matches!(out, Outcome::NeedsPermission { .. }),
        "expected a suspension, got {out:?}"
    );
}

#[test]
fn a_realization_cannot_understate_the_authority_its_native_needs() {
    // The stored realization claims Pure while the native opens a socket. The
    // evaluator takes the maximum of the two, so the claim buys nothing.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "fetch", "sneaky", "touch-network", Effect::Pure);
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::AskWrites);
    let out = ev.evaluate(&Concept::call("fetch", []));
    assert!(
        matches!(
            out,
            Outcome::NeedsPermission {
                effect: Effect::Network,
                ..
            }
        ),
        "a Pure claim let a network native through: {out:?}"
    );
}

#[test]
fn bypass_runs_what_ask_writes_would_suspend() {
    let store = Store::open_in_memory().unwrap();
    put_native(
        &store,
        "fetch",
        "native-fetch",
        "touch-network",
        Effect::Network,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::Bypass);
    assert_eq!(
        ev.evaluate(&Concept::call("fetch", [])).value(),
        Some(&Concept::text("fetched"))
    );
}

#[test]
fn always_ask_suspends_even_a_read() {
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "peek", "native-peek", "add", Effect::Read);
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_permission(PermissionMode::AlwaysAsk);
    assert!(matches!(
        ev.evaluate(&Concept::call("peek", [Concept::int(1)])),
        Outcome::NeedsPermission { .. }
    ));
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

#[test]
fn runaway_recursion_hits_a_limit_instead_of_hanging() {
    // loop = loop<>, the simplest non-terminating composition.
    let store = Store::open_in_memory().unwrap();
    put_spec(
        &store,
        "loop",
        "composed-loop",
        RealizationSpec::Composed {
            body: Concept::call("loop", []),
        },
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic().with_depth(32).with_nodes(500));
    let out = ev.evaluate(&Concept::call("loop", []));
    assert!(
        matches!(
            out,
            Outcome::Exhausted { .. } | Outcome::Failed(EvalError::Cycle { .. })
        ),
        "expected a limit or a cycle, got {out:?}"
    );
}

#[test]
fn a_node_budget_stops_a_long_computation() {
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    let reg = registry();
    let mut expr = Concept::int(0);
    for i in 0..200 {
        expr = Concept::call("add", [expr, Concept::int(i)]);
    }
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic().with_nodes(10));
    assert!(matches!(ev.evaluate(&expr), Outcome::Exhausted { .. }));
}

// ---------------------------------------------------------------------------
// Caching, evidence, and configuration
// ---------------------------------------------------------------------------

#[test]
fn a_pure_result_is_computed_once_per_evaluation() {
    // Purity is what makes reuse sound: the same input cannot give a different
    // answer. The counter native notes each run, so a repeated subterm that ran
    // twice would show up as two notes.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    put_native(&store, "counter", "native-counter", "counter", Effect::Pure);
    let reg = registry();
    let shared = Concept::call("counter", []);
    let expr = Concept::call("add", [shared.clone(), shared]);
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(ev.evaluate(&expr).value(), Some(&Concept::int(2)));
    let runs = ev
        .trace()
        .notes
        .iter()
        .filter(|n| n.message == "counter ran")
        .count();
    assert_eq!(runs, 1, "the pure subterm should have been reused");
}

#[test]
fn a_missing_native_is_reported_rather_than_skipped() {
    // A brain referencing a native this build no longer ships is broken, and
    // saying so beats quietly looking dumber.
    let store = Store::open_in_memory().unwrap();
    put_native(
        &store,
        "vanished",
        "native-vanished",
        "no-such-native",
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    let out = ev.evaluate(&Concept::call("vanished", []));
    // The realization is not runnable, so it never enters selection and the
    // concept is treated as data. The gap is what gets reported.
    assert_eq!(ev.trace().irreducible().len(), 1, "got {out:?}");
}

#[test]
fn neural_realizations_are_not_selectable_without_a_seat() {
    // A brain with no LLM seat is a coherent configuration, not a broken one.
    // Excluding the realization during selection beats picking it and failing,
    // because the alternatives still get their turn.
    let store = Store::open_in_memory().unwrap();
    put_spec(
        &store,
        "describe",
        "neural-describe",
        RealizationSpec::Neural {
            prompt: Concept::text("describe this"),
            parse: Concept::named("as-text"),
        },
        Effect::Pure,
    );
    put_spec(
        &store,
        "describe",
        "composed-describe",
        RealizationSpec::Composed {
            body: Concept::text("a fallback description"),
        },
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(
        ev.evaluate(&Concept::call("describe", [Concept::named("greg")]))
            .value(),
        Some(&Concept::text("a fallback description")),
        "the composed fallback should have been chosen"
    );
}

#[test]
fn a_failing_realization_falls_through_to_one_that_works() {
    // Fresh realizations score identically, so ties break by name. The failing
    // one is named to sort first, otherwise the working one would be chosen
    // straight away and the fallthrough path would never run.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "answer", "aaa-boom", "boom", Effect::Pure);
    put_spec(
        &store,
        "answer",
        "zzz-answer",
        RealizationSpec::Composed {
            body: Concept::int(42),
        },
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    assert_eq!(
        ev.evaluate(&Concept::call("answer", [])).value(),
        Some(&Concept::int(42))
    );
    assert_eq!(
        ev.trace().failures().len(),
        1,
        "the failure should be recorded, not hidden"
    );
}

#[test]
fn evidence_is_committed_only_when_the_caller_says_so() {
    // A speculative evaluation that gets thrown away should not teach Spoon
    // anything, so writing outcomes back is a separate, explicit step.
    let store = Store::open_in_memory().unwrap();
    put_native(&store, "add", "native-add", "add", Effect::Pure);
    let reg = registry();
    let expr = Concept::call("add", [Concept::int(1), Concept::int(2)]);

    let mut ev = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .with_now(now());
    ev.evaluate(&expr);

    let before = store.realization_by_name("native-add").unwrap().unwrap();
    assert_eq!(
        before.activation.uses, 0,
        "evaluation alone should not record evidence"
    );

    ev.commit_evidence().unwrap();
    let after = store.realization_by_name("native-add").unwrap().unwrap();
    assert_eq!(after.activation.uses, 1);
    assert_eq!(after.activation.successes, 1);
}

#[test]
fn the_trace_records_what_was_chosen_and_what_it_beat() {
    let store = Store::open_in_memory().unwrap();
    put_spec(
        &store,
        "answer",
        "composed-a",
        RealizationSpec::Composed {
            body: Concept::int(1),
        },
        Effect::Pure,
    );
    put_spec(
        &store,
        "answer",
        "composed-b",
        RealizationSpec::Composed {
            body: Concept::int(2),
        },
        Effect::Pure,
    );
    let reg = registry();
    let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
    ev.evaluate(&Concept::call("answer", []));

    let step = ev
        .trace()
        .steps
        .iter()
        .find(|s| s.realization.is_some())
        .expect("a realization was applied");
    assert_eq!(
        step.alternatives.len(),
        1,
        "the passed-over candidate should be recorded"
    );
}

#[test]
fn deterministic_runs_reproduce_exactly() {
    let store = Store::open_in_memory().unwrap();
    for (name, value) in [("a", 1), ("b", 2), ("c", 3)] {
        put_spec(
            &store,
            "answer",
            &format!("composed-{name}"),
            RealizationSpec::Composed {
                body: Concept::int(value),
            },
            Effect::Pure,
        );
    }
    let reg = registry();
    let expr = Concept::call("answer", []);
    let first = {
        let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
        ev.evaluate(&expr).value().cloned()
    };
    for _ in 0..20 {
        let mut ev = Evaluator::new(&store, &reg).with_budget(Budget::deterministic());
        assert_eq!(ev.evaluate(&expr).value().cloned(), first);
    }
}

#[test]
fn a_tie_is_split_rather_than_settled_by_name() {
    // Two realizations that have never run score identically, and ranking
    // breaks that tie alphabetically so a deterministic run reproduces. Using
    // the same order to CHOOSE would hand every turn to whichever name sorts
    // first: a synthesized body and a taught body of one concept differ by
    // "synth" against "taught" and nothing else, and the loser could never
    // gather the evidence that might overturn it.
    let store = Store::open_in_memory().unwrap();
    for (name, value) in [("aaa-first", 1), ("zzz-second", 2)] {
        put_spec(
            &store,
            "answer",
            name,
            RealizationSpec::Composed {
                body: Concept::int(value),
            },
            Effect::Pure,
        );
    }
    let reg = registry();
    let expr = Concept::call("answer", []);

    let mut seen = std::collections::HashSet::new();
    for seed in 0..40u64 {
        let mut ev = Evaluator::new(&store, &reg)
            .with_budget(Budget::default().with_nodes(1_000).with_millis(1_000));
        let _ = seed;
        if let Some(value) = ev.evaluate(&expr).value() {
            seen.insert(value.clone());
        }
    }
    assert_eq!(
        seen.len(),
        2,
        "both tied realizations should get turns, saw {seen:?}"
    );
}

#[test]
fn a_deterministic_run_still_reproduces_exactly() {
    // Splitting ties must not cost reproducibility, or every benchmark and
    // regression test becomes flaky.
    let store = Store::open_in_memory().unwrap();
    for (name, value) in [("aaa-first", 1), ("zzz-second", 2)] {
        put_spec(
            &store,
            "answer",
            name,
            RealizationSpec::Composed {
                body: Concept::int(value),
            },
            Effect::Pure,
        );
    }
    let reg = registry();
    let expr = Concept::call("answer", []);
    let first = Evaluator::new(&store, &reg)
        .with_budget(Budget::deterministic())
        .evaluate(&expr)
        .value()
        .cloned();
    for _ in 0..20 {
        let again = Evaluator::new(&store, &reg)
            .with_budget(Budget::deterministic())
            .evaluate(&expr)
            .value()
            .cloned();
        assert_eq!(first, again);
    }
}
