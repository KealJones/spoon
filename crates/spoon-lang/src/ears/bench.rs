//! Benchmark runners: ACE (158 messy-English items) and convo20 (20 turns of
//! conversation). `allow_llm=false` runs fully offline using `hear_offline`;
//! `allow_llm=true` creates a tokio runtime and calls the async `hear`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use spoon_core::types::clause::{Act, Clause, EarsResult, Quant};

use crate::ears::{Ears, Gate};

#[derive(Debug, Serialize)]
pub struct AceMiss {
    pub input: String,
    pub expected: String,
    pub got: String,
}

#[derive(Debug, Serialize)]
pub struct AceReport {
    pub total: usize,
    /// Structural hits (parse both, compare Act+preds+nouns+quants ignoring variable names).
    pub hits: usize,
    /// Exact string match (whitespace-normalized, case-insensitive).
    pub exact: usize,
    pub parsed: usize,
    pub repairs: usize,
    pub path_histogram: HashMap<String, usize>,
    pub category_breakdown: HashMap<String, CategoryStats>,
    pub misses: Vec<AceMiss>,
}

#[derive(Debug, Serialize, Default)]
pub struct CategoryStats {
    pub total: usize,
    pub hits: usize,
    pub parsed: usize,
}

impl std::fmt::Display for AceReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "ACE bench: {}/{} structural hits ({:.1}%)", self.hits, self.total,
            100.0 * self.hits as f64 / self.total.max(1) as f64)?;
        writeln!(f, "Exact:   {}/{} ({:.1}%)", self.exact, self.total,
            100.0 * self.exact as f64 / self.total.max(1) as f64)?;
        writeln!(f, "Parsed:  {}/{} ({:.1}%)", self.parsed, self.total,
            100.0 * self.parsed as f64 / self.total.max(1) as f64)?;
        writeln!(f, "Repairs: {}", self.repairs)?;
        writeln!(f, "Path breakdown:")?;
        let mut paths: Vec<_> = self.path_histogram.iter().collect();
        paths.sort_by(|a, b| b.1.cmp(a.1));
        for (path, count) in &paths {
            writeln!(f, "  {}: {}", path, count)?;
        }
        writeln!(f, "Category breakdown:")?;
        let mut cats: Vec<_> = self.category_breakdown.iter().collect();
        cats.sort_by_key(|(k, _)| k.to_string());
        for (cat, stats) in &cats {
            writeln!(f, "  {}: {}/{}", cat, stats.hits, stats.total)?;
        }
        Ok(())
    }
}

impl AceReport {
    /// Save report as JSON to a file.
    pub fn save_json(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct CorpusItem {
    pub id: u32,
    pub category: String,
    pub input: String,
    pub expected_ace: String,
}

// ---- structural equality ----

/// Canonical representation of a Clause that ignores variable names.
fn clause_canon(c: &Clause) -> String {
    // Act discriminant (ignoring focus variable names in WH questions)
    let act = match &c.act {
        Act::Assert => "assert".to_string(),
        Act::Command => "command".to_string(),
        Act::Rule => "rule".to_string(),
        Act::Question { kind } => {
            let k = format!("{:?}", kind);
            // Drop the focus variable name from WH questions
            let k = k.split_once('{').map(|(prefix, _)| prefix.trim()).unwrap_or(&k);
            format!("question/{}", k.to_lowercase())
        }
    };

    // Predicates: sorted set of (pred, negated, attr)
    let mut preds: Vec<String> = c.conditions.iter().map(|p| {
        let neg = if p.negated { "!" } else { "" };
        let attr = p.attr.as_deref().unwrap_or("");
        let modal = p.modal.as_ref().map(|m| format!("{:?}", m).to_lowercase()).unwrap_or_default();
        if attr.is_empty() && modal.is_empty() {
            format!("{}{}", neg, p.pred)
        } else if attr.is_empty() {
            format!("{}{}[{}]", neg, p.pred, modal)
        } else {
            format!("{}{}/{}", neg, p.pred, attr)
        }
    }).collect();
    preds.sort();

    // Referent nouns + quants: sorted set of (noun, quant_discriminant)
    let mut refs: Vec<String> = c.referents.iter().map(|r| {
        let noun = r.noun.as_deref().unwrap_or("_");
        let quant = match &r.quant {
            Quant::Indef => "a".to_string(),
            Quant::Def => "the".to_string(),
            Quant::Every => "every".to_string(),
            Quant::No => "no".to_string(),
            Quant::AtLeast(n) => format!("atleast{}", n),
            Quant::AtMost(n) => format!("atmost{}", n),
            Quant::Exactly(n) => format!("exactly{}", n),
            Quant::Count(n) => format!("count{}", n),
            Quant::Named(s) => format!("named:{}", s.to_lowercase()),
            Quant::Literal(_) => "literal".to_string(),
            Quant::Wh => "wh".to_string(),
        };
        format!("{}:{}", noun, quant)
    }).collect();
    refs.sort();

    format!("{}|preds=[{}]|refs=[{}]", act, preds.join(","), refs.join(","))
}

/// Returns true if expected and got are structurally equivalent according to the gate.
/// Falls back to false if either fails to parse.
pub fn structural_eq(expected: &str, got: &str, gate: &dyn Gate) -> bool {
    let exp_clauses = match gate.parse(expected) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let got_clauses = match gate.parse(got) {
        Ok(c) => c,
        Err(_) => return false,
    };
    if exp_clauses.len() != got_clauses.len() {
        return false;
    }
    let mut exp_canons: Vec<String> = exp_clauses.iter().map(clause_canon).collect();
    let mut got_canons: Vec<String> = got_clauses.iter().map(clause_canon).collect();
    exp_canons.sort();
    got_canons.sort();
    exp_canons == got_canons
}

/// `direct*` is the last-resort direct parse that carries unknown words.
fn path_label(result: &EarsResult) -> String {
    let path = format!("{:?}", result.path).to_lowercase();
    if result.path == spoon_core::types::clause::EarsPath::Direct && result.confidence < 1.0 {
        format!("{path}*")
    } else {
        path
    }
}

/// Exact match after whitespace/case normalization.
fn exact_eq(expected: &str, got: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    norm(expected) == norm(got)
}

/// One `hear` per item, timed. Offline runs use `hear_offline` (the same
/// pipeline minus the LLM seat); LLM runs block on the async `hear`.
fn hear_each<'a, I>(ears: &Ears, gate: &dyn Gate, inputs: I, allow_llm: bool) -> anyhow::Result<Vec<(EarsResult, u128)>>
where
    I: IntoIterator<Item = &'a str>,
{
    let rt = if allow_llm { Some(tokio::runtime::Runtime::new()?) } else { None };
    Ok(inputs
        .into_iter()
        .map(|input| {
            let start = std::time::Instant::now();
            let result = match &rt {
                Some(rt) => rt.block_on(ears.hear(input, gate)),
                None => ears.hear_offline(input, gate),
            };
            (result, start.elapsed().as_millis())
        })
        .collect())
}

/// Run the ACE benchmark.
///
/// The `gate` should be the production SCE parser; for tests a `FakeGate` is fine.
pub fn run_ace(
    ears: &Ears,
    gate: &dyn Gate,
    corpus: &Path,
    allow_llm: bool,
) -> anyhow::Result<AceReport> {
    let text = std::fs::read_to_string(corpus)
        .map_err(|e| anyhow::anyhow!("cannot read corpus {}: {e}", corpus.display()))?;
    let items: Vec<CorpusItem> = serde_json::from_str(&text)?;

    let mut report = AceReport {
        total: items.len(),
        hits: 0,
        exact: 0,
        parsed: 0,
        repairs: 0,
        path_histogram: HashMap::new(),
        category_breakdown: HashMap::new(),
        misses: vec![],
    };

    let (_, repairs_before, _) = ears.stats().snapshot();
    let results = hear_each(ears, gate, items.iter().map(|i| i.input.as_str()), allow_llm)?;
    let (_, repairs_after, _) = ears.stats().snapshot();
    report.repairs = (repairs_after - repairs_before) as usize;

    for (item, (result, _ms)) in items.iter().zip(results) {
        *report.path_histogram.entry(path_label(&result)).or_default() += 1;

        let cat = report.category_breakdown.entry(item.category.clone()).or_default();
        cat.total += 1;

        if !result.clauses.is_empty() {
            report.parsed += 1;
            cat.parsed += 1;
        }

        let is_exact = exact_eq(&item.expected_ace, &result.sce);
        // Structural match (compare parsed clauses, ignoring variable names)
        let is_structural = is_exact || structural_eq(&item.expected_ace, &result.sce, gate);

        if is_exact { report.exact += 1; }
        if is_structural {
            report.hits += 1;
            cat.hits += 1;
        } else {
            report.misses.push(AceMiss {
                input: item.input.clone(),
                expected: item.expected_ace.clone(),
                got: result.sce.clone(),
            });
        }
    }

    Ok(report)
}

// ---- convo20 ----

#[derive(Deserialize)]
pub struct ConvoItem {
    pub input: String,
    pub sce: String,
}

#[derive(Debug, Serialize)]
pub struct ConvoRow {
    pub input: String,
    pub expected: String,
    pub got: String,
    pub path: String,
    pub ms: u128,
    pub hit: bool,
}

#[derive(Debug, Serialize)]
pub struct ConvoReport {
    pub total: usize,
    pub hits: usize,
    pub exact: usize,
    pub llm_calls: u32,
    pub repairs: u32,
    pub rows: Vec<ConvoRow>,
}

impl std::fmt::Display for ConvoReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{:<3} {:<44} | {:<44} | {:<44} | {:<9} | {:>6}", "", "input", "expected", "got", "path", "ms")?;
        writeln!(f, "{}", "-".repeat(165))?;
        for row in &self.rows {
            writeln!(
                f,
                "{:<3} {:<44} | {:<44} | {:<44} | {:<9} | {:>6}",
                if row.hit { "ok" } else { "MISS" },
                clip(&row.input, 44),
                clip(&row.expected, 44),
                clip(&row.got, 44),
                row.path,
                row.ms
            )?;
        }
        writeln!(
            f,
            "convo: {}/{} structural hits, {} exact, {} llm calls, {} repairs (direct* = last-resort parse with unknown words)",
            self.hits, self.total, self.exact, self.llm_calls, self.repairs
        )
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{head}...")
    }
}

/// Run the conversation benchmark (`data/bench/convo20.json`).
pub fn run_convo(ears: &Ears, gate: &dyn Gate, corpus: &Path, allow_llm: bool) -> anyhow::Result<ConvoReport> {
    let text = std::fs::read_to_string(corpus)
        .map_err(|e| anyhow::anyhow!("cannot read corpus {}: {e}", corpus.display()))?;
    let items: Vec<ConvoItem> = serde_json::from_str(&text)?;

    let (calls_before, repairs_before, _) = ears.stats().snapshot();
    let results = hear_each(ears, gate, items.iter().map(|i| i.input.as_str()), allow_llm)?;
    let (calls_after, repairs_after, _) = ears.stats().snapshot();

    let mut report = ConvoReport {
        total: items.len(),
        hits: 0,
        exact: 0,
        llm_calls: calls_after - calls_before,
        repairs: repairs_after - repairs_before,
        rows: vec![],
    };
    for (item, (result, ms)) in items.iter().zip(results) {
        let exact = exact_eq(&item.sce, &result.sce);
        let hit = exact || structural_eq(&item.sce, &result.sce, gate);
        report.exact += exact as usize;
        report.hits += hit as usize;
        report.rows.push(ConvoRow {
            input: item.input.clone(),
            expected: item.sce.clone(),
            got: result.sce.clone(),
            path: path_label(&result),
            ms,
            hit,
        });
    }
    Ok(report)
}
