//! `spoon bench <corpus>`: convo20 and ace run the ears benches with the
//! Brain's own ears; babi and demo run whole turns. Every run prints a table,
//! writes a JSON record under data/bench/results, reports the weaning numbers
//! and fails loudly if the interior touched an LLM.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use spoon_core::types::EarsPath;
use spoon_lang::ears::bench::{run_ace, run_convo};
use spoon_mind::brain::{Brain, TurnResult};

pub const CORPORA: &[&str] = &["convo20", "ace", "babi", "demo"];

/// The numbers the weaning curve is drawn from: how often the ears needed
/// the LLM, and that the interior never did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Weaning {
    pub turns: u64,
    pub ears_native: u64,
    pub ears_llm: u64,
    pub ears_failed: u64,
    pub ears_llm_calls: u64,
    pub mouth_llm: u64,
    pub teacher_llm: u64,
    pub interior_llm_calls: u64,
}

impl Weaning {
    fn count_path(&mut self, path: &str) {
        self.turns += 1;
        match path.trim_end_matches('*') {
            "direct" | "phrasing" | "retrieval" => self.ears_native += 1,
            "llm" => self.ears_llm += 1,
            _ => self.ears_failed += 1,
        }
    }

    fn count_turn(&mut self, r: &TurnResult) {
        let m = &r.episode.metrics;
        self.count_path(&path_label(r));
        self.ears_llm_calls += m.ears_llm_calls as u64;
        self.mouth_llm += m.mouth_llm_calls as u64;
        self.teacher_llm += m.teacher_llm_calls as u64;
        self.interior_llm_calls += m.interior_llm_calls as u64;
    }
}

impl std::fmt::Display for Weaning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "weaning: turns={} ears_native={} ears_llm={} ears_failed={} ears_llm_calls={} mouth_llm={} teacher_llm={} interior_llm_calls={}",
            self.turns,
            self.ears_native,
            self.ears_llm,
            self.ears_failed,
            self.ears_llm_calls,
            self.mouth_llm,
            self.teacher_llm,
            self.interior_llm_calls
        )
    }
}

/// One finished run: what gets printed and what gets written.
#[derive(Serialize)]
pub struct BenchRecord {
    pub corpus: String,
    /// Ears model name, or "offline".
    pub model: String,
    pub at: String,
    pub elapsed_ms: u64,
    pub passed: usize,
    pub total: usize,
    pub weaning: Weaning,
    pub report: serde_json::Value,
    #[serde(skip)]
    pub table: String,
}

struct Outcome {
    passed: usize,
    total: usize,
    weaning: Weaning,
    report: serde_json::Value,
    table: String,
}

/// Run, print, write. The demo corpus is a regression suite: any miss is an error.
pub async fn run(brain: Arc<Brain>, corpus: &str) -> anyhow::Result<()> {
    let record = run_corpus(Arc::clone(&brain), corpus).await?;
    print!("{}", record.table);
    println!("{}", record.weaning);
    let path = results_path(&brain.data_dir(), corpus, &record.model);
    write_record(&record, &path)?;
    println!("wrote {}", path.display());
    if corpus == "demo" && record.passed != record.total {
        bail!("demo regression: {}/{} turns passed", record.passed, record.total);
    }
    Ok(())
}

/// Run one corpus and assert the interior stayed LLM-free. No printing, no files.
pub async fn run_corpus(brain: Arc<Brain>, corpus: &str) -> anyhow::Result<BenchRecord> {
    let started = Instant::now();
    let data_dir = brain.data_dir();
    let model = if brain.cfg.offline { "offline".to_string() } else { sanitize(&brain.cfg.ears_model) };
    let outcome = match corpus {
        "convo20" => convo(Arc::clone(&brain), &data_dir).await?,
        "ace" => ace(Arc::clone(&brain), &data_dir).await?,
        "babi" => babi(&brain, &data_dir).await?,
        "demo" => demo(&brain, &data_dir).await?,
        other => bail!("unknown corpus '{other}': use one of {}", CORPORA.join(" | ")),
    };
    let interior = brain.metrics().interior_llm_calls + outcome.weaning.interior_llm_calls;
    if interior != 0 {
        bail!("interior_llm_calls = {interior} over the {corpus} run; the hard rule is 0");
    }
    Ok(BenchRecord {
        corpus: corpus.to_string(),
        model,
        at: chrono::Local::now().to_rfc3339(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        passed: outcome.passed,
        total: outcome.total,
        weaning: outcome.weaning,
        report: outcome.report,
        table: outcome.table,
    })
}

// ---------------------------------------------------------------------------
// convo20 and ace: the ears benches with the Brain's ears
// ---------------------------------------------------------------------------

async fn convo(brain: Arc<Brain>, data_dir: &Path) -> anyhow::Result<Outcome> {
    let corpus = data_dir.join("bench/convo20.json");
    let allow_llm = !brain.cfg.offline;
    // The ears bench drives its own runtime for the LLM seat, so it runs on a
    // blocking thread instead of inside this one.
    let report = tokio::task::spawn_blocking(move || {
        brain.with_ears_blocking(|ears, gate| run_convo(ears, gate, &corpus, allow_llm))
    })
    .await
    .context("convo20 bench thread")??;

    let mut weaning = Weaning::default();
    for row in &report.rows {
        weaning.count_path(&row.path);
    }
    weaning.ears_llm_calls = report.llm_calls as u64;
    Ok(Outcome {
        passed: report.hits,
        total: report.total,
        weaning,
        table: report.to_string(),
        report: serde_json::to_value(&report)?,
    })
}

async fn ace(brain: Arc<Brain>, data_dir: &Path) -> anyhow::Result<Outcome> {
    let corpus = data_dir.join("bench/ace_corpus.json");
    let allow_llm = !brain.cfg.offline;
    let (report, llm_calls) = tokio::task::spawn_blocking(move || {
        brain.with_ears_blocking(|ears, gate| {
            let (calls_before, _, _) = ears.stats().snapshot();
            let report = run_ace(ears, gate, &corpus, allow_llm);
            let (calls_after, _, _) = ears.stats().snapshot();
            report.map(|r| (r, calls_after - calls_before))
        })
    })
    .await
    .context("ace bench thread")??;

    let mut weaning = Weaning::default();
    for (path, n) in &report.path_histogram {
        for _ in 0..*n {
            weaning.count_path(path);
        }
    }
    weaning.ears_llm_calls = llm_calls as u64;
    let table = format!("{report}LLM calls: {llm_calls}\n");
    Ok(Outcome {
        passed: report.hits,
        total: report.total,
        weaning,
        table,
        report: serde_json::to_value(&report)?,
    })
}

// ---------------------------------------------------------------------------
// babi: story turns then a question, one fresh session per probe
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Probe {
    family: u32,
    name: String,
    story: Vec<String>,
    question: String,
    answer: serde_json::Value,
}

#[derive(Serialize)]
struct ProbeRow {
    family: u32,
    name: String,
    question: String,
    expected: String,
    got: String,
    pass: bool,
    paths: Vec<String>,
}

#[derive(Serialize, Default)]
struct FamilyTotal {
    name: String,
    passed: usize,
    total: usize,
}

async fn babi(brain: &Brain, data_dir: &Path) -> anyhow::Result<Outcome> {
    let probes: Vec<Probe> = read_json(&data_dir.join("bench/babi_probes.json"))?;
    let mut weaning = Weaning::default();
    let mut rows: Vec<ProbeRow> = Vec::with_capacity(probes.len());
    for (i, probe) in probes.iter().enumerate() {
        // Each bAbI story is an independent world: a fresh in-memory brain per
        // probe, otherwise Mary still carries the football from an earlier story.
        let brain = Brain::open(spoon_mind::brain::BrainConfig { db_path: None, ..brain.cfg.clone() }).await?;
        let session = format!("babi-{i}");
        let mut paths = Vec::with_capacity(probe.story.len() + 1);
        for line in &probe.story {
            let r = brain.turn(&session, line).await?;
            weaning.count_turn(&r);
            paths.push(path_label(&r));
        }
        let r = brain.turn(&session, &probe.question).await?;
        weaning.count_turn(&r);
        paths.push(path_label(&r));
        rows.push(ProbeRow {
            family: probe.family,
            name: probe.name.clone(),
            question: probe.question.clone(),
            expected: expected_text(&probe.answer),
            got: r.text.clone(),
            pass: babi_pass(&probe.answer, &r.text),
            paths,
        });
    }

    let mut families: BTreeMap<u32, FamilyTotal> = BTreeMap::new();
    for row in &rows {
        let f = families.entry(row.family).or_default();
        f.name = row.name.clone();
        f.total += 1;
        f.passed += row.pass as usize;
    }
    let passed = rows.iter().filter(|r| r.pass).count();

    let mut table = String::new();
    table.push_str(&format!(
        "{:<4} {:<4} {:<40} | {:<12} | {:<52} | paths\n",
        "fam", "", "question", "expected", "got"
    ));
    table.push_str(&format!("{}\n", "-".repeat(150)));
    for row in &rows {
        table.push_str(&format!(
            "{:<4} {:<4} {:<40} | {:<12} | {:<52} | {}\n",
            row.family,
            if row.pass { "ok" } else { "MISS" },
            clip(&row.question, 40),
            clip(&row.expected, 12),
            clip(&row.got, 52),
            row.paths.join(",")
        ));
    }
    table.push_str("per family:\n");
    for (fam, t) in &families {
        table.push_str(&format!("  {:>2} {:<26} {}/{}\n", fam, t.name, t.passed, t.total));
    }
    table.push_str(&format!("babi: {}/{} probes passed\n", passed, rows.len()));

    Ok(Outcome {
        passed,
        total: rows.len(),
        weaning,
        report: serde_json::json!({ "probes": rows, "families": families }),
        table,
    })
}

fn expected_text(answer: &serde_json::Value) -> String {
    match answer {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items.iter().map(expected_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// Replies that only say "I don't know" or ask to rephrase never pass, even
/// when they echo the expected word.
const NON_ANSWERS: &[&str] = &["don't know", "didn't quite get", "rephrase", "i can't", "don't have a view"];

/// Pass rule, documented here because the corpus mixes answer shapes:
/// - strings: the reply contains the answer, case-insensitively;
/// - `yes`: an affirmative reply (starts with or contains the word "yes");
///   `bigger` (family 18) counts as `yes` or a plain restatement with "bigger";
/// - `no`: the whole word "no" ("know" does not count);
/// - `maybe`: "maybe", "might" or "either";
/// - numbers: the digits or the number word;
/// - lists: every element must pass.
fn babi_pass(answer: &serde_json::Value, reply: &str) -> bool {
    let reply_l = reply.to_lowercase();
    if NON_ANSWERS.iter().any(|m| reply_l.contains(m)) {
        return false;
    }
    match answer {
        serde_json::Value::Array(items) => items.iter().all(|v| babi_pass(v, reply)),
        serde_json::Value::Number(n) => {
            let Some(n) = n.as_i64() else { return false };
            has_word(&reply_l, &n.to_string()) || number_word(n).is_some_and(|w| has_word(&reply_l, w))
        }
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "yes" => affirmative(&reply_l),
            "bigger" => affirmative(&reply_l) || reply_l.contains("bigger"),
            "no" => reply_l.starts_with("no") || has_word(&reply_l, "no"),
            "maybe" => has_word(&reply_l, "maybe") || reply_l.contains("might") || reply_l.contains("either"),
            other => reply_l.contains(other),
        },
        _ => false,
    }
}

fn affirmative(reply_l: &str) -> bool {
    reply_l.starts_with("yes") || has_word(reply_l, "yes")
}

fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|w| w == word)
}

fn number_word(n: i64) -> Option<&'static str> {
    const WORDS: [&str; 11] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];
    usize::try_from(n).ok().and_then(|i| WORDS.get(i).copied())
}

// ---------------------------------------------------------------------------
// demo: the STATUS demo lines, one session, must all pass offline
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DemoCorpus {
    turns: Vec<DemoTurn>,
}

#[derive(Deserialize)]
struct DemoTurn {
    text: String,
    expect_contains: String,
}

#[derive(Serialize)]
struct DemoRow {
    text: String,
    expect_contains: String,
    got: String,
    pass: bool,
    path: String,
}

async fn demo(brain: &Brain, data_dir: &Path) -> anyhow::Result<Outcome> {
    let corpus: DemoCorpus = read_json(&data_dir.join("bench/demo.json"))?;
    let mut weaning = Weaning::default();
    let mut rows = Vec::with_capacity(corpus.turns.len());
    for turn in &corpus.turns {
        let r = brain.turn("demo", &turn.text).await?;
        weaning.count_turn(&r);
        rows.push(DemoRow {
            text: turn.text.clone(),
            expect_contains: turn.expect_contains.clone(),
            got: r.text.clone(),
            pass: r.text.to_lowercase().contains(&turn.expect_contains.to_lowercase()),
            path: path_label(&r),
        });
    }
    let passed = rows.iter().filter(|r| r.pass).count();

    let mut table = String::new();
    table.push_str(&format!("{:<4} {:<40} | {:<28} | {:<56} | path\n", "", "input", "expect", "got"));
    table.push_str(&format!("{}\n", "-".repeat(150)));
    for row in &rows {
        table.push_str(&format!(
            "{:<4} {:<40} | {:<28} | {:<56} | {}\n",
            if row.pass { "ok" } else { "MISS" },
            clip(&row.text, 40),
            clip(&row.expect_contains, 28),
            clip(&row.got, 56),
            row.path
        ));
    }
    table.push_str(&format!("demo: {}/{} turns passed\n", passed, rows.len()));

    Ok(Outcome {
        passed,
        total: rows.len(),
        weaning,
        report: serde_json::json!({ "turns": rows }),
        table,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn path_label(r: &TurnResult) -> String {
    match r.episode.metrics.ears_path {
        Some(EarsPath::Direct) => "direct",
        Some(EarsPath::Phrasing) => "phrasing",
        Some(EarsPath::Retrieval) => "retrieval",
        Some(EarsPath::Llm) => "llm",
        Some(EarsPath::Failed) => "failed",
        None => "none",
    }
    .to_string()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn sanitize(model: &str) -> String {
    model.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

fn results_path(data_dir: &Path, corpus: &str, model: &str) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M");
    data_dir.join("bench/results").join(format!("{corpus}_{model}_{stamp}.json"))
}

fn write_record(record: &BenchRecord, path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(record)?).with_context(|| format!("writing {}", path.display()))
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{head}...")
    }
}
