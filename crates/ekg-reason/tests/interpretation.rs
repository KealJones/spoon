use ekg_core::ConceptId;
use ekg_reason::{InterpretationCandidate, InterpretationError, InterpretationSet};

fn candidate(meaning: ConceptId, weight: f64) -> InterpretationCandidate {
    InterpretationCandidate { meaning, weight }
}

#[test]
fn preserves_weighted_ambiguity_and_episode_losers() {
    let double = ConceptId::new();
    let repeat = ConceptId::new();
    let unknown = ConceptId::new();
    let interpreted = InterpretationSet::try_new(
        vec![
            candidate(double, 0.91),
            candidate(repeat, 0.06),
            candidate(unknown, 0.03),
        ],
        Some(double),
    )
    .unwrap();

    assert_eq!(interpreted.candidates()[0].meaning, double);
    assert_eq!(interpreted.candidates()[1].meaning, repeat);
    assert_eq!(interpreted.candidates()[2].meaning, unknown);
    assert_eq!(interpreted.chosen(), Some(double));

    let episode_rows = interpreted.to_episode_interpretations();
    assert_eq!(episode_rows.len(), 3);
    assert!(episode_rows[0].chosen);
    assert!(!episode_rows[1].chosen);
    assert!(!episode_rows[2].chosen);
    assert_eq!(episode_rows[2].meaning, unknown);
}

#[test]
fn unresolved_ambiguity_is_valid_and_marks_no_candidate_chosen() {
    let first = ConceptId::new();
    let unknown = ConceptId::new();
    let interpreted =
        InterpretationSet::try_new(vec![candidate(first, 0.5), candidate(unknown, 0.5)], None)
            .unwrap();

    assert_eq!(interpreted.chosen(), None);
    assert!(
        interpreted
            .to_episode_interpretations()
            .iter()
            .all(|candidate| !candidate.chosen)
    );
}

#[test]
fn accepts_a_configurable_floating_point_tolerance() {
    let first = ConceptId::new();
    let second = ConceptId::new();
    let interpreted = InterpretationSet::try_new_with_tolerance(
        vec![candidate(first, 0.7), candidate(second, 0.300_000_4)],
        Some(first),
        0.000_001,
    )
    .unwrap();
    let serialized = serde_json::to_string(&interpreted).unwrap();
    let round_tripped: InterpretationSet = serde_json::from_str(&serialized).unwrap();

    assert_eq!(round_tripped, interpreted);
    assert_eq!(round_tripped.tolerance(), 0.000_001);
}

#[test]
fn rejects_empty_duplicate_or_missing_selected_candidates() {
    let concept = ConceptId::new();
    let absent = ConceptId::new();

    assert!(matches!(
        InterpretationSet::try_new(Vec::new(), None),
        Err(InterpretationError::EmptyCandidates)
    ));
    assert!(matches!(
        InterpretationSet::try_new(
            vec![candidate(concept, 0.5), candidate(concept, 0.5)],
            Some(concept)
        ),
        Err(InterpretationError::DuplicateMeaning(id)) if id == concept
    ));
    assert!(matches!(
        InterpretationSet::try_new(vec![candidate(concept, 1.0)], Some(absent)),
        Err(InterpretationError::ChosenCandidateMissing(id)) if id == absent
    ));
}

#[test]
fn rejects_non_finite_negative_and_non_normalized_weights() {
    let concept = ConceptId::new();

    for weight in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            InterpretationSet::try_new(vec![candidate(concept, weight)], None),
            Err(InterpretationError::NonFiniteWeight { meaning, .. }) if meaning == concept
        ));
    }

    assert!(matches!(
        InterpretationSet::try_new(vec![candidate(concept, -1.0)], None),
        Err(InterpretationError::NegativeWeight { meaning, .. }) if meaning == concept
    ));
    assert!(matches!(
        InterpretationSet::try_new(vec![candidate(concept, 0.75)], None),
        Err(InterpretationError::WeightsDoNotSumToOne { .. })
    ));
    assert!(matches!(
        InterpretationSet::try_new_with_tolerance(vec![candidate(concept, 1.0)], None, -0.1),
        Err(InterpretationError::InvalidTolerance(_))
    ));
}

#[test]
fn deserialization_cannot_bypass_distribution_validation() {
    let meaning = ConceptId::new();
    let json = format!(
        r#"{{"candidates":[{{"meaning":"{}","weight":0.25}}],"chosen":null}}"#,
        meaning.0
    );

    assert!(serde_json::from_str::<InterpretationSet>(&json).is_err());
}

#[test]
fn deserialization_cannot_widen_the_normalization_tolerance() {
    let meaning = ConceptId::new();
    let json = format!(
        r#"{{"candidates":[{{"meaning":"{}","weight":0.25}}],"chosen":null,"tolerance":1.0}}"#,
        meaning.0
    );

    assert!(serde_json::from_str::<InterpretationSet>(&json).is_err());
    assert!(matches!(
        InterpretationSet::try_new_with_tolerance(vec![candidate(meaning, 1.0)], None, 0.1),
        Err(InterpretationError::ToleranceExceedsMaximum { .. })
    ));
}

#[test]
fn interpretation_candidate_count_has_an_absolute_ceiling() {
    let candidates = (0..=ekg_reason::MAX_INTERPRETATION_CANDIDATES)
        .map(|_| candidate(ConceptId::new(), 0.0))
        .collect();

    assert!(matches!(
        InterpretationSet::try_new(candidates, None),
        Err(InterpretationError::TooManyCandidates { .. })
    ));
}
