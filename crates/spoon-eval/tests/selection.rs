//! How a realization arriving late ever displaces one already established.
//!
//! This is the question the whole "realizations compete" design rests on, and
//! it is easy to get wrong in a way nothing surfaces: a scoring rule that
//! quietly makes a newcomer ineligible produces a system that looks like it is
//! learning and has actually frozen.

use chrono::{DateTime, Duration, TimeZone, Utc};
use spoon_concept::{
    Activation, Concept, Effect, NativeId, Provenance, Realization, RealizationSpec, Tier,
};
use spoon_eval::{Rng, score};
use std::sync::Arc;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap()
}

fn with_record(name: &str, uses: u64, wins: u64) -> Arc<Realization> {
    let now = now();
    let mut activation = Activation::new(now - Duration::days(30));
    for i in 0..uses {
        activation.record(now - Duration::minutes(i as i64), i < wins);
    }
    Arc::new(Realization {
        target: Concept::named("target"),
        name: name.into(),
        spec: RealizationSpec::Native {
            native: NativeId::new("x"),
        },
        effect: Effect::Pure,
        activation,
        provenance: Provenance::Bootstrap,
        tier: Tier::Kernel,
    })
}

/// How often the challenger outscores the incumbent, over many draws.
fn challenger_share(incumbent: (u64, u64), challenger: (u64, u64)) -> f64 {
    let mut rng = Rng::new(0xABCD);
    let mut wins = 0;
    const TRIALS: usize = 20_000;
    for _ in 0..TRIALS {
        let a = score(
            with_record("incumbent", incumbent.0, incumbent.1),
            0.5,
            now(),
            Some(&mut rng),
        );
        let b = score(
            with_record("challenger", challenger.0, challenger.1),
            0.5,
            now(),
            Some(&mut rng),
        );
        if b.score > a.score {
            wins += 1;
        }
    }
    wins as f64 / TRIALS as f64
}

#[test]
fn an_untried_realization_gets_a_fair_share_of_turns() {
    // The failure this guards against: a newcomer scoring so far below a proven
    // incumbent that it is not merely ranked last but ineligible, so it can
    // never earn the evidence that would let it compete. That state is
    // invisible from outside, because the system keeps answering correctly with
    // the old realization while having silently stopped learning.
    let share = challenger_share((50, 45), (0, 0));
    assert!(
        share > 0.03,
        "an untried realization got {:.1}% of turns against a proven one, \
         which is too few to ever learn anything about it",
        share * 100.0
    );
    assert!(
        share < 0.35,
        "an untried realization got {:.1}% of turns, which is too many: it has \
         earned nothing and is displacing something that works",
        share * 100.0
    );
}

#[test]
fn evidence_moves_the_share_in_the_right_direction() {
    let untried = challenger_share((50, 45), (0, 0));
    let promising = challenger_share((50, 45), (3, 3));
    let proven = challenger_share((50, 45), (10, 10));
    assert!(
        untried < promising && promising < proven,
        "share should rise with evidence, got {untried:.3} then {promising:.3} then {proven:.3}"
    );
}

#[test]
fn a_realization_that_keeps_failing_is_abandoned() {
    // The other half. Trying a newcomer is only reasonable if a bad one stops
    // being tried, or exploration becomes a permanent tax.
    let share = challenger_share((50, 45), (10, 2));
    assert!(
        share < 0.02,
        "a failing realization still got {:.1}% of turns",
        share * 100.0
    );
}

#[test]
fn genuinely_better_wins() {
    let share = challenger_share((50, 45), (40, 38));
    assert!(
        share > 0.6,
        "a realization with a better record only got {:.1}% of turns",
        share * 100.0
    );
}

#[test]
fn frequency_of_use_is_not_counted_twice() {
    // Two realizations with the same success rate and very different usage
    // counts should be close to even. Multiplying the score by activation made
    // the busier one win outright, which is double-counting: how often
    // something has run is already what makes its posterior tight.
    let share = challenger_share((100, 90), (10, 9));
    assert!(
        (0.2..0.8).contains(&share),
        "same rate, different volume gave {:.1}% of turns, so volume is being \
         counted on top of the confidence it already produces",
        share * 100.0
    );
}

#[test]
fn scoring_without_a_generator_is_the_posterior_mean() {
    // Deterministic runs and anything displaying a number want a stable value,
    // not a draw that changes on every look.
    let realization = with_record("stable", 50, 45);
    let first = score(realization.clone(), 0.5, now(), None).score;
    for _ in 0..20 {
        assert_eq!(score(realization.clone(), 0.5, now(), None).score, first);
    }
}
