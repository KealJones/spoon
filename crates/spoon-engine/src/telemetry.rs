//! Durable, falsification-oriented telemetry for the Section 38 metrics.
//!
//! This store deliberately records measurements supplied by a benchmark or
//! probe runner; it does not infer favourable outcomes from ordinary episodes.
//! Every row is immutable, explicitly labelled by cohort and teacher mode, and
//! the report refuses to score a metric when its required evidence is absent.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::EngineError;

pub const MAX_FALSIFICATION_RUNS: usize = 1_024;
pub const MAX_FALSIFICATION_MEASUREMENTS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeCohort {
    Training,
    HeldOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeacherMode {
    On,
    Off,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroundingTier {
    None,
    Teacher,
    Soft,
    Strong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FalsificationRunInput {
    pub label: String,
    pub benchmark: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FalsificationRun {
    pub id: String,
    pub label: String,
    pub benchmark: String,
    pub notes: Option<String>,
    pub created_at: i64,
}

/// One immutable benchmark/probe observation.
///
/// `probe_id` identifies the intended task. `novelty_identity` identifies the
/// concrete input/task content. A retry or ablation must name its original
/// measurement in `repeat_of`; it can then be shown, but is automatically
/// excluded from acquisition/transfer evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FalsificationMeasurementInput {
    pub domain: String,
    pub family: String,
    pub cohort: ProbeCohort,
    pub probe_id: String,
    pub novelty_identity: String,
    #[serde(default)]
    pub repeat_of: Option<String>,
    pub teacher_mode: TeacherMode,
    pub teacher_used: bool,
    pub teacher_calls: u32,
    pub rung: String,
    pub steps: u32,
    pub candidates: u32,
    pub trace_steps: u32,
    pub cost: f64,
    pub abstained: bool,
    pub clarified: bool,
    #[serde(default)]
    pub confidence: Option<f64>,
    pub grounding_tier: GroundingTier,
    #[serde(default)]
    pub used_skill_id: Option<String>,
    #[serde(default)]
    pub created_skill_id: Option<String>,
    #[serde(default)]
    pub correct: Option<bool>,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub baseline_trace_steps: Option<u32>,
    #[serde(default)]
    pub regression_probe: bool,
    #[serde(default)]
    pub attribution_correct: Option<bool>,
    #[serde(default)]
    pub attribution_cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FalsificationMeasurement {
    pub id: String,
    pub run_id: String,
    pub recorded_at: i64,
    #[serde(flatten)]
    pub observation: FalsificationMeasurementInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetricEvidenceStatus {
    Measured,
    InsufficientEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section38Metric {
    pub slot: u8,
    pub name: String,
    pub status: MetricEvidenceStatus,
    /// Number of eligible observations; it is never a fabricated denominator.
    pub sample_size: u64,
    pub value: Option<f64>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section38TelemetrySnapshot {
    pub runs: u64,
    pub measurements: u64,
    pub failures: u64,
    pub abstentions: u64,
    pub clarifications: u64,
    pub teacher_off_violations_rejected: u64,
    pub duplicate_measurements_rejected: u64,
    pub cohort_leakage_rejected: u64,
    pub metrics: Vec<Section38Metric>,
}

pub(crate) struct FalsificationTelemetryStore {
    conn: Connection,
}

impl FalsificationTelemetryStore {
    pub(crate) fn open(path: &str) -> Result<Self, EngineError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub(crate) fn in_memory() -> Result<Self, EngineError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, EngineError> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS engine_falsification_runs (
                 id TEXT PRIMARY KEY,
                 created_at INTEGER NOT NULL,
                 payload_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS engine_falsification_measurements (
                 id TEXT PRIMARY KEY,
                 run_id TEXT NOT NULL REFERENCES engine_falsification_runs(id),
                 recorded_at INTEGER NOT NULL,
                 probe_id TEXT NOT NULL,
                 novelty_identity TEXT NOT NULL,
                 payload_json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS engine_falsification_measurements_run_idx
                 ON engine_falsification_measurements(run_id, recorded_at DESC);
             CREATE TABLE IF NOT EXISTS engine_falsification_rejections (
                 kind TEXT PRIMARY KEY,
                 count INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    pub(crate) fn create_run(
        &self,
        input: FalsificationRunInput,
    ) -> Result<FalsificationRun, EngineError> {
        require_text("run label", &input.label)?;
        require_text("benchmark", &input.benchmark)?;
        let run = FalsificationRun {
            id: Uuid::new_v4().to_string(),
            label: input.label,
            benchmark: input.benchmark,
            notes: input.notes,
            created_at: now_seconds(),
        };
        self.conn.execute(
            "INSERT INTO engine_falsification_runs(id, created_at, payload_json) VALUES (?1, ?2, ?3)",
            params![run.id, run.created_at, serde_json::to_string(&run)?],
        )?;
        self.prune()?;
        Ok(run)
    }

    pub(crate) fn record(
        &self,
        run_id: &str,
        observation: FalsificationMeasurementInput,
    ) -> Result<FalsificationMeasurement, EngineError> {
        self.validate(&observation)?;
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM engine_falsification_runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(EngineError::InvalidInput(format!(
                "unknown falsification run {run_id}"
            )));
        }
        // Novelty identity is global to the local telemetry database. A new
        // benchmark run cannot make an exact task look novel again.
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM engine_falsification_measurements
             WHERE probe_id = ?1 AND novelty_identity = ?2
             ORDER BY recorded_at ASC LIMIT 1",
                params![observation.probe_id, observation.novelty_identity],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing
            && observation.repeat_of.as_deref() != Some(existing.as_str())
        {
            self.reject("duplicateMeasurements")?;
            return Err(EngineError::InvalidInput(
                "a repeated probe/novelty pair must declare repeatOf; repeats are excluded from acquisition and transfer".into(),
            ));
        }
        if observation.repeat_of.is_none() && self.cross_cohort_family(&observation)? {
            self.reject("cohortLeakage")?;
            return Err(EngineError::InvalidInput(
                "a task family cannot be both training and held-out evidence".into(),
            ));
        }
        let measurement = FalsificationMeasurement {
            id: Uuid::new_v4().to_string(),
            run_id: run_id.to_owned(),
            recorded_at: now_seconds(),
            observation,
        };
        self.conn.execute(
            "INSERT INTO engine_falsification_measurements
             (id, run_id, recorded_at, probe_id, novelty_identity, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                measurement.id,
                measurement.run_id,
                measurement.recorded_at,
                measurement.observation.probe_id,
                measurement.observation.novelty_identity,
                serde_json::to_string(&measurement)?,
            ],
        )?;
        self.prune()?;
        Ok(measurement)
    }

    pub(crate) fn snapshot(&self) -> Result<Section38TelemetrySnapshot, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json FROM engine_falsification_measurements ORDER BY recorded_at ASC, id ASC",
        )?;
        let records = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| {
                let payload = row?;
                serde_json::from_str::<FalsificationMeasurement>(&payload).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        payload.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let runs: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM engine_falsification_runs",
            [],
            |row| row.get(0),
        )?;
        let failures = records
            .iter()
            .filter(|record| record.observation.correct == Some(false))
            .count() as u64;
        let abstentions = records
            .iter()
            .filter(|record| record.observation.abstained)
            .count() as u64;
        let clarifications = records
            .iter()
            .filter(|record| record.observation.clarified)
            .count() as u64;
        Ok(Section38TelemetrySnapshot {
            runs,
            measurements: records.len() as u64,
            failures,
            abstentions,
            clarifications,
            teacher_off_violations_rejected: self.rejections("teacherOffViolations")?,
            duplicate_measurements_rejected: self.rejections("duplicateMeasurements")?,
            cohort_leakage_rejected: self.rejections("cohortLeakage")?,
            metrics: report(&records),
        })
    }

    fn cross_cohort_family(
        &self,
        observation: &FalsificationMeasurementInput,
    ) -> Result<bool, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json FROM engine_falsification_measurements
             WHERE novelty_identity != ?1",
        )?;
        let rows = statement.query_map([observation.novelty_identity.as_str()], |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            let payload = row?;
            let prior: FalsificationMeasurement = serde_json::from_str(&payload)?;
            if prior.observation.repeat_of.is_none()
                && prior.observation.domain == observation.domain
                && prior.observation.family == observation.family
                && prior.observation.cohort != observation.cohort
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn validate(&self, value: &FalsificationMeasurementInput) -> Result<(), EngineError> {
        for (label, text) in [
            ("domain", &value.domain),
            ("family", &value.family),
            ("probe id", &value.probe_id),
            ("novelty identity", &value.novelty_identity),
            ("rung", &value.rung),
        ] {
            require_text(label, text)?;
        }
        if !value.cost.is_finite() || value.cost < 0.0 {
            return Err(EngineError::InvalidInput(
                "cost must be finite and non-negative".into(),
            ));
        }
        if let Some(confidence) = value.confidence
            && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err(EngineError::InvalidInput(
                "confidence must be between 0 and 1".into(),
            ));
        }
        if let Some(cost) = value.attribution_cost
            && (!cost.is_finite() || cost < 0.0)
        {
            return Err(EngineError::InvalidInput(
                "attribution cost must be finite and non-negative".into(),
            ));
        }
        if let Some(baseline) = value.baseline_trace_steps
            && baseline < value.trace_steps
        {
            return Err(EngineError::InvalidInput(
                "baseline trace steps cannot be lower than the measured trace steps".into(),
            ));
        }
        if value.teacher_mode == TeacherMode::Off
            && (value.teacher_used || value.teacher_calls != 0)
        {
            self.reject("teacherOffViolations")?;
            return Err(EngineError::InvalidInput(
                "teacher-off measurement cannot use or call a teacher".into(),
            ));
        }
        if value.teacher_mode == TeacherMode::Off && value.grounding_tier == GroundingTier::Teacher
        {
            self.reject("teacherOffViolations")?;
            return Err(EngineError::InvalidInput(
                "teacher-off measurement cannot claim teacher grounding".into(),
            ));
        }
        if value.cohort == ProbeCohort::HeldOut && value.created_skill_id.is_some() {
            self.reject("heldOutTrainingViolations")?;
            return Err(EngineError::InvalidInput(
                "held-out measurements cannot create or train a skill".into(),
            ));
        }
        if value.abstained && value.correct.is_some() {
            return Err(EngineError::InvalidInput(
                "abstentions must be represented separately from correctness".into(),
            ));
        }
        if value.correct == Some(true) && value.failure_reason.is_some() {
            return Err(EngineError::InvalidInput(
                "successful observations cannot carry a failure reason".into(),
            ));
        }
        if value.correct == Some(false)
            && value
                .failure_reason
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(EngineError::InvalidInput(
                "failed observations require a failure reason".into(),
            ));
        }
        Ok(())
    }

    fn reject(&self, kind: &str) -> Result<(), EngineError> {
        self.conn.execute(
            "INSERT INTO engine_falsification_rejections(kind, count) VALUES (?1, 1)
             ON CONFLICT(kind) DO UPDATE SET count = count + 1",
            [kind],
        )?;
        Ok(())
    }

    fn rejections(&self, kind: &str) -> Result<u64, EngineError> {
        Ok(self
            .conn
            .query_row(
                "SELECT count FROM engine_falsification_rejections WHERE kind = ?1",
                [kind],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    fn prune(&self) -> Result<(), EngineError> {
        self.conn.execute(
            "DELETE FROM engine_falsification_measurements WHERE id IN (
                SELECT id FROM engine_falsification_measurements
                ORDER BY recorded_at DESC, id DESC LIMIT -1 OFFSET ?1
             )",
            [MAX_FALSIFICATION_MEASUREMENTS as i64],
        )?;
        self.conn.execute(
            "DELETE FROM engine_falsification_measurements WHERE run_id IN (
                SELECT id FROM engine_falsification_runs ORDER BY created_at DESC, id DESC
                LIMIT -1 OFFSET ?1
             )",
            [MAX_FALSIFICATION_RUNS as i64],
        )?;
        self.conn.execute(
            "DELETE FROM engine_falsification_runs WHERE id IN (
                SELECT id FROM engine_falsification_runs ORDER BY created_at DESC, id DESC
                LIMIT -1 OFFSET ?1
             )",
            [MAX_FALSIFICATION_RUNS as i64],
        )?;
        Ok(())
    }
}

fn report(records: &[FalsificationMeasurement]) -> Vec<Section38Metric> {
    let eligible = records
        .iter()
        .filter(|r| r.observation.repeat_of.is_none())
        .collect::<Vec<_>>();
    let successes = |items: &[&FalsificationMeasurement]| {
        items
            .iter()
            .filter(|r| r.observation.correct == Some(true))
            .count() as f64
    };
    let rate = |items: &[&FalsificationMeasurement]| {
        (!items.is_empty()).then(|| successes(items) / items.len() as f64)
    };
    let insufficient = |slot, name: &str, sample_size, need: &str| Section38Metric {
        slot,
        name: name.into(),
        status: MetricEvidenceStatus::InsufficientEvidence,
        sample_size,
        value: None,
        detail: need.into(),
    };
    let measured = |slot, name: &str, sample_size, value, detail: String| Section38Metric {
        slot,
        name: name.into(),
        status: MetricEvidenceStatus::Measured,
        sample_size,
        value: Some(value),
        detail,
    };

    let acquisition = eligible
        .iter()
        .copied()
        .filter(|r| r.observation.created_skill_id.is_some() && r.observation.correct == Some(true))
        .collect::<Vec<_>>();
    let compounding = if acquisition.len() >= 2 {
        let first = acquisition.first().unwrap().observation.cost;
        let last = acquisition.last().unwrap().observation.cost;
        if first > 0.0 {
            measured(1, "Compounding", acquisition.len() as u64, (first - last) / first, "Cost change from first to latest successful non-repeat skill acquisition; comparable task sequencing remains the caller's responsibility.".into())
        } else {
            insufficient(
                1,
                "Compounding",
                acquisition.len() as u64,
                "First eligible acquisition has zero cost; a cost ratio is undefined.",
            )
        }
    } else {
        insufficient(
            1,
            "Compounding",
            acquisition.len() as u64,
            "Need at least two successful non-repeat skill acquisitions with comparable task sequencing.",
        )
    };

    let transfer = eligible
        .iter()
        .copied()
        .filter(|r| {
            r.observation.cohort == ProbeCohort::HeldOut && r.observation.used_skill_id.is_some()
        })
        .collect::<Vec<_>>();
    let transfer_metric = rate(&transfer)
        .map(|value| {
            measured(
                2,
                "Transfer",
                transfer.len() as u64,
                value,
                "Held-out, non-repeat probes that used a skill; exact repeats are excluded.".into(),
            )
        })
        .unwrap_or_else(|| {
            insufficient(
                2,
                "Transfer",
                0,
                "Need held-out non-repeat probes that exercised a skill.",
            )
        });

    let all = records.iter().collect::<Vec<_>>();
    let mut weaning_pairs = Vec::new();
    let mut domains = BTreeSet::new();
    for record in &all {
        domains.insert((
            record.observation.domain.as_str(),
            record.observation.family.as_str(),
        ));
    }
    for (domain, family) in domains {
        let on = all
            .iter()
            .copied()
            .filter(|r| {
                r.observation.domain == domain
                    && r.observation.family == family
                    && r.observation.teacher_used
            })
            .collect::<Vec<_>>();
        let off = all
            .iter()
            .copied()
            .filter(|r| {
                r.observation.domain == domain
                    && r.observation.family == family
                    && r.observation.teacher_mode == TeacherMode::Off
            })
            .collect::<Vec<_>>();
        if let (Some(on), Some(off)) = (rate(&on), rate(&off)) {
            weaning_pairs.push(off - on);
        }
    }
    let weaning = if weaning_pairs.is_empty() {
        insufficient(
            3,
            "Per-domain weaning",
            0,
            "Need teacher-assisted and teacher-off successful/failed evidence in the same domain/family.",
        )
    } else {
        measured(3, "Per-domain weaning", weaning_pairs.len() as u64, weaning_pairs.iter().sum::<f64>() / weaning_pairs.len() as f64, "Mean teacher-off minus teacher-assisted success rate across comparable domain/family cohorts.".into())
    };

    let compression = all
        .iter()
        .copied()
        .filter_map(|r| {
            r.observation
                .baseline_trace_steps
                .filter(|baseline| *baseline > 0)
                .map(|baseline| (r, baseline))
        })
        .collect::<Vec<_>>();
    let compression_metric = if compression.is_empty() {
        insufficient(
            4,
            "Trace compression",
            0,
            "Need explicit baselineTraceSteps paired with measured trace steps.",
        )
    } else {
        measured(
            4,
            "Trace compression",
            compression.len() as u64,
            compression
                .iter()
                .map(|(r, baseline)| 1.0 - r.observation.trace_steps as f64 / *baseline as f64)
                .sum::<f64>()
                / compression.len() as f64,
            "Mean reduction against explicitly supplied paired baselines.".into(),
        )
    };

    let rungs = all
        .iter()
        .fold(BTreeMap::<String, u64>::new(), |mut counts, r| {
            *counts.entry(r.observation.rung.clone()).or_default() += 1;
            counts
        });
    let rung = if rungs.is_empty() {
        insufficient(5, "Rung distribution", 0, "No measurements recorded.")
    } else {
        measured(
            5,
            "Rung distribution",
            all.len() as u64,
            rungs.len() as f64,
            format!(
                "{} rung categories across {} measurements; value is category count.",
                rungs.len(),
                all.len()
            ),
        )
    };

    let regressions = all
        .iter()
        .copied()
        .filter(|r| r.observation.regression_probe)
        .collect::<Vec<_>>();
    let regression = rate(&regressions).map(|value| measured(6, "No regression", regressions.len() as u64, value, "Explicit regression probes; value is observed pass rate, not a blanket no-regression claim.".into())).unwrap_or_else(|| insufficient(6, "No regression", 0, "Need explicit regressionProbe measurements with observed correctness."));

    let attribution = all
        .iter()
        .copied()
        .filter(|r| r.observation.attribution_correct.is_some())
        .collect::<Vec<_>>();
    let attribution_accuracy = if attribution.is_empty() {
        insufficient(
            7,
            "Attribution accuracy",
            0,
            "Need benchmarked attribution outcomes, including injected-fault failures.",
        )
    } else {
        measured(
            7,
            "Attribution accuracy",
            attribution.len() as u64,
            attribution
                .iter()
                .filter(|r| r.observation.attribution_correct == Some(true))
                .count() as f64
                / attribution.len() as f64,
            "Observed attribution correctness; abstentions are not silently counted as correct."
                .into(),
        )
    };

    let attribution_cost = all
        .iter()
        .copied()
        .filter_map(|r| r.observation.attribution_cost.map(|cost| (r, cost)))
        .collect::<Vec<_>>();
    let attribution_cost_metric = if attribution_cost.is_empty() {
        insufficient(
            8,
            "Attribution cost",
            0,
            "Need attributionCost and total cost from the same measurements.",
        )
    } else {
        measured(8, "Attribution cost", attribution_cost.len() as u64, attribution_cost.iter().map(|(r, cost)| if r.observation.cost == 0.0 { 0.0 } else { cost / r.observation.cost }).sum::<f64>() / attribution_cost.len() as f64, "Mean attribution-cost / total-cost ratio; zero-total-cost rows contribute zero rather than an invented ratio.".into())
    };

    let mut ablations = Vec::new();
    let mut by_probe = BTreeMap::<&str, Vec<&FalsificationMeasurement>>::new();
    for record in &all {
        by_probe
            .entry(&record.observation.probe_id)
            .or_default()
            .push(record);
    }
    for values in by_probe.values() {
        let on = values
            .iter()
            .copied()
            .filter(|r| r.observation.teacher_used)
            .collect::<Vec<_>>();
        let off = values
            .iter()
            .copied()
            .filter(|r| r.observation.teacher_mode == TeacherMode::Off)
            .collect::<Vec<_>>();
        if let (Some(on), Some(off)) = (rate(&on), rate(&off)) {
            ablations.push(off - on);
        }
    }
    let ablation = if ablations.is_empty() {
        insufficient(
            9,
            "Teacher ablation",
            0,
            "Need paired same-probe teacher-on and teacher-off observations; teacher-off claims are validated at write time.",
        )
    } else {
        measured(
            9,
            "Teacher ablation",
            ablations.len() as u64,
            ablations.iter().sum::<f64>() / ablations.len() as f64,
            "Mean teacher-off minus teacher-on success rate over paired probe identities.".into(),
        )
    };

    let grounded = all
        .iter()
        .copied()
        .filter(|r| {
            matches!(
                r.observation.grounding_tier,
                GroundingTier::Soft | GroundingTier::Strong
            )
        })
        .collect::<Vec<_>>();
    let grounding = if all.is_empty() {
        insufficient(10, "Grounding drift", 0, "No measurements recorded.")
    } else {
        measured(
            10,
            "Grounding drift",
            all.len() as u64,
            grounded.len() as f64 / all.len() as f64,
            "Grounded-observation share, not a belief-level drift claim.".into(),
        )
    };

    let survival = all
        .iter()
        .copied()
        .filter(|r| r.observation.used_skill_id.is_some())
        .collect::<Vec<_>>();
    let survival_metric = rate(&survival)
        .map(|value| {
            measured(
                11,
                "Abstraction survival",
                survival.len() as u64,
                value,
                "Post-acquisition uses of a named skill; value is observed success rate.".into(),
            )
        })
        .unwrap_or_else(|| {
            insufficient(
                11,
                "Abstraction survival",
                0,
                "Need post-acquisition uses of named skills.",
            )
        });

    let calibration = all
        .iter()
        .copied()
        .filter(|r| r.observation.confidence.is_some() && r.observation.correct.is_some())
        .collect::<Vec<_>>();
    let calibration_metric = if calibration.is_empty() {
        insufficient(
            12,
            "Calibration",
            0,
            "Need confidence paired with observed correctness; abstentions remain separate.",
        )
    } else {
        measured(
            12,
            "Calibration",
            calibration.len() as u64,
            calibration
                .iter()
                .map(|r| {
                    let target = if r.observation.correct == Some(true) {
                        1.0
                    } else {
                        0.0
                    };
                    let confidence = r.observation.confidence.unwrap_or_default();
                    (confidence - target).powi(2)
                })
                .sum::<f64>()
                / calibration.len() as f64,
            "Brier score (lower is better) on confidence/correctness pairs.".into(),
        )
    };

    vec![
        compounding,
        transfer_metric,
        weaning,
        compression_metric,
        rung,
        regression,
        attribution_accuracy,
        attribution_cost_metric,
        ablation,
        grounding,
        survival_metric,
        calibration_metric,
    ]
}

fn require_text(label: &str, value: &str) -> Result<(), EngineError> {
    if value.trim().is_empty() {
        return Err(EngineError::InvalidInput(format!(
            "{label} must be non-empty"
        )));
    }
    Ok(())
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
