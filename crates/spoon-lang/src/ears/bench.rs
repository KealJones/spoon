//! ACE benchmark runner.
//! Scores the ears against the 158-item messy-English corpus.
//! `allow_llm=false` runs fully offline using `hear_native`.
//! `allow_llm=true` creates a tokio runtime and calls the async `hear`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use spoon_core::types::clause::{Act, Clause, EarsPath, Quant};

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

/// Run the ACE benchmark.
///
/// When `allow_llm=false`, `hear_native` is used (sync, no LLM required).
/// When `allow_llm=true`, a tokio `Runtime` is created to call the async `hear`.
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

    let rt = if allow_llm {
        Some(tokio::runtime::Runtime::new()?)
    } else {
        None
    };

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

    for item in &items {
        let result = if let Some(rt) = &rt {
            rt.block_on(ears.hear(&item.input, gate))
        } else {
            ears.hear_native(&item.input, gate).unwrap_or_else(|| {
                spoon_core::types::clause::EarsResult {
                    clauses: vec![],
                    path: EarsPath::Failed,
                    sce: item.input.clone(),
                    confidence: 0.0,
                    unknown_words: vec![],
                }
            })
        };

        let path_name = format!("{:?}", result.path).to_lowercase();
        *report.path_histogram.entry(path_name).or_default() += 1;

        let cat = report.category_breakdown.entry(item.category.clone()).or_default();
        cat.total += 1;

        let parsed = !result.clauses.is_empty();
        if parsed {
            report.parsed += 1;
            cat.parsed += 1;
        }

        // Exact match after whitespace/case normalization
        let got_norm = result.sce.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let exp_norm = item.expected_ace.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let is_exact = got_norm == exp_norm;

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
        let _ = item.id;
    }

    Ok(report)
}
