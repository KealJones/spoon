//! Worked examples survive a restart, and evidence moves what they are worth.

use spoon_concept::Concept;
use spoon_store::pairs::{Pair, PairSource};
use spoon_store::{Store, StoreError};

fn double(n: i64) -> Vec<Concept> {
    vec![Concept::call(
        "do",
        [Concept::call("double", [Concept::int(n)])],
    )]
}

fn greet() -> Vec<Concept> {
    vec![Concept::call("chat", [Concept::call("greet", [])])]
}

fn named(pairs: &[Pair], utterance: &str) -> Pair {
    pairs
        .iter()
        .find(|p| p.utterance == utterance)
        .unwrap_or_else(|| panic!("no pair for {utterance:?}"))
        .clone()
}

#[test]
fn a_pair_round_trips_through_the_store() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .put_pair("can u double 21 for me", &double(21), PairSource::Model)
        .unwrap();

    let pairs = store.all_pairs(10).unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].id, id);
    assert_eq!(pairs[0].utterance, "can u double 21 for me");
    assert_eq!(pairs[0].steps, double(21));
    assert_eq!(pairs[0].source, PairSource::Model);
    assert_eq!(pairs[0].successes, 0);
    assert_eq!(pairs[0].failures, 0);
    assert_eq!(store.count_pairs().unwrap(), 1);
}

#[test]
fn pairs_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");

    let id = {
        let store = Store::open(&path).unwrap();
        let id = store
            .put_pair("can u double 21 for me", &double(21), PairSource::Model)
            .unwrap();
        store.put_pair("hey", &greet(), PairSource::Seed).unwrap();
        store.record_pair_outcome(id, true).unwrap();
        store.record_pair_outcome(id, true).unwrap();
        store.record_pair_outcome(id, false).unwrap();
        id
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), spoon_store::SCHEMA_VERSION);
    assert_eq!(store.count_pairs().unwrap(), 2);

    let pairs = store.all_pairs(10).unwrap();
    let reopened = named(&pairs, "can u double 21 for me");
    assert_eq!(reopened.id, id);
    assert_eq!(reopened.steps, double(21));
    assert_eq!(reopened.successes, 2);
    assert_eq!(reopened.failures, 1);
    assert_eq!(named(&pairs, "hey").source, PairSource::Seed);
}

/// The same sentence read two different ways is two pairs, so the readings can
/// compete on evidence instead of one silently overwriting the other.
#[test]
fn the_same_utterance_read_two_ways_is_two_pairs() {
    let store = Store::open_in_memory().unwrap();
    let first = store
        .put_pair("double 21", &double(21), PairSource::Model)
        .unwrap();
    let second = store
        .put_pair("double 21", &greet(), PairSource::Model)
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(store.count_pairs().unwrap(), 2);
}

/// Hearing the same reading twice is not new evidence, so nothing moves.
#[test]
fn storing_the_same_pair_twice_is_idempotent() {
    let store = Store::open_in_memory().unwrap();
    let first = store
        .put_pair("double 21", &double(21), PairSource::Model)
        .unwrap();
    store.record_pair_outcome(first, true).unwrap();
    let again = store
        .put_pair("double 21", &double(21), PairSource::Model)
        .unwrap();

    assert_eq!(first, again);
    assert_eq!(store.count_pairs().unwrap(), 1);
    assert_eq!(store.all_pairs(10).unwrap()[0].successes, 1);
}

/// A model guess the user later confirms should stop being ranked as a guess,
/// and confirmation must never be downgraded back to a guess.
#[test]
fn a_confirmed_reading_upgrades_a_model_one() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .put_pair("double 21", &double(21), PairSource::Model)
        .unwrap();
    store
        .put_pair("double 21", &double(21), PairSource::Confirmed)
        .unwrap();
    assert_eq!(
        store.all_pairs(10).unwrap()[0].source,
        PairSource::Confirmed
    );

    store
        .put_pair("double 21", &double(21), PairSource::Model)
        .unwrap();
    let pairs = store.all_pairs(10).unwrap();
    assert_eq!(pairs[0].id, id);
    assert_eq!(pairs[0].source, PairSource::Confirmed);
}

/// Where a reading came from bounds what it is worth before any evidence
/// arrives, and a fresh pair sits at exactly that prior.
#[test]
fn source_sets_the_starting_standing() {
    assert!(PairSource::Confirmed.prior() > PairSource::Seed.prior());
    assert!(PairSource::Seed.prior() > PairSource::Model.prior());

    let store = Store::open_in_memory().unwrap();
    store
        .put_pair("guessed", &double(1), PairSource::Model)
        .unwrap();
    store
        .put_pair("shipped", &double(2), PairSource::Seed)
        .unwrap();
    store
        .put_pair("approved", &double(3), PairSource::Confirmed)
        .unwrap();

    let pairs = store.all_pairs(10).unwrap();
    let order: Vec<&str> = pairs.iter().map(|p| p.utterance.as_str()).collect();
    assert_eq!(order, ["approved", "shipped", "guessed"]);
    assert_eq!(
        named(&pairs, "guessed").standing(),
        PairSource::Model.prior()
    );
}

/// A phrasing that keeps producing bad readings has to be able to lose, or one
/// bad generalization poisons the native path forever.
#[test]
fn repeated_failure_costs_a_pair_its_ranking() {
    let store = Store::open_in_memory().unwrap();
    let confirmed = store
        .put_pair("approved", &double(1), PairSource::Confirmed)
        .unwrap();
    let guessed = store
        .put_pair("guessed", &double(2), PairSource::Model)
        .unwrap();

    let started = store.all_pairs(10).unwrap();
    assert_eq!(started[0].id, confirmed, "the confirmed pair starts ahead");

    for _ in 0..4 {
        store.record_pair_outcome(confirmed, false).unwrap();
    }
    for _ in 0..4 {
        store.record_pair_outcome(guessed, true).unwrap();
    }

    let ranked = store.all_pairs(10).unwrap();
    assert_eq!(
        ranked[0].id, guessed,
        "four failures against four successes flips the order"
    );
    assert!(named(&ranked, "approved").standing() < named(&ranked, "guessed").standing());
    // Four failures roughly halve a confirmed pair: enough to stop it winning,
    // and enough to put any reading built from it under the ears' threshold.
    assert!(named(&ranked, "approved").standing() < 0.55);
}

/// Limit means "the best N", which only means anything because ranking happens
/// before the cut.
#[test]
fn the_limit_keeps_the_best_standing() {
    let store = Store::open_in_memory().unwrap();
    store
        .put_pair("guessed", &double(1), PairSource::Model)
        .unwrap();
    store
        .put_pair("approved", &double(2), PairSource::Confirmed)
        .unwrap();

    let top = store.all_pairs(1).unwrap();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].utterance, "approved");
    assert!(store.all_pairs(0).unwrap().is_empty());
}

/// Crediting a pair that is not there is a bug in the caller, and silently
/// crediting nothing would hide it for the life of the brain.
#[test]
fn crediting_a_missing_pair_is_an_error() {
    let store = Store::open_in_memory().unwrap();
    let err = store.record_pair_outcome(404, true).unwrap_err();
    assert!(
        matches!(err, StoreError::MissingRecord { kind: "pair", .. }),
        "unexpected error: {err}"
    );
}

/// The store records; it does not judge. Screening usable examples is the
/// phrasing index's job, and doing it in two places would mean two answers.
#[test]
fn empty_input_is_stored_as_given() {
    let store = Store::open_in_memory().unwrap();
    store.put_pair("", &double(1), PairSource::Model).unwrap();
    store
        .put_pair("nothing came of it", &[], PairSource::Model)
        .unwrap();
    assert_eq!(store.count_pairs().unwrap(), 2);
    assert_eq!(named(&store.all_pairs(10).unwrap(), "").steps, double(1));
}
