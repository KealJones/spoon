use ekg_engine::{
    Engine, FalsificationMeasurementInput, FalsificationRunInput, GroundingTier,
    MetricEvidenceStatus, ProbeCohort, TeacherMode,
};

fn observation(probe: &str, novelty: &str) -> FalsificationMeasurementInput {
    FalsificationMeasurementInput {
        domain: "arithmetic".into(),
        family: "doubles".into(),
        cohort: ProbeCohort::Training,
        probe_id: probe.into(),
        novelty_identity: novelty.into(),
        repeat_of: None,
        teacher_mode: TeacherMode::On,
        teacher_used: true,
        teacher_calls: 1,
        rung: "Ask".into(),
        steps: 4,
        candidates: 2,
        trace_steps: 2,
        cost: 10.0,
        abstained: false,
        clarified: false,
        confidence: Some(0.9),
        grounding_tier: GroundingTier::Strong,
        used_skill_id: None,
        created_skill_id: Some("double-number".into()),
        correct: Some(true),
        failure_reason: None,
        baseline_trace_steps: Some(4),
        regression_probe: true,
        attribution_correct: Some(true),
        attribution_cost: Some(2.0),
    }
}

#[test]
fn durable_telemetry_scores_only_explicit_probe_evidence() {
    let engine = Engine::in_memory().unwrap();
    let run = engine
        .create_falsification_run(FalsificationRunInput {
            label: "small falsification harness".into(),
            benchmark: "tests/falsification/fixtures.json".into(),
            notes: Some("Synthetic fixture: does not claim production performance.".into()),
        })
        .unwrap();

    let first = engine
        .record_falsification_measurement(&run.id, observation("train-1", "double-2"))
        .unwrap();
    let mut later_acquisition = observation("train-2", "double-3");
    later_acquisition.teacher_calls = 0;
    later_acquisition.teacher_used = false;
    later_acquisition.teacher_mode = TeacherMode::Off;
    later_acquisition.cost = 5.0;
    engine
        .record_falsification_measurement(&run.id, later_acquisition)
        .unwrap();

    let mut held_out = observation("heldout-1", "double-97");
    held_out.cohort = ProbeCohort::HeldOut;
    held_out.family = "held-out-doubles".into();
    held_out.used_skill_id = Some("double-number".into());
    held_out.created_skill_id = None;
    held_out.teacher_mode = TeacherMode::Off;
    held_out.teacher_used = false;
    held_out.teacher_calls = 0;
    held_out.cost = 3.0;
    engine
        .record_falsification_measurement(&run.id, held_out)
        .unwrap();

    // A declared repeat is permitted for ablation reporting, but cannot be
    // smuggled into acquisition/transfer metrics.
    let mut teacher_off = observation("train-1", "double-2");
    teacher_off.repeat_of = Some(first.id);
    teacher_off.teacher_mode = TeacherMode::Off;
    teacher_off.teacher_used = false;
    teacher_off.teacher_calls = 0;
    teacher_off.used_skill_id = Some("double-number".into());
    engine
        .record_falsification_measurement(&run.id, teacher_off)
        .unwrap();

    let mut abstention = observation("heldout-clarify", "ambiguous-double");
    abstention.cohort = ProbeCohort::HeldOut;
    abstention.family = "held-out-clarifications".into();
    abstention.abstained = true;
    abstention.clarified = true;
    abstention.correct = None;
    abstention.confidence = None;
    abstention.created_skill_id = None;
    abstention.used_skill_id = None;
    engine
        .record_falsification_measurement(&run.id, abstention)
        .unwrap();

    let snapshot = engine.metrics_snapshot().unwrap().section38;
    assert_eq!(snapshot.runs, 1);
    assert_eq!(snapshot.measurements, 5);
    assert_eq!(snapshot.abstentions, 1);
    assert_eq!(snapshot.clarifications, 1);
    assert_eq!(snapshot.metrics.len(), 12);
    assert_eq!(snapshot.metrics[1].name, "Transfer");
    assert_eq!(snapshot.metrics[1].status, MetricEvidenceStatus::Measured);
    assert_eq!(snapshot.metrics[1].sample_size, 1);
    assert_eq!(snapshot.metrics[8].name, "Teacher ablation");
    assert_eq!(snapshot.metrics[8].status, MetricEvidenceStatus::Measured);
    assert_eq!(snapshot.metrics[11].name, "Calibration");
    assert_eq!(snapshot.metrics[11].status, MetricEvidenceStatus::Measured);
}

#[test]
fn rejects_teacher_leakage_and_undeclared_exact_repeats() {
    let engine = Engine::in_memory().unwrap();
    let run = engine
        .create_falsification_run(FalsificationRunInput {
            label: "validation".into(),
            benchmark: "unit".into(),
            notes: None,
        })
        .unwrap();
    let mut leaked = observation("same", "same-input");
    leaked.teacher_mode = TeacherMode::Off;
    assert!(
        engine
            .record_falsification_measurement(&run.id, leaked)
            .is_err()
    );
    let valid = observation("same", "same-input");
    engine
        .record_falsification_measurement(&run.id, valid.clone())
        .unwrap();
    assert!(
        engine
            .record_falsification_measurement(&run.id, valid)
            .is_err()
    );
    let telemetry = engine.metrics_snapshot().unwrap().section38;
    assert_eq!(telemetry.teacher_off_violations_rejected, 1);
    assert_eq!(telemetry.duplicate_measurements_rejected, 1);
}

#[test]
fn held_out_measurements_cannot_train_or_claim_teacher_grounding_when_off() {
    let engine = Engine::in_memory().unwrap();
    let run = engine
        .create_falsification_run(FalsificationRunInput {
            label: "held-out boundary".into(),
            benchmark: "unit".into(),
            notes: None,
        })
        .unwrap();

    let mut held_out_training = observation("heldout-train", "unseen-input");
    held_out_training.cohort = ProbeCohort::HeldOut;
    assert!(
        engine
            .record_falsification_measurement(&run.id, held_out_training)
            .is_err()
    );

    let mut teacher_grounding = observation("teacher-grounding", "another-input");
    teacher_grounding.teacher_mode = TeacherMode::Off;
    teacher_grounding.teacher_used = false;
    teacher_grounding.teacher_calls = 0;
    teacher_grounding.grounding_tier = GroundingTier::Teacher;
    assert!(
        engine
            .record_falsification_measurement(&run.id, teacher_grounding)
            .is_err()
    );
}

#[test]
fn training_and_held_out_cohorts_cannot_share_a_task_family() {
    let engine = Engine::in_memory().unwrap();
    let run = engine
        .create_falsification_run(FalsificationRunInput {
            label: "cohort separation".into(),
            benchmark: "unit".into(),
            notes: None,
        })
        .unwrap();
    engine
        .record_falsification_measurement(&run.id, observation("train", "input-1"))
        .unwrap();
    let mut held_out = observation("held-out", "input-2");
    held_out.cohort = ProbeCohort::HeldOut;
    held_out.created_skill_id = None;
    assert!(
        engine
            .record_falsification_measurement(&run.id, held_out)
            .is_err()
    );
    assert_eq!(
        engine
            .metrics_snapshot()
            .unwrap()
            .section38
            .cohort_leakage_rejected,
        1
    );
}
