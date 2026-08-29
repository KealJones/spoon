//! The whole front-language pipeline against a real Engine.
//!
//! Every other test in this work covers one stage. This one runs the worked
//! example straight through: real model output, real grounding, real procedure
//! execution through the Engine, a real response plan, and both the template
//! realizer and the deterministic fallback.
//!
//! The proposal is the checked-in `qwen3.8:27b` fixture rather than a
//! hand-authored one, so a change that makes the pipeline reject real model
//! output fails here.

use std::collections::{BTreeMap, BTreeSet};

use spoon_core::language::{
    DialogueAct, PlannedClaim, ResponseRenderer, ResponseTone, TextSpan, tokenize,
};
use spoon_core::realizer::RealizationProposal;
use spoon_core::utterance::{
    MentionResolution, PartId, UtteranceAnalysis, UtteranceAnalysisProposal, UtteranceLimits,
};
use spoon_core::{Concept, EpisodeId, Expr, IntrinsicOp, MutabilityClass, Param, Procedure, Value};
use spoon_engine::engine::Engine;
use spoon_engine::parts::{EvidenceOrigin, PartOutcome, PartState, PartsRun, claim_id};

const UTTERANCE: &str = "hey whats 2+2 and then double that";
const FIXTURE: &str = include_str!("../../spoon-core/tests/fixtures/utterance-qwen3.8-27b.json");

fn analysis() -> UtteranceAnalysis {
    let stream = tokenize(UTTERANCE).expect("tokenizes");
    serde_json::from_str::<UtteranceAnalysisProposal>(FIXTURE)
        .expect("real model output deserializes")
        .ground_for(&stream, &BTreeSet::new(), &UtteranceLimits::default())
        .expect("real model output grounds")
}

/// Admits `add` and `double` so the parts have something real to execute.
fn engine_with_arithmetic() -> (Engine, spoon_core::ProcedureId, spoon_core::ProcedureId) {
    let mut engine = Engine::in_memory_with_admin("front-language-admin").unwrap();

    let add_concept = Concept::new("ADDITION", MutabilityClass::Definitional);
    engine.admin_insert_concept(&add_concept).unwrap();
    let add = Procedure::new(
        "ADD",
        vec![Param::named("left"), Param::named("right")],
        Expr::BinOp {
            op: spoon_core::BinOp::Add,
            left: Box::new(Expr::Var("left".into())),
            right: Box::new(Expr::Var("right".into())),
        },
    )
    .with_concept(add_concept.id);
    engine.admin_insert_procedure(&add).unwrap();

    let double_concept = Concept::new("DOUBLING", MutabilityClass::Definitional);
    engine.admin_insert_concept(&double_concept).unwrap();
    let double = Procedure::new(
        "DOUBLE",
        vec![Param::named("value")],
        Expr::BinOp {
            op: spoon_core::BinOp::Mul,
            left: Box::new(Expr::Var("value".into())),
            right: Box::new(Expr::Literal(Value::Int(2))),
        },
    )
    .with_concept(double_concept.id);
    engine.admin_insert_procedure(&double).unwrap();

    (engine, add.id, double.id)
}

/// Drives the run to completion, executing each ready part against the Engine.
///
/// This is the shape the cycle takes: ask what is ready, run it, record the
/// outcome, repeat. A consumer's input comes from the producer's recorded
/// outcome rather than from any rewrite of the analysis.
fn dispatch(
    run: &mut PartsRun,
    engine: &Engine,
    add: spoon_core::ProcedureId,
    double: spoon_core::ProcedureId,
) {
    while let Some(next) = run.next_ready().cloned() {
        let part = run.analysis.part(&next).expect("part exists").clone();

        // A part with no intent to execute is spoken, not computed. The
        // greeting is grounded in the fact that the user greeted.
        if part.act == DialogueAct::Acknowledge {
            let span = part.spans.first().copied().unwrap_or(TextSpan::new(0, 0));
            run.record(PartOutcome::spoken(next, "Hey.", span));
            continue;
        }

        let literals: Vec<Value> = part
            .mentions
            .iter()
            .filter_map(|mention| match &mention.resolved {
                MentionResolution::Literal { value } => Some(value.clone()),
                _ => None,
            })
            .collect();

        let bound_result = part
            .mentions
            .iter()
            .find_map(|mention| match &mention.resolved {
                MentionResolution::PartRef { part, .. } => run.resolved_value(part).cloned(),
                _ => None,
            });

        let (procedure, inputs, text) = if let Some(value) = bound_result {
            let inputs = BTreeMap::from([("value".to_string(), value)]);
            (double, inputs, "Double that is")
        } else {
            let inputs = BTreeMap::from([
                ("left".to_string(), literals[0].clone()),
                ("right".to_string(), literals[1].clone()),
            ]);
            (add, inputs, "2 + 2 is")
        };

        let outcome = engine
            .execute_procedure(procedure, inputs, None)
            .expect("procedure executes");
        let answer = outcome.value.clone();
        let rendered = match &answer {
            Value::Int(number) => format!("{text} {number}."),
            other => format!("{text} {other:?}."),
        };

        run.record(PartOutcome::executed(
            next,
            answer,
            rendered,
            EvidenceOrigin::Procedure {
                id: procedure.to_string(),
                version: 1,
            },
        ));
    }
}

#[test]
fn the_worked_example_runs_end_to_end() {
    let (engine, add, double) = engine_with_arithmetic();
    let mut run = PartsRun::new(analysis(), EpisodeId::new()).expect("acyclic");

    dispatch(&mut run, &engine, add, double);

    assert!(run.is_complete());
    assert_eq!(run.executed_count(), 3);

    // The second question really consumed the first answer rather than a
    // guess: 2 + 2 = 4, doubled is 8.
    assert_eq!(
        run.resolved_value(&PartId::parse("p1").unwrap()),
        Some(&Value::Int(4))
    );
    assert_eq!(
        run.resolved_value(&PartId::parse("p2").unwrap()),
        Some(&Value::Int(8))
    );

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let rendered = ResponseRenderer.render(&plan).expect("renders");

    // Neither half of the utterance was dropped, which is the entire point.
    assert_eq!(rendered.text, "Hey. 2 + 2 is 4. Double that is 8.");
    assert!(rendered.omitted_claim_ids.is_empty());
    assert_eq!(plan.dialogue_move.act, DialogueAct::Inform);
}

#[test]
fn the_realizer_stitches_the_same_plan_into_one_sentence() {
    let (engine, add, double) = engine_with_arithmetic();
    let mut run = PartsRun::new(analysis(), EpisodeId::new()).expect("acyclic");
    dispatch(&mut run, &engine, add, double);

    let dependencies = run.claim_dependencies();
    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let stream = tokenize(UTTERANCE).expect("tokenizes");

    let realized = RealizationProposal {
        template_id: "join.ack.and".to_string(),
        slot_order: vec![
            claim_id(&PartId::parse("p0").unwrap()),
            claim_id(&PartId::parse("p1").unwrap()),
            claim_id(&PartId::parse("p2").unwrap()),
        ],
        tone: ResponseTone::Neutral,
    }
    .realize(&plan, &dependencies, &stream)
    .expect("valid realization");

    assert_eq!(realized.text, "Hey. 2 + 2 is 4, and double that is 8.");
    // Every computed number survives verbatim.
    assert!(realized.text.contains("4"));
    assert!(realized.text.contains("8."));
}

#[test]
fn the_realizer_cannot_reverse_the_dependency_even_with_real_results() {
    let (engine, add, double) = engine_with_arithmetic();
    let mut run = PartsRun::new(analysis(), EpisodeId::new()).expect("acyclic");
    dispatch(&mut run, &engine, add, double);

    let dependencies = run.claim_dependencies();
    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let stream = tokenize(UTTERANCE).expect("tokenizes");

    let error = RealizationProposal {
        template_id: "join.ack.and".to_string(),
        slot_order: vec![
            claim_id(&PartId::parse("p0").unwrap()),
            claim_id(&PartId::parse("p2").unwrap()),
            claim_id(&PartId::parse("p1").unwrap()),
        ],
        tone: ResponseTone::Neutral,
    }
    .realize(&plan, &dependencies, &stream)
    .expect_err("a consumer cannot be worded before its producer");
    assert!(
        error.to_string().contains("cannot be worded before it"),
        "{error}"
    );
}

#[test]
fn an_untaught_part_still_lets_its_independent_sibling_answer() {
    let (engine, add, double) = engine_with_arithmetic();
    let mut run = PartsRun::new(analysis(), EpisodeId::new()).expect("acyclic");

    // The greeting runs, then the sum turns out to be untaught.
    let greeting = run.next_ready().cloned().expect("greeting is ready");
    let span = run
        .analysis
        .part(&greeting)
        .unwrap()
        .spans
        .first()
        .copied()
        .unwrap();
    run.record(PartOutcome::spoken(greeting, "Hey.", span));
    run.record(PartOutcome::abstained(
        PartId::parse("p1").unwrap(),
        "no procedure was taught for this part",
    ));

    // The part that consumed the sum is blocked, not attempted.
    assert_eq!(
        run.outcomes[&PartId::parse("p2").unwrap()].state,
        PartState::Blocked
    );
    assert!(run.is_complete());
    let _ = (engine, add, double);

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    let rendered = ResponseRenderer.render(&plan).expect("renders");

    // The greeting still reaches the user, and the two parts that could not
    // run are retained as unsupported rather than silently vanishing.
    assert_eq!(rendered.text, "Hey.");
    assert_eq!(rendered.omitted_claim_ids.len(), 2);
    let unsupported = plan
        .claims
        .iter()
        .filter(|claim| matches!(claim, PlannedClaim::Unsupported { .. }))
        .count();
    assert_eq!(unsupported, 2);
}

#[test]
fn every_rendered_claim_traces_back_to_something_that_happened() {
    let (engine, add, double) = engine_with_arithmetic();
    let mut run = PartsRun::new(analysis(), EpisodeId::new()).expect("acyclic");
    dispatch(&mut run, &engine, add, double);

    let plan = run.response_plan("turn-1", ResponseTone::Neutral);
    for claim in &plan.claims {
        let PlannedClaim::Grounded(claim) = claim else {
            panic!("every part executed, so every claim should be grounded");
        };
        assert!(!claim.evidence.is_empty(), "{} has no evidence", claim.id);
        let evidence = &claim.evidence[0];
        assert!(evidence.linked_episode.is_some());
        // Either the user said it, or a procedure computed it. Nothing else
        // can produce a claim.
        assert!(
            evidence.id.contains(":utterance:") || evidence.id.contains(":part:"),
            "unexpected evidence id {}",
            evidence.id
        );
    }
}
