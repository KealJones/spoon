//! ACE benchmark runner.
//! Scores the ears against the 158-item messy-English corpus.
//! `allow_llm=false` runs fully offline using `hear_native`.
//! `allow_llm=true` creates a tokio runtime and calls the async `hear`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use spoon_core::types::clause::EarsPath;

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
    pub hits: usize,
    pub parsed: usize,
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
        writeln!(f, "ACE bench: {}/{} hits ({:.1}%)", self.hits, self.total,
            100.0 * self.hits as f64 / self.total.max(1) as f64)?;
        writeln!(f, "Parsed: {}/{} ({:.1}%)", self.parsed, self.total,
            100.0 * self.parsed as f64 / self.total.max(1) as f64)?;
        writeln!(f, "Path breakdown:")?;
        let mut paths: Vec<_> = self.path_histogram.iter().collect();
        paths.sort_by(|a, b| b.1.cmp(a.1));
        for (path, count) in &paths {
            writeln!(f, "  {}: {}", path, count)?;
        }
        writeln!(f, "Category breakdown:")?;
        let mut cats: Vec<_> = self.category_breakdown.iter().collect();
        cats.sort_by_key(|(k, _)| k.clone());
        for (cat, stats) in &cats {
            writeln!(f, "  {}: {}/{}", cat, stats.hits, stats.total)?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct CorpusItem {
    id: u32,
    category: String,
    input: String,
    expected_ace: String,
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
        parsed: 0,
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
        let got = result.sce.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let expected = item.expected_ace.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();

        if got == expected {
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
