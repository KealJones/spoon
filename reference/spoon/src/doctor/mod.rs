//! `spoon doctor [--since <window>] [--limit N] [--json] [--all] [--build <id>]`
//!
//! Reads stored episodes and buckets them by failure signature, then clusters
//! within each bucket by **symptom** - the diagnosis of what went wrong.
//! Two turns with very different phrasing that need the same fix land in one
//! cluster.
//!
//! By default the report is scoped to the CURRENT build (stamped at compile
//! time). Pass `--all` for the lifetime view or `--build <id>` to pin a
//! specific build.

pub mod detect;
pub mod report;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use spoon_core::types::{EarsPath, Episode};
use spoon_mind::brain::Brain;

pub use detect::SymptomResult;
pub use report::{bucket_label, input_shape, symptom_label};

// ---- args ------------------------------------------------------------------

pub struct DoctorArgs {
    pub since_ms: Option<i64>,
    pub limit: usize,
    pub json: bool,
    /// Show all builds instead of defaulting to current.
    pub all: bool,
    /// Show only episodes from a specific build id.
    pub build: Option<String>,
}

/// Parse `7d`, `24h`, `2w` into a millisecond-epoch cutoff.
pub fn parse_since(s: &str) -> anyhow::Result<i64> {
    let s = s.trim();
    let (n, unit) = if let Some(rest) = s.strip_suffix('d') {
        (rest.parse::<u64>()?, 86_400_000u64)
    } else if let Some(rest) = s.strip_suffix('h') {
        (rest.parse::<u64>()?, 3_600_000u64)
    } else if let Some(rest) = s.strip_suffix('w') {
        (rest.parse::<u64>()?, 7 * 86_400_000u64)
    } else {
        anyhow::bail!("unknown since format '{}'; use e.g. 7d, 24h, 2w", s);
    };
    let now = chrono::Utc::now().timestamp_millis() as u64;
    Ok(now.saturating_sub(n * unit) as i64)
}

// ---- context for analysis --------------------------------------------------

/// Context passed to `analyse_with_context` that carries build scope info.
pub struct AnalyseContext {
    /// The build id of the currently running binary.
    pub current_build: String,
    /// Build being reported on. Same as `current_build` in default mode;
    /// empty in `--all` mode; a specific id when `--build` was used.
    pub report_build: String,
    /// Episodes excluded from this analysis window (pre-filtered out).
    pub excluded_count: usize,
    /// True if --all was passed (all builds in window).
    pub all_mode: bool,
}

impl Default for AnalyseContext {
    fn default() -> Self {
        let build = spoon_core::BUILD_ID.to_string();
        AnalyseContext {
            current_build: build.clone(),
            report_build: build,
            excluded_count: 0,
            all_mode: false,
        }
    }
}

// ---- report types ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeaningNumbers {
    pub ears_native: u64,
    pub ears_llm: u64,
    pub ears_failed: u64,
    pub mouth_llm: u64,
    pub teacher_llm: u64,
    pub interior_llm_calls: u64,
}

/// One example inside a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleRow {
    pub user_text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sce: String,
    /// Coarse input shape (shown as a detail, not as the cluster key).
    pub shape: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secondary_symptoms: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub details: String,
    /// Build id for this example - useful in --all mode.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub build_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterReport {
    pub symptom: String,
    pub count: usize,
    /// How many of these are from the current build (same as count outside --all mode).
    pub current_build_count: usize,
    pub examples: Vec<ExampleRow>,
    pub suggests: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketReport {
    pub bucket: String,
    pub count: usize,
    pub clusters: Vec<ClusterReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Build being reported on.
    pub build_id: String,
    /// How many episodes from the full window were excluded.
    pub excluded_count: usize,
    /// True when --all was passed.
    pub all_mode: bool,
    /// Number of episodes with no build stamp (pre-dates the field).
    pub unstamped_count: usize,
    pub total: usize,
    pub failures: usize,
    pub failure_rate_pct: f64,
    pub honest_unknowns: usize,
    pub slow: usize,
    pub weaning: WeaningNumbers,
    pub interior_llm_violation: bool,
    pub buckets: Vec<BucketReport>,
}

// ---- run -------------------------------------------------------------------

pub async fn run(brain: Arc<Brain>, args: &DoctorArgs) -> anyhow::Result<()> {
    if brain.cfg.db_path.is_none() {
        anyhow::bail!(
            "spoon doctor requires a persistent database; \
             --ephemeral has no history to diagnose"
        );
    }
    let all_episodes = brain.store.lock().episodes_window(args.since_ms, args.limit)?;

    let current_build = spoon_core::BUILD_ID;
    let (episodes, ctx) = build_context(all_episodes, current_build, &args.build, args.all);

    let report = analyse_with_context(&episodes, &ctx);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report::print_report(&report);
    }
    Ok(())
}

/// Pre-filter episodes and build the `AnalyseContext`.
fn build_context(
    all: Vec<Episode>,
    current_build: &str,
    build_flag: &Option<String>,
    all_mode: bool,
) -> (Vec<Episode>, AnalyseContext) {
    if all_mode {
        let unstamped = all.iter().filter(|e| e.build_id.is_empty()).count();
        let ctx = AnalyseContext {
            current_build: current_build.to_string(),
            report_build: String::new(),
            excluded_count: 0,
            all_mode: true,
        };
        // We pass everything; unstamped count is stored in ctx for the report.
        // Pass it via report_build being empty as sentinel; unstamped tracked by analyse.
        let _ = unstamped; // tracked inside analyse_with_context
        return (all, ctx);
    }

    let target = build_flag.as_deref().unwrap_or(current_build);
    let (included, excluded): (Vec<_>, Vec<_>) =
        all.into_iter().partition(|e| !e.build_id.is_empty() && e.build_id == target);

    let ctx = AnalyseContext {
        current_build: current_build.to_string(),
        report_build: target.to_string(),
        excluded_count: excluded.len(),
        all_mode: false,
    };
    (included, ctx)
}

// ---- analysis --------------------------------------------------------------

/// Convenience wrapper with default context (current build, no filter).
pub fn analyse(episodes: &[Episode]) -> DoctorReport {
    analyse_with_context(episodes, &AnalyseContext::default())
}

pub fn analyse_with_context(episodes: &[Episode], ctx: &AnalyseContext) -> DoctorReport {
    let total = episodes.len();
    let mut weaning = WeaningNumbers {
        ears_native: 0,
        ears_llm: 0,
        ears_failed: 0,
        mouth_llm: 0,
        teacher_llm: 0,
        interior_llm_calls: 0,
    };
    let mut interior_violation = false;
    let mut slow = 0usize;
    let mut honest_unknowns = 0usize;
    let mut unstamped_count = 0usize;

    // bucket name -> Vec<(episode_ref, symptom_result)>
    let mut raw: HashMap<&'static str, Vec<(usize, SymptomResult)>> = HashMap::new();

    for (idx, ep) in episodes.iter().enumerate() {
        if ep.build_id.is_empty() {
            unstamped_count += 1;
        }

        let m = &ep.metrics;
        match &m.ears_path {
            Some(EarsPath::Direct) | Some(EarsPath::Phrasing) | Some(EarsPath::Retrieval) => {
                weaning.ears_native += 1
            }
            Some(EarsPath::Llm) => weaning.ears_llm += 1,
            Some(EarsPath::Failed) | None => weaning.ears_failed += 1,
        }
        weaning.mouth_llm += m.mouth_llm_calls as u64;
        weaning.teacher_llm += m.teacher_llm_calls as u64;
        weaning.interior_llm_calls += m.interior_llm_calls as u64;
        if m.interior_llm_calls > 0 {
            interior_violation = true;
        }
        if m.ms_ears + m.ms_interior + m.ms_mouth > 3_000 {
            slow += 1;
        }

        let is_failed = matches!(m.ears_path, Some(EarsPath::Failed));
        if is_failed {
            if ep.sce.trim().is_empty() {
                let sr = SymptomResult {
                    primary: "no_llm_output".into(),
                    secondaries: vec![],
                    details: String::new(),
                };
                raw.entry("ears_no_parse").or_default().push((idx, sr));
            } else {
                let sr = detect::detect_symptom(&ep.user_text, &ep.sce);
                raw.entry("ears_didnt_reparse").or_default().push((idx, sr));
            }
        } else {
            if detect::is_honest_unknown(&ep.reply_text) {
                honest_unknowns += 1;
                continue;
            }
            if detect::has_clarify_move(ep) {
                let sr = SymptomResult {
                    primary: "clarify".into(),
                    secondaries: vec![],
                    details: String::new(),
                };
                raw.entry("asked_to_rephrase").or_default().push((idx, sr));
            }
            if detect::is_runtime_error(ep) {
                let sr = SymptomResult {
                    primary: "runtime".into(),
                    secondaries: vec![],
                    details: String::new(),
                };
                raw.entry("runtime_error").or_default().push((idx, sr));
            }
            if m.synthesis_attempted && !m.synthesis_succeeded {
                let sr = SymptomResult {
                    primary: "synthesis".into(),
                    secondaries: vec![],
                    details: String::new(),
                };
                raw.entry("synthesis_failed").or_default().push((idx, sr));
            }
        }
    }

    const BUCKET_ORDER: &[&str] = &[
        "ears_no_parse",
        "ears_didnt_reparse",
        "asked_to_rephrase",
        "runtime_error",
        "synthesis_failed",
    ];

    let mut failures = 0usize;
    let mut buckets: Vec<BucketReport> = Vec::new();

    for &bname in BUCKET_ORDER {
        if let Some(entries) = raw.get(bname) {
            failures += entries.len();
            let clusters = cluster_by_symptom(entries, episodes, &ctx.current_build, ctx.all_mode);
            buckets.push(BucketReport {
                bucket: bname.to_string(),
                count: entries.len(),
                clusters,
            });
        }
    }

    buckets.sort_by(|a, b| b.count.cmp(&a.count));

    let failure_rate_pct = if total > 0 {
        failures as f64 / total as f64 * 100.0
    } else {
        0.0
    };

    let build_id = if ctx.all_mode {
        format!("all builds (current: {})", ctx.current_build)
    } else if ctx.report_build.is_empty() {
        ctx.current_build.clone()
    } else {
        ctx.report_build.clone()
    };

    DoctorReport {
        build_id,
        excluded_count: ctx.excluded_count,
        all_mode: ctx.all_mode,
        unstamped_count,
        total,
        failures,
        failure_rate_pct,
        honest_unknowns,
        slow,
        weaning,
        interior_llm_violation: interior_violation,
        buckets,
    }
}

// ---- clustering ------------------------------------------------------------

fn cluster_by_symptom(
    entries: &[(usize, SymptomResult)],
    episodes: &[Episode],
    current_build: &str,
    all_mode: bool,
) -> Vec<ClusterReport> {
    let mut by_symptom: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, sr) in entries {
        by_symptom.entry(sr.primary.clone()).or_default().push(*idx);
    }

    let total_failures = entries.len();

    let mut clusters: Vec<ClusterReport> = by_symptom
        .into_iter()
        .map(|(symptom, ep_indices)| {
            let count = ep_indices.len();
            let current_build_count = if all_mode {
                ep_indices
                    .iter()
                    .filter(|&&i| episodes[i].build_id == current_build)
                    .count()
            } else {
                count
            };

            let examples: Vec<ExampleRow> = ep_indices
                .iter()
                .take(3)
                .map(|&i| {
                    let ep = &episodes[i];
                    // Find the symptom result for this episode index
                    let sr = entries
                        .iter()
                        .find(|(idx, _)| *idx == i)
                        .map(|(_, sr)| sr)
                        .expect("index must be in entries");
                    ExampleRow {
                        user_text: ep.user_text.clone(),
                        sce: ep.sce.clone(),
                        shape: report::input_shape(&ep.user_text),
                        secondary_symptoms: sr.secondaries.clone(),
                        details: sr.details.clone(),
                        build_id: if all_mode { ep.build_id.clone() } else { String::new() },
                    }
                })
                .collect();

            let suggests = report::symptom_suggests(&symptom, count, total_failures);
            ClusterReport { symptom, count, current_build_count, examples, suggests }
        })
        .collect();

    clusters.sort_by(|a, b| b.count.cmp(&a.count));
    clusters
}
