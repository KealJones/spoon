use std::collections::{BTreeMap, HashMap};

use sha2::{Digest, Sha256};
use spoon_capability::{
    CapabilityStore, LocalValidation, Permission, import_bundle, reconstruct_bundle,
};
use spoon_core::{Procedure, ProcedureId, TraceStepStatus, Value};
use spoon_engine::{
    CycleBudget, CycleDisposition, CycleInput, CycleOutcome, CycleProgress, Engine, RecallMode,
};

use crate::ForgeError;
use crate::curriculum::{
    Activity, Curriculum, ExpectedDisposition, GateStage, GateStore, ImportStep, TeacherMode,
    TeacherOffGate,
};
use crate::export::{ReplayCase, SeedBundle, build_seed_bundle, install_seed};
use crate::inspect::inspect_structures;
use crate::report::{
    ActivityReport, CleanImportReport, ExportReport, ForgeReport, GateReport, ImportStepReport,
    Observed, Phase, PhaseReport,
};
use crate::teacher::{CurriculumTeacher, TeacherSession};

/// Admin secret for the forge's own instances. Both engines are created and
/// destroyed inside a run, and the target needs admin standing to install a
/// reconstructed seed, which is an explicit local action rather than something
/// the bundle can do to it.
const SEED_ADMIN: &str = "spoon-forge";
const MAX_EXEC_STEPS: u32 = 10_000;
const MAX_CONTEXT_ITEMS: usize = 32;
const MAX_TEACHER_TURNS: u32 = 2;
/// A cycle alternates between the engine and the Teacher. Anything that has
/// not settled in this many exchanges is looping, not thinking.
const MAX_ROUNDS: usize = 8;
const MAX_LOCAL_EVIDENCE: u32 = 64;

type Cases = HashMap<ProcedureId, Vec<ReplayCase>>;

/// Drives one curriculum from a clean engine to an independently reconstructed
/// seed.
pub struct ForgeRunner<'a> {
    curriculum: &'a Curriculum,
}

impl<'a> ForgeRunner<'a> {
    pub fn new(curriculum: &'a Curriculum) -> Self {
        Self { curriculum }
    }

    pub fn run(&self, teacher: &mut dyn CurriculumTeacher) -> Result<ForgeReport, ForgeError> {
        let mut source = Engine::in_memory_with_admin(SEED_ADMIN)?;
        let mut session = TeacherSession::new(teacher);
        let mut cases = Cases::new();

        let mut phases = Vec::new();
        for (phase, activities) in [
            (Phase::Demonstrations, &self.curriculum.demonstrations),
            (Phase::Counterexamples, &self.curriculum.counterexamples),
            (Phase::Exercises, &self.curriculum.exercises),
            (
                Phase::HeldOutGeneralization,
                &self.curriculum.held_out_generalization,
            ),
        ] {
            phases.push(run_phase(
                &mut source,
                phase,
                activities,
                &mut session,
                &mut cases,
            ));
        }

        let structures = inspect_structures(&self.curriculum.expected_learned_structures, &source)?;

        let mut gates = Vec::new();
        for gate in &self.curriculum.teacher_off_gates {
            if gate.independent_store == GateStore::SameCleanCurriculumStore {
                gates.push(run_gate(&mut source, self.curriculum, gate, &mut cases));
            }
        }

        let (export, seed) = self.export(&source, &cases);
        let clean_import = self.clean_import(seed.as_ref())?;

        let passed = phases.iter().all(|phase| phase.passed)
            && structures
                .iter()
                .all(|finding| finding.status == crate::StructureStatus::Matched)
            && gates.iter().all(|gate| gate.passed)
            && export.passed
            && clean_import.passed;

        Ok(ForgeReport {
            curriculum_id: self.curriculum.id.clone(),
            curriculum_version: self.curriculum.version.clone(),
            phases,
            structures,
            gates,
            export,
            clean_import,
            signature: None,
            passed,
        })
    }

    fn export(&self, engine: &Engine, cases: &Cases) -> (ExportReport, Option<SeedBundle>) {
        let declared_deny = self.curriculum.export_privacy.deny.clone();
        match build_seed_bundle(self.curriculum, engine, cases) {
            Ok(seed) => (
                ExportReport {
                    content_id: Some(seed.content_id().into()),
                    procedures: seed.procedure_names(),
                    byte_length: seed.bytes.len(),
                    declared_deny,
                    passed: true,
                    refusal: None,
                },
                Some(seed),
            ),
            Err(error) => (
                ExportReport {
                    content_id: None,
                    procedures: Vec::new(),
                    byte_length: 0,
                    declared_deny,
                    passed: false,
                    refusal: Some(error.to_string()),
                },
                None,
            ),
        }
    }

    /// Rebuild the seed in a second clean engine and make it earn its standing
    /// there.
    ///
    /// The full six-step sequence always runs. A manifest that declares fewer
    /// steps does not get a weaker check: under-declaring is not permission to
    /// skip verification.
    fn clean_import(&self, seed: Option<&SeedBundle>) -> Result<CleanImportReport, ForgeError> {
        let Some(seed) = seed else {
            return Ok(refused_import());
        };
        let mut steps = Vec::new();
        let mut replayed_cases = Vec::new();
        let mut gates = Vec::new();

        let bundle = import_bundle(&seed.bytes);
        steps.push(step(
            ImportStep::VerifyManifestAndContentHashes,
            bundle.as_ref().map(|bundle| bundle.content_id.clone()),
            bundle.as_ref().err(),
        ));
        let Ok(bundle) = bundle else {
            return Ok(halted_import(steps));
        };

        let reconstructed = reconstruct_bundle(&bundle);
        steps.push(step(
            ImportStep::ResolveDependencyClosure,
            reconstructed.as_ref().map(|reconstructed| {
                format!("{} dependencies", reconstructed.dependency_order.len())
            }),
            reconstructed.as_ref().err(),
        ));
        let Ok(reconstructed) = reconstructed else {
            return Ok(halted_import(steps));
        };

        // Nothing is granted here and nothing calls `grant`. The check is that
        // the bundle only ever asks to be observed, so an import cannot even
        // request reach into the host.
        let inert = reconstructed.procedures.iter().all(|procedure| {
            matches!(
                procedure.permissions.as_slice(),
                [Permission::ObserveTarget { .. }]
            )
        });
        steps.push(ImportStepReport {
            step: ImportStep::CheckLocalPermissions,
            passed: inert,
            detail: Some(if inert {
                "every procedure requests observation only; no grant issued".into()
            } else {
                "a procedure requests authority beyond observation".into()
            }),
        });

        let target = Engine::in_memory_with_admin(SEED_ADMIN)?;
        let mut installed = Vec::new();
        for procedure in &reconstructed.procedures {
            installed.push(install_seed(&target, procedure)?);
        }
        // The recorded expectation is deliberately withheld from the engine and
        // checked here instead. Handing it over as a prediction makes the
        // engine record a verified fact about the concept for each distinct
        // input, and two such facts refine into a discriminator the gate
        // cycles cannot satisfy, which would route every later probe to
        // abstention. The replay must not decide the gate that follows it.
        for (id, name, cases) in &installed {
            let reproduced = cases.iter().all(|case| {
                target
                    .execute_procedure(*id, case.inputs.clone(), None)
                    .is_ok_and(|outcome| outcome.value == case.expected)
            });
            replayed_cases.push((name.clone(), reproduced));
        }
        let tests_passed =
            !replayed_cases.is_empty() && replayed_cases.iter().all(|(_, passed)| *passed);
        steps.push(ImportStepReport {
            step: ImportStep::ReconstructAndRunDeterministicTests,
            passed: tests_passed,
            detail: Some(format!(
                "{} of {} rebuilt procedures reproduced their recorded cases",
                replayed_cases.iter().filter(|(_, passed)| *passed).count(),
                replayed_cases.len()
            )),
        });

        let mut target = target;
        let mut discard = Cases::new();
        for gate in &self.curriculum.teacher_off_gates {
            if gate.independent_store == GateStore::CleanImportTarget {
                gates.push(run_gate(&mut target, self.curriculum, gate, &mut discard));
            }
        }
        let gates_passed = gates.iter().all(|gate| gate.passed);
        steps.push(ImportStepReport {
            step: ImportStep::RunTeacherOffGates,
            passed: gates_passed,
            detail: Some(format!("{} clean-import gates", gates.len())),
        });

        let evidence: Vec<String> = target
            .episodes()
            .list_recent(MAX_LOCAL_EVIDENCE)?
            .iter()
            .map(|episode| episode.id.to_string())
            .collect();
        let locally_earned = tests_passed && gates_passed && !evidence.is_empty();
        let store = CapabilityStore::in_memory()?;
        let admitted = store.import_and_revalidate(
            &seed.bytes,
            &LocalValidation {
                passed: locally_earned,
                validation_episodes: evidence,
                environment_digest: environment_digest(&self.curriculum.id, &bundle.content_id),
            },
        )?;
        let promoted = locally_earned
            && admitted.locally_validated
            && admitted.status == spoon_capability::CapabilityStatus::Provisional;
        steps.push(ImportStepReport {
            step: ImportStep::PromoteOnlyLocalEvidence,
            passed: promoted,
            detail: Some(format!("target status {:?}", admitted.status)),
        });

        let passed = steps.iter().all(|step| step.passed);
        Ok(CleanImportReport {
            steps,
            gates,
            replayed_cases,
            promoted,
            authority_transferred: false,
            passed,
        })
    }
}

fn run_phase(
    engine: &mut Engine,
    phase: Phase,
    activities: &[Activity],
    session: &mut TeacherSession<'_>,
    cases: &mut Cases,
) -> PhaseReport {
    let mut reports = Vec::with_capacity(activities.len());
    for activity in activities {
        // The phase caps Teacher access and the activity can decline it. A
        // held-out activity never sees a Teacher even if the phase would allow
        // one, because that ablation is the whole point of the phase.
        let allowed = phase.teacher_allowed() && matches!(activity.teacher_mode, TeacherMode::On);
        let teacher = allowed.then_some(&mut *session);
        reports.push(run_activity(engine, activity, teacher, cases));
    }
    PhaseReport {
        phase,
        teacher_allowed: phase.teacher_allowed(),
        teacher_calls: reports.iter().map(|report| report.teacher_calls).sum(),
        passed: reports.iter().all(|report| report.passed),
        activities: reports,
    }
}

/// Replay the stage's probes with the Teacher removed.
///
/// A gate's `passCriteria` are prose, so what the runner checks is the
/// mechanical part: the Teacher is unreachable, the store is the one the gate
/// names, and every probe reaches the disposition the curriculum declared for
/// it with no outside help.
fn run_gate(
    engine: &mut Engine,
    curriculum: &Curriculum,
    gate: &TeacherOffGate,
    cases: &mut Cases,
) -> GateReport {
    let activities = match gate.stage {
        GateStage::Retention => &curriculum.demonstrations,
        GateStage::Composition => &curriculum.exercises,
        GateStage::HeldOutGeneralization | GateStage::CleanImport => {
            &curriculum.held_out_generalization
        }
    };
    let probes: Vec<ActivityReport> = activities
        .iter()
        .map(|activity| run_activity(engine, activity, None, cases))
        .collect();
    GateReport {
        id: gate.id.clone(),
        stage: gate.stage,
        store: gate.independent_store,
        teacher_calls: probes.iter().map(|probe| probe.teacher_calls).sum(),
        declared_criteria: gate.pass_criteria.clone(),
        failure_policy: gate.failure_policy,
        passed: !probes.is_empty() && probes.iter().all(|probe| probe.passed),
        probes,
    }
}

fn run_activity(
    engine: &mut Engine,
    activity: &Activity,
    teacher: Option<&mut TeacherSession<'_>>,
    cases: &mut Cases,
) -> ActivityReport {
    let teacher_allowed = teacher.is_some();
    let probe = probe(activity);
    let (result, teacher_calls) = drive(engine, probe, teacher);
    let expected = expected_observation(activity.expected_disposition);
    match result {
        Ok(outcome) => {
            capture_cases(engine, &outcome, cases);
            let observed = observed(&outcome);
            ActivityReport {
                id: activity.id.clone(),
                probe: probe.into(),
                teacher_allowed,
                teacher_calls,
                expected: activity.expected_disposition,
                observed: Some(observed),
                passed: observed == expected,
                failure: (observed != expected)
                    .then(|| format!("expected the engine to have {expected:?}, it {observed:?}")),
            }
        }
        Err(failure) => ActivityReport {
            id: activity.id.clone(),
            probe: probe.into(),
            teacher_allowed,
            teacher_calls,
            expected: activity.expected_disposition,
            observed: None,
            passed: false,
            failure: Some(failure),
        },
    }
}

/// The situation text a probe puts to the engine.
///
/// A curriculum deliberately carries no canned utterances, so the probe is the
/// activity's declared operation verbatim. That keeps the manifest the single
/// source of what gets asked, and the report records the exact text so a
/// reader can see what the engine actually received.
fn probe(activity: &Activity) -> &str {
    &activity.task_model.operation
}

fn drive(
    engine: &mut Engine,
    situation: &str,
    mut teacher: Option<&mut TeacherSession<'_>>,
) -> (Result<CycleOutcome, String>, u32) {
    let before = teacher.as_ref().map_or(0, |session| session.calls());
    let result = drive_cycle(engine, situation, teacher.as_deref_mut());
    let after = teacher.as_ref().map_or(0, |session| session.calls());
    (result, after - before)
}

fn drive_cycle(
    engine: &mut Engine,
    situation: &str,
    mut teacher: Option<&mut TeacherSession<'_>>,
) -> Result<CycleOutcome, String> {
    let allowed = teacher.is_some();
    let input = CycleInput {
        situation: situation.into(),
        working_directory: None,
        environment: BTreeMap::new(),
        assumptions: Vec::new(),
        budget: CycleBudget {
            max_exec_steps: MAX_EXEC_STEPS,
            max_context_items: MAX_CONTEXT_ITEMS,
            max_teacher_turns: if allowed { MAX_TEACHER_TURNS } else { 0 },
        },
        teacher_allowed: allowed,
        interpreter_allowed: false,
        session_id: None,
        recall_mode: RecallMode::Global,
        permission_mode: None,
    };
    let mut progress = engine
        .begin_cycle(input)
        .map_err(|error| error.to_string())?;
    for _ in 0..MAX_ROUNDS {
        match progress {
            CycleProgress::Completed(outcome) => return Ok(*outcome),
            CycleProgress::NeedIntent { cycle_id, .. } => {
                progress = engine
                    .skip_intent(cycle_id, "a forge run has no language interpreter")
                    .map_err(|error| error.to_string())?;
            }
            CycleProgress::NeedTeacher { cycle_id, request } => {
                let Some(session) = teacher.as_mut() else {
                    let _ = engine.abort_cycle(cycle_id, "the Teacher is disabled");
                    return Err(
                        "the engine asked for the Teacher during a Teacher-OFF probe".into(),
                    );
                };
                match session.respond(&request) {
                    Ok(proposal) => {
                        progress = engine
                            .resume_cycle(cycle_id, proposal)
                            .map_err(|error| error.to_string())?;
                    }
                    Err(error) => {
                        let reason = error.to_string();
                        let _ = engine.abort_cycle(cycle_id, reason.clone());
                        return Err(reason);
                    }
                }
            }
        }
    }
    Err(format!(
        "the cycle did not settle within {MAX_ROUNDS} rounds"
    ))
}

/// Keep every successful call the run observed, so the export ships evidence
/// the acquisition actually produced instead of hand-written fixtures.
///
/// A trace records call inputs positionally. Naming them against the stored
/// parameter list is what makes a case replayable in a target that assigned
/// the procedure a different identity.
fn capture_cases(engine: &Engine, outcome: &CycleOutcome, cases: &mut Cases) {
    for step in &outcome.episode.reasoning_trace.steps {
        if step.status != TraceStepStatus::Succeeded {
            continue;
        }
        let (Some(id), Some(input), Some(expected)) =
            (step.procedure_used, step.input.clone(), step.output.clone())
        else {
            continue;
        };
        let Ok(Some(procedure)) = engine.graph().get_procedure(id) else {
            continue;
        };
        let Some(inputs) = named_inputs(&procedure, input) else {
            continue;
        };
        let case = ReplayCase { inputs, expected };
        let recorded = cases.entry(id).or_default();
        if !recorded.contains(&case) {
            recorded.push(case);
        }
    }
}

fn named_inputs(procedure: &Procedure, input: Value) -> Option<BTreeMap<String, Value>> {
    match input {
        Value::Map(named) => Some(named),
        Value::List(positional) if positional.len() == procedure.params.len() => Some(
            procedure
                .params
                .iter()
                .map(|param| param.name.clone())
                .zip(positional)
                .collect(),
        ),
        _ => None,
    }
}

fn observed(outcome: &CycleOutcome) -> Observed {
    match outcome.disposition {
        CycleDisposition::Abstained => Observed::Abstained,
        CycleDisposition::Verified | CycleDisposition::Provisional => Observed::Answered,
    }
}

/// Collapse the curriculum's disposition vocabulary onto what the engine can
/// actually report. Everything other than `accept` means the engine must
/// decline rather than commit to an answer, which is exactly the property a
/// counterexample tests.
fn expected_observation(expected: ExpectedDisposition) -> Observed {
    match expected {
        ExpectedDisposition::Accept => Observed::Answered,
        ExpectedDisposition::Clarify
        | ExpectedDisposition::Reject
        | ExpectedDisposition::Abstain
        | ExpectedDisposition::FailClosed => Observed::Abstained,
    }
}

fn step(
    step: ImportStep,
    detail: Result<String, &spoon_capability::CapabilityError>,
    error: Option<&spoon_capability::CapabilityError>,
) -> ImportStepReport {
    ImportStepReport {
        step,
        passed: error.is_none(),
        detail: Some(match detail {
            Ok(detail) => detail,
            Err(error) => error.to_string(),
        }),
    }
}

fn halted_import(steps: Vec<ImportStepReport>) -> CleanImportReport {
    CleanImportReport {
        steps,
        gates: Vec::new(),
        replayed_cases: Vec::new(),
        promoted: false,
        authority_transferred: false,
        passed: false,
    }
}

fn refused_import() -> CleanImportReport {
    halted_import(vec![ImportStepReport {
        step: ImportStep::VerifyManifestAndContentHashes,
        passed: false,
        detail: Some("nothing was exported, so nothing could be imported".into()),
    }])
}

fn environment_digest(curriculum_id: &str, content_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(curriculum_id.as_bytes());
    digest.update(b"\0");
    digest.update(content_id.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}
