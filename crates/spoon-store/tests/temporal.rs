//! Belief over time: assert, retract, and ask what was believed then.

use std::thread::sleep;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use spoon_concept::{Concept, Provenance};
use spoon_store::Store;

fn friendship() -> Concept {
    Concept::call(
        "friend-with",
        [Concept::named("greg"), Concept::named("keal")],
    )
}

#[test]
fn retracting_flips_holds_while_the_earlier_belief_survives() {
    let store = Store::open_in_memory().unwrap();
    let c = friendship();

    let id = store
        .assert_concept(
            &c,
            Provenance::User { episode: Some(7) },
            Some(7),
            Some(0.9),
        )
        .unwrap();
    assert!(store.holds(&c).unwrap());

    let live = store.live_assertions(&c).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, id);
    assert_eq!(live[0].concept, c);
    assert_eq!(live[0].episode, Some(7));
    assert_eq!(live[0].confidence, Some(0.9));
    assert_eq!(live[0].provenance, Provenance::User { episode: Some(7) });
    assert!(live[0].invalidated_at.is_none());
    let asserted_at = live[0].asserted_at;

    // A strictly later instant, so the two moments are distinguishable at
    // millisecond resolution.
    sleep(Duration::from_millis(5));
    let when_retracted = Utc::now();
    assert!(store.retract(id, when_retracted).unwrap());

    assert!(!store.holds(&c).unwrap());
    assert!(store.live_assertions(&c).unwrap().is_empty());

    // History is intact: at the moment it was asserted, it was believed.
    let then = store.assertions_at(&c, asserted_at).unwrap();
    assert_eq!(then.len(), 1);
    assert_eq!(then[0].id, id);
    // Timestamps are stored to the millisecond, so compare at that
    // resolution rather than at the clock's nanoseconds.
    assert_eq!(
        then[0].invalidated_at.unwrap().timestamp_millis(),
        when_retracted.timestamp_millis()
    );

    // And before it was ever asserted, it was not.
    let before = store
        .assertions_at(&c, asserted_at - ChronoDuration::seconds(1))
        .unwrap();
    assert!(before.is_empty());
}

#[test]
fn retracting_twice_reports_that_nothing_changed() {
    let store = Store::open_in_memory().unwrap();
    let c = friendship();
    let id = store
        .assert_concept(&c, Provenance::Bootstrap, None, None)
        .unwrap();

    assert!(store.retract(id, Utc::now()).unwrap());
    assert!(!store.retract(id, Utc::now()).unwrap());
    assert!(
        !store
            .retract(spoon_store::AssertionId(9999), Utc::now())
            .unwrap()
    );
}

#[test]
fn a_validity_window_in_the_past_is_believed_but_does_not_hold_now() {
    let store = Store::open_in_memory().unwrap();
    let employment = Concept::call("works-at", [Concept::named("greg"), Concept::named("acme")]);
    let now = Utc::now();

    // Learned today, true only until last year.
    store
        .assert_concept_during(
            &employment,
            Provenance::Teacher { episode: None },
            None,
            None,
            Some(now - ChronoDuration::days(730)),
            Some(now - ChronoDuration::days(365)),
        )
        .unwrap();

    // Still believed: the claim about the past stands.
    let live = store.live_assertions(&employment).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(
        live[0].valid_to.unwrap().timestamp_millis(),
        (now - ChronoDuration::days(365)).timestamp_millis()
    );
    // But it is not the case now.
    assert!(!store.holds(&employment).unwrap());

    // An open-ended assertion of the same concept does hold.
    store
        .assert_concept(&employment, Provenance::User { episode: None }, None, None)
        .unwrap();
    assert!(store.holds(&employment).unwrap());
    assert_eq!(store.live_assertions(&employment).unwrap().len(), 2);
}
