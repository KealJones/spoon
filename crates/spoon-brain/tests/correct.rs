//! Corrections: the most informative thing a user says.

use chrono::Utc;
use spoon_brain::{
    Correction, EarsPath, Episode, MouthPath, TurnMetrics, apply_correction, is_correction,
};
use spoon_concept::{
    Activation, Concept, Effect, NativeId, Provenance, Realization, RealizationSpec, Tier,
};
use spoon_store::Store;

fn episode(id: u64, claim: Concept, realization: &str) -> Episode {
    Episode {
        id,
        at: Utc::now(),
        session: "t".into(),
        user_text: "john has a dog".into(),
        steps: vec![],
        ears_path: EarsPath::Model,
        request_text: None,
        phrasing: None,
        assertions: None,
        correction_of: None,
        unknown_words: vec![],
        goal: Some(claim.clone()),
        result: Some(claim),
        gaps: vec![],
        realizations: vec![(realization.to_string(), true)],
        trace: vec![],
        learning: vec![],
        rules: vec![],
        reply: "noted: owns<john, dog>".into(),
        mouth_path: MouthPath::Template,
        metrics: TurnMetrics::default(),
        correction: None,
        ears_exchange: None,
        teacher_exchanges: vec![],
        mouth_exchange: None,
    }
}

fn a_realization(store: &Store, name: &str) {
    store
        .put_realization(&Realization {
            target: Concept::named("owns"),
            name: name.into(),
            spec: RealizationSpec::Native {
                native: NativeId::new("math-add"),
            },
            effect: Effect::Pure,
            activation: Activation::new(Utc::now()),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })
        .unwrap();
}

#[test]
fn a_correction_withdraws_what_the_previous_turn_asserted() {
    let store = Store::open_in_memory().unwrap();
    let claim = Concept::call("owns", [Concept::named("john"), Concept::named("dog")]);
    store
        .assert_concept(&claim, Provenance::User { episode: None }, None, None)
        .unwrap();
    assert!(store.holds(&claim).unwrap());

    a_realization(&store, "r1");
    let out = apply_correction(&store, &episode(1, claim.clone(), "r1"), Utc::now()).unwrap();

    assert!(
        !store.holds(&claim).unwrap(),
        "the claim should no longer hold"
    );
    assert_eq!(out.retracted.len(), 1);
}

#[test]
fn retraction_keeps_the_history_rather_than_deleting_it() {
    // What Spoon believed at the time is exactly what makes a wrong answer
    // diagnosable afterwards.
    let store = Store::open_in_memory().unwrap();
    let claim = Concept::call("owns", [Concept::named("john"), Concept::named("dog")]);
    store
        .assert_concept(&claim, Provenance::User { episode: None }, None, None)
        .unwrap();
    let before = Utc::now();
    std::thread::sleep(std::time::Duration::from_millis(5));

    a_realization(&store, "r1");
    apply_correction(&store, &episode(1, claim.clone(), "r1"), Utc::now()).unwrap();

    assert!(!store.holds(&claim).unwrap());
    assert!(
        !store.assertions_at(&claim, before).unwrap().is_empty(),
        "the earlier belief should still be visible"
    );
}

#[test]
fn the_realization_that_produced_the_answer_is_marked_down() {
    let store = Store::open_in_memory().unwrap();
    let claim = Concept::call("owns", [Concept::named("john"), Concept::named("dog")]);
    store
        .assert_concept(&claim, Provenance::User { episode: None }, None, None)
        .unwrap();
    a_realization(&store, "r1");
    store
        .record_realization_use("r1", true, Utc::now())
        .unwrap();

    let before = store.realization_by_name("r1").unwrap().unwrap();
    assert_eq!(before.activation.failures, 0);

    apply_correction(&store, &episode(1, claim, "r1"), Utc::now()).unwrap();

    let after = store.realization_by_name("r1").unwrap().unwrap();
    assert_eq!(
        after.activation.failures, 1,
        "the failure should be recorded"
    );
    assert!(after.activation.success_rate() < before.activation.success_rate());
}

#[test]
fn the_episode_is_amended_not_replaced() {
    let store = Store::open_in_memory().unwrap();
    let claim = Concept::call("owns", [Concept::named("john"), Concept::named("dog")]);
    let ep = episode(1, claim, "r1");
    store
        .put_episode(&serde_json::to_string(&ep).unwrap(), 1)
        .unwrap();

    assert!(
        store
            .mark_episode_corrected(1, "no, mary has the dog")
            .unwrap()
    );
    let raw = store.recent_episodes(1, Some("t")).unwrap();
    let back: Episode = serde_json::from_str(&raw[0]).unwrap();

    assert_eq!(back.correction.as_deref(), Some("no, mary has the dog"));
    // Everything that made the turn diagnosable is untouched.
    assert_eq!(back.user_text, "john has a dog");
    assert_eq!(back.reply, "noted: owns<john, dog>");
    assert_eq!(back.realizations, vec![("r1".to_string(), true)]);
    assert_eq!(store.corrected_episodes(10).unwrap().len(), 1);
}

#[test]
fn repair_markers_are_recognised() {
    for text in [
        "no wait",
        "no, mary has the dog",
        "actually no",
        "thats wrong",
        "i meant mary",
        "scratch that",
        "nvm",
        "nope",
        "wrong",
        "sorry i meant the scores",
    ] {
        assert!(is_correction(text), "{text:?} should read as a repair");
    }
}

#[test]
fn ordinary_speech_is_not_mistaken_for_a_repair() {
    // A false positive silently deletes something the user asserted, so the
    // marker list stays short and the check stays strict.
    for text in [
        "john has a dog",
        "who owns a dog?",
        "no one owns a dog",
        "there is no problem",
        "knowing that helps",
        "i know what you meant",
        "",
    ] {
        assert!(!is_correction(text), "{text:?} should not read as a repair");
    }
}

#[test]
fn a_correction_with_nothing_to_undo_is_harmless() {
    let store = Store::open_in_memory().unwrap();
    let claim = Concept::call("owns", [Concept::named("john"), Concept::named("dog")]);
    let out = apply_correction(&store, &episode(1, claim, "missing"), Utc::now()).unwrap();
    assert_eq!(
        out,
        Correction {
            episode: 1,
            ..Default::default()
        }
    );
    assert!(out.is_empty());
}

#[test]
fn an_elliptical_repair_borrows_the_verb_it_left_out() {
    // "reverse banana, actually no, possession" replaces one word and leaves
    // the verb implied. Reading only the part after the marker gets a bare
    // noun; reading only the part before gets the answer to a question that
    // was withdrawn mid-sentence. Both were happening.
    for (say, want) in [
        (
            "reverse banana. actually no, possession",
            "reverse possession",
        ),
        ("reverse committee. actually no, hello", "reverse hello"),
        ("reverse science sorry i meant banana", "reverse banana"),
    ] {
        assert_eq!(spoon_brain::repaired(say).as_deref(), Some(want), "{say:?}");
    }
}

#[test]
fn a_repair_keeps_the_words_it_does_not_replace() {
    // The repair covers three words, so it replaces three, and "what is"
    // survives because the speaker never withdrew it.
    assert_eq!(
        spoon_brain::repaired("what is 32 plus 50. scratch that, 32 plus 57").as_deref(),
        Some("what is 32 plus 57")
    );
}

#[test]
fn a_marker_opening_the_utterance_repairs_the_previous_turn() {
    // Nothing before it to splice into, so the repair is the whole sentence
    // and the retraction machinery is what handles it.
    let repair = spoon_brain::split_repair("no wait, reverse spoon").expect("marker");
    assert!(repair.before.is_empty());
    assert_eq!(repair.after, "reverse spoon");
}

#[test]
fn changing_your_mind_twice_means_the_third_thing() {
    assert_eq!(
        spoon_brain::repaired("reverse banana, no wait, coffee, actually no, spoon").as_deref(),
        Some("reverse spoon")
    );
}

#[test]
fn an_utterance_with_no_marker_needs_no_repair() {
    assert!(spoon_brain::repaired("reverse banana").is_none());
}
