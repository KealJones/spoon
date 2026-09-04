//! The index has exactly one correctness obligation: never lose a rule.
//!
//! Over-approximating costs one failed unification. Under-approximating makes a
//! rule dead. It can never fire, no error is raised, and nothing indicates why
//! the answer is missing. Most of this file is about proving that does not
//! happen.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use spoon_concept::{Activation, Concept, RuleDirection, Tier};
use spoon_infer::{DiscriminationTree, RuleIndex, ScanIndex, StoredRule, unify_terms};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn h(n: u32) -> Concept {
    Concept::hole(n)
}

fn make_rule(name: &str, produce: Concept) -> Arc<StoredRule> {
    Arc::new(StoredRule {
        name: name.into(),
        target: Concept::named("t"),
        pattern: Concept::call("antecedent", [h(0)]),
        condition: None,
        produce,
        direction: RuleDirection::Backward,
        activation: Activation::new(now()),
        tier: Tier::Kernel,
    })
}

/// Rules that genuinely unify with the goal. This is the ground truth the index
/// is measured against, computed the slow honest way.
fn truly_matching(rules: &[Arc<StoredRule>], goal: &Concept) -> BTreeSet<String> {
    rules
        .iter()
        .filter(|r| unify_terms(goal, &r.produce).is_some())
        .map(|r| r.name.to_string())
        .collect()
}

fn names(rules: &[Arc<StoredRule>]) -> BTreeSet<String> {
    rules.iter().map(|r| r.name.to_string()).collect()
}

// ---------------------------------------------------------------------------
// The obligation
// ---------------------------------------------------------------------------

#[test]
fn the_tree_never_loses_a_rule_that_would_have_unified() {
    // The differential property, over a generated corpus. A hand-picked set of
    // cases cannot give confidence here; the failure mode is a shape nobody
    // thought to write down.
    let mut rng = Gen(0x1234_5678_9ABC_DEF0);
    let rules: Vec<Arc<StoredRule>> = (0..400)
        .map(|i| make_rule(&format!("r{i:04}"), rng.term(3)))
        .collect();

    let tree = DiscriminationTree::new(rules.clone());
    let scan = ScanIndex::new(rules.clone());

    let mut checked_nonempty = 0;
    for _ in 0..400 {
        let goal = rng.term(3);
        let truth = truly_matching(&rules, &goal);
        let from_tree = names(&tree.candidates(&goal));
        let from_scan = names(&scan.candidates(&goal));

        assert!(
            truth.is_subset(&from_tree),
            "tree lost {:?} for goal {goal:?}",
            truth.difference(&from_tree).collect::<Vec<_>>()
        );
        assert!(
            truth.is_subset(&from_scan),
            "scan lost {:?} for goal {goal:?}",
            truth.difference(&from_scan).collect::<Vec<_>>()
        );
        if !truth.is_empty() {
            checked_nonempty += 1;
        }
    }
    assert!(
        checked_nonempty > 20,
        "the corpus produced almost no matches ({checked_nonempty}), so it proved little"
    );
}

#[test]
fn the_tree_agrees_with_the_scan_on_hand_written_shapes() {
    let rules = vec![
        make_rule("plain", Concept::call("owns", [h(0), h(1)])),
        make_rule(
            "ground",
            Concept::call("owns", [Concept::named("greg"), Concept::named("dog")]),
        ),
        make_rule(
            "mixed",
            Concept::call("owns", [h(0), Concept::named("dog")]),
        ),
        make_rule("other-head", Concept::call("friend-with", [h(0), h(1)])),
        make_rule("wrong-arity", Concept::call("owns", [h(0)])),
        make_rule("hole-head", Concept::apply(h(0), vec![h(1), h(2)])),
        make_rule(
            "nested",
            Concept::call("stated", [h(0), Concept::call("is-sad", [h(1)])]),
        ),
        make_rule("int-404", Concept::call("status", [Concept::int(404)])),
        make_rule("int-500", Concept::call("status", [Concept::int(500)])),
    ];
    let tree = DiscriminationTree::new(rules.clone());

    let goals = [
        Concept::call("owns", [Concept::named("greg"), Concept::named("dog")]),
        Concept::call("owns", [h(0), Concept::named("dog")]),
        Concept::call("owns", [Concept::named("keal"), Concept::named("cat")]),
        Concept::call("friend-with", [Concept::named("a"), Concept::named("b")]),
        Concept::call("status", [Concept::int(404)]),
        Concept::call("status", [Concept::int(500)]),
        Concept::call("status", [h(0)]),
        Concept::call(
            "stated",
            [
                Concept::named("keal"),
                Concept::call("is-sad", [Concept::named("greg")]),
            ],
        ),
        Concept::call("owns", [h(0), h(1)]),
    ];
    for goal in goals {
        let truth = truly_matching(&rules, &goal);
        let got = names(&tree.candidates(&goal));
        assert!(
            truth.is_subset(&got),
            "lost {:?} for {goal:?}",
            truth.difference(&got).collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// The shapes that are easy to get wrong
// ---------------------------------------------------------------------------

#[test]
fn a_hole_headed_conclusion_is_offered_for_every_goal() {
    // ?0<?1, ?2> is how Symmetric applies to every relation at once. An index
    // that filters it out by head silently kills the entire meta-vocabulary.
    let rules = vec![
        make_rule("quantified", Concept::apply(h(0), vec![h(1), h(2)])),
        make_rule("specific", Concept::call("owns", [h(0), h(1)])),
    ];
    let tree = DiscriminationTree::new(rules);
    for goal in [
        Concept::call("friend-with", [Concept::named("a"), Concept::named("b")]),
        Concept::call("married-to", [Concept::named("a"), Concept::named("b")]),
        Concept::call("owns", [Concept::named("a"), Concept::named("b")]),
    ] {
        let got = names(&tree.candidates(&goal));
        assert!(
            got.contains("quantified"),
            "lost the quantified rule for {goal:?}"
        );
    }
}

#[test]
fn ground_values_discriminate() {
    // A goal about 500 should not drag in the 404 rule. Ground identity is the
    // value itself, so the index has something real to key on.
    let rules = vec![
        make_rule("http-404", Concept::call("meaning", [Concept::int(404)])),
        make_rule("http-500", Concept::call("meaning", [Concept::int(500)])),
    ];
    let tree = DiscriminationTree::new(rules);
    let got = names(&tree.candidates(&Concept::call("meaning", [Concept::int(500)])));
    assert!(got.contains("http-500"));
    assert!(
        !got.contains("http-404"),
        "the index did not discriminate on value"
    );
}

#[test]
fn a_goal_with_holes_still_reaches_ground_conclusions() {
    // Asking "what does some status mean" has to find the rule about 404, or a
    // question with a variable in it can never be answered.
    let rules = vec![make_rule(
        "http-404",
        Concept::call("meaning", [Concept::int(404)]),
    )];
    let tree = DiscriminationTree::new(rules);
    let got = names(&tree.candidates(&Concept::call("meaning", [h(0)])));
    assert!(
        got.contains("http-404"),
        "a variable goal lost a ground rule"
    );
}

#[test]
fn head_and_arity_both_discriminate() {
    let rules = vec![
        make_rule("binary", Concept::call("owns", [h(0), h(1)])),
        make_rule("unary", Concept::call("owns", [h(0)])),
        make_rule("elsewhere", Concept::call("knows", [h(0), h(1)])),
    ];
    let tree = DiscriminationTree::new(rules);
    let got = names(&tree.candidates(&Concept::call("owns", [h(0), h(1)])));
    assert!(got.contains("binary"));
    assert!(
        !got.contains("elsewhere"),
        "a different head was not excluded"
    );
}

#[test]
fn nesting_is_matched_through_depth() {
    let rules = vec![
        make_rule(
            "sad",
            Concept::call("stated", [h(0), Concept::call("is-sad", [h(1)])]),
        ),
        make_rule(
            "happy",
            Concept::call("stated", [h(0), Concept::call("is-happy", [h(1)])]),
        ),
    ];
    let tree = DiscriminationTree::new(rules);
    let goal = Concept::call(
        "stated",
        [
            Concept::named("keal"),
            Concept::call("is-sad", [Concept::named("greg")]),
        ],
    );
    let got = names(&tree.candidates(&goal));
    assert!(got.contains("sad"));
    assert!(
        !got.contains("happy"),
        "nested structure did not discriminate"
    );
}

// ---------------------------------------------------------------------------
// It has to earn its complexity
// ---------------------------------------------------------------------------

#[test]
fn the_tree_is_selective_where_the_scan_is_not() {
    // The scan already filters on head symbol and arity, so a corpus with many
    // distinct heads flatters it and proves nothing. The case that separates
    // them is many rules sharing one head and differing deeper in the term,
    // which is exactly what a brain full of rules about one relation looks
    // like. If the tree cannot beat the scan there, it is complexity with no
    // payoff and should be deleted.
    let mut rules = Vec::new();
    for i in 0..2000 {
        rules.push(make_rule(
            &format!("r{i:05}"),
            Concept::call(
                "owns",
                [
                    Concept::named(&format!("person{}", i % 100)),
                    Concept::int(i as i64 % 20),
                ],
            ),
        ));
    }
    let tree = DiscriminationTree::new(rules.clone());
    let scan = ScanIndex::new(rules.clone());

    let goal = Concept::call("owns", [Concept::named("person7"), Concept::int(7)]);
    let from_tree = tree.candidates(&goal);
    let from_scan = scan.candidates(&goal);

    assert!(
        truly_matching(&rules, &goal).is_subset(&names(&from_tree)),
        "selectivity came at the cost of correctness"
    );
    assert_eq!(
        from_scan.len(),
        rules.len(),
        "the scan should be returning everything here, or this corpus is not testing what it claims"
    );
    assert!(
        from_tree.len() * 20 < rules.len(),
        "tree returned {} of {} rules, which is not selective enough to be worth it",
        from_tree.len(),
        rules.len()
    );
}

#[test]
fn retrieval_examines_far_fewer_rules_than_it_holds() {
    let mut rules = Vec::new();
    for i in 0..2000 {
        rules.push(make_rule(
            &format!("r{i:05}"),
            Concept::call(&format!("rel{}", i % 200), [h(0)]),
        ));
    }
    let tree = DiscriminationTree::new(rules);
    let (_, stats) = tree.candidates_with_stats(&Concept::call("rel3", [Concept::named("x")]));
    assert!(
        stats.rules_examined * 10 < 2000,
        "examined {} of 2000 rules, which is barely better than scanning",
        stats.rules_examined
    );
}

// ---------------------------------------------------------------------------
// Housekeeping
// ---------------------------------------------------------------------------

#[test]
fn ordering_does_not_depend_on_insertion_order() {
    // Two brains holding the same rules must try them in the same order, or
    // derivations stop being reproducible.
    let mut rules: Vec<Arc<StoredRule>> = (0..30)
        .map(|i| make_rule(&format!("r{i:03}"), Concept::call("owns", [h(0), h(1)])))
        .collect();
    let forward = DiscriminationTree::new(rules.clone());
    rules.reverse();
    let backward = DiscriminationTree::new(rules);

    let goal = Concept::call("owns", [Concept::named("a"), Concept::named("b")]);
    let a: Vec<String> = forward
        .candidates(&goal)
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    let b: Vec<String> = backward
        .candidates(&goal)
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    assert_eq!(a, b, "candidate order followed insertion order");
    assert_eq!(forward.all().len(), backward.all().len());
}

#[test]
fn a_rule_inserted_later_is_retrievable() {
    // A rule learned mid-session has to work without rebuilding the index.
    let mut tree = DiscriminationTree::new(Vec::new());
    assert!(tree.is_empty());
    let rule = make_rule("late", Concept::call("owns", [h(0), h(1)]));
    tree.insert(rule);
    let got = names(&tree.candidates(&Concept::call(
        "owns",
        [Concept::named("a"), Concept::named("b")],
    )));
    assert!(got.contains("late"));
    assert_eq!(tree.len(), 1);
}

#[test]
fn degenerate_rule_sets_behave() {
    let empty = DiscriminationTree::new(Vec::new());
    assert!(empty.candidates(&Concept::call("owns", [h(0)])).is_empty());

    let single = DiscriminationTree::new(vec![make_rule("only", Concept::call("owns", [h(0)]))]);
    assert_eq!(
        single
            .candidates(&Concept::call("owns", [Concept::named("x")]))
            .len(),
        1
    );

    // Every conclusion identical: the tree degenerates to a list, and must
    // still return all of them rather than collapsing duplicates.
    let identical: Vec<Arc<StoredRule>> = (0..20)
        .map(|i| make_rule(&format!("dup{i:02}"), Concept::call("owns", [h(0)])))
        .collect();
    let tree = DiscriminationTree::new(identical);
    assert_eq!(
        tree.candidates(&Concept::call("owns", [Concept::named("x")]))
            .len(),
        20
    );
}

/// Seeded generator, so a failure is reproducible.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn term(&mut self, depth: u32) -> Concept {
        let choice = if depth == 0 {
            self.pick(4)
        } else {
            self.pick(6)
        };
        match choice {
            0 => Concept::named(["greg", "keal", "dog", "cat"][self.pick(4)]),
            1 => Concept::int(self.pick(6) as i64),
            2 => h(self.pick(3) as u32),
            3 => Concept::text(["a", "b"][self.pick(2)]),
            4 => {
                let arity = 1 + self.pick(3);
                let args: Vec<Concept> = (0..arity).map(|_| self.term(depth - 1)).collect();
                Concept::call(["owns", "knows", "friend-with"][self.pick(3)], args)
            }
            _ => {
                // A compound with a hole head, which is the shape that must not
                // be filtered out.
                let args: Vec<Concept> = (0..=self.pick(2)).map(|_| self.term(depth - 1)).collect();
                Concept::apply(h(self.pick(3) as u32), args)
            }
        }
    }
}
