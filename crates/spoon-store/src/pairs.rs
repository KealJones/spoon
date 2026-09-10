//! Worked examples: what was said, and the reading it produced.
//!
//! Every reading the model hands back is a data point about how *this* user
//! talks. Thrown away, the ears cost a model call for the same phrasing
//! forever. Kept, the native path can learn the shape and stop paying. That is
//! the whole weaning curve, and this table is where it lives.
//!
//! A pair is keyed on `(utterance, steps)` rather than on the utterance alone.
//! The same sentence can legitimately produce two different readings over the
//! life of a brain, and collapsing them would throw away the disagreement
//! instead of letting the two compete on evidence.
//!
//! # Trust is not uniform
//!
//! Where a pair came from bounds how far it can be trusted before any evidence
//! arrives. A reading a user explicitly confirmed is the strongest thing Spoon
//! has: a human looked at it and said yes. A seeded pair is curated but was
//! written for nobody in particular. A model reading is plausible and nothing
//! more. [`PairSource::prior`] is that ordering, and [`Pair::standing`] is what
//! outcomes do to it afterwards.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use spoon_concept::Concept;

use crate::Store;
use crate::encode::{ts_from_sql, ts_to_sql};
use crate::error::{Result, StoreError};

/// Where a reading came from, which is what bounds how far it is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairSource {
    /// The model produced it and it parsed. Plausible, unverified.
    Model,
    /// A user confirmed it. The strongest evidence a brain can have about how
    /// its own user talks.
    Confirmed,
    /// It shipped in a seed. Curated, but written for nobody in particular, so
    /// it ranks below something this user actually said yes to.
    Seed,
}

impl PairSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PairSource::Model => "model",
            PairSource::Confirmed => "confirmed",
            PairSource::Seed => "seed",
        }
    }

    /// Standing a pair from this source starts at, before any outcome is
    /// recorded.
    ///
    /// These are ceilings as much as starting points: a phrasing learned from a
    /// model guess never gets to be as sure of itself as one a human approved,
    /// no matter how many times it has worked. That asymmetry is deliberate,
    /// because a model reading that keeps "working" may simply never have been
    /// checked.
    pub fn prior(self) -> f64 {
        match self {
            PairSource::Model => 0.80,
            PairSource::Confirmed => 1.00,
            PairSource::Seed => 0.90,
        }
    }

    fn parse(raw: &str) -> Result<PairSource> {
        match raw {
            "model" => Ok(PairSource::Model),
            "confirmed" => Ok(PairSource::Confirmed),
            "seed" => Ok(PairSource::Seed),
            other => Err(StoreError::corrupt(
                "pairs",
                format!("unknown pair source {other:?}"),
            )),
        }
    }
}

/// One utterance and the concept steps it produced, with how that has gone.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub id: i64,
    pub utterance: String,
    pub steps: Vec<Concept>,
    pub source: PairSource,
    pub at: DateTime<Utc>,
    /// Times a reading built from this pair turned out to be right.
    pub successes: u32,
    /// Times it turned out to be wrong. A phrasing that keeps misfiring has to
    /// be able to lose, or one bad generalization poisons the native path
    /// permanently.
    pub failures: u32,
}

impl Pair {
    /// How much to trust this pair now, in `[0, 1]`.
    ///
    /// With no outcomes recorded this is exactly the source prior: a fresh pair
    /// is worth what its provenance is worth and nothing more. As outcomes
    /// accumulate the observed rate takes over, so three failures in a row pull
    /// a model pair well below the threshold the ears will act on, and the
    /// prior stops mattering.
    ///
    /// The blend weight `n / (n + 3)` is what makes early evidence count
    /// without letting a single unlucky turn erase a source's standing.
    pub fn standing(&self) -> f64 {
        standing(self.source.prior(), self.successes, self.failures)
    }
}

/// A pair's standing from its prior and its record.
///
/// Free rather than a method because the ears keep their own copy of a
/// template's standing in memory and have to age it the same way. Two
/// formulas that were meant to agree and drifted would show up as a phrasing
/// the store had demoted and the index still trusted.
pub fn standing(prior: f64, successes: u32, failures: u32) -> f64 {
    let successes = f64::from(successes);
    let failures = f64::from(failures);
    let observations = successes + failures;
    if observations == 0.0 {
        return prior;
    }
    // Laplace smoothing keeps a single failure from meaning "never right".
    let observed = (successes + 1.0) / (observations + 2.0);
    let weight = observations / (observations + 3.0);
    (prior * (1.0 - weight) + observed * weight).clamp(0.0, 1.0)
}

/// Structural key for a reading.
///
/// Content ids are already stable across processes and machines, so hashing
/// them (with the length mixed in, so `[a, b]` cannot collide with `[ab]`)
/// gives a key that means the same thing in every brain that ever stores it.
fn steps_digest(steps: &[Concept]) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(steps.len() as u32).to_le_bytes());
    for step in steps {
        hasher.update(step.content_id().as_bytes());
    }
    hasher.finalize().as_bytes().to_vec()
}

impl Store {
    /// Record that this utterance was read as these steps.
    ///
    /// Storing the same pair again is not new evidence and does not touch the
    /// outcome counters: only [`Store::record_pair_outcome`] moves those. It
    /// does upgrade the source, because a model guess the user later confirms
    /// should stop being ranked as a guess.
    ///
    /// The store records; it does not judge. An empty utterance or an empty
    /// step list is stored as given, and the phrasing index refuses to build a
    /// template from either. Rejecting here would mean two places deciding what
    /// counts as a usable example.
    pub fn put_pair(&self, utterance: &str, steps: &[Concept], source: PairSource) -> Result<i64> {
        let digest = steps_digest(steps);
        let encoded = serde_json::to_string(steps)?;
        let conn = self.conn.lock();
        let existing: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, source FROM pairs WHERE utterance = ?1 AND steps_id = ?2",
                params![utterance, digest],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((id, current)) = existing {
            if source.prior() > PairSource::parse(&current)?.prior() {
                conn.execute(
                    "UPDATE pairs SET source = ?1 WHERE id = ?2",
                    params![source.as_str(), id],
                )?;
            }
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO pairs (utterance, steps_id, steps, source, at, successes, failures)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
            params![
                utterance,
                digest,
                encoded,
                source.as_str(),
                ts_to_sql(Utc::now())
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Pairs worth building templates from, best standing first.
    ///
    /// Ranking happens here rather than in the `ORDER BY` because standing is
    /// the source prior blended with the outcome history, and the source prior
    /// is a policy decision that lives in [`PairSource::prior`]. Restating it
    /// as a SQL `CASE` would put the same judgement in two places, ready to
    /// drift. The table holds one row per distinct reading, so sorting it in
    /// memory is not the expensive part of anything.
    pub fn all_pairs(&self, limit: usize) -> Result<Vec<Pair>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, utterance, steps, source, at, successes, failures FROM pairs")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;

        let mut pairs = Vec::new();
        for row in rows {
            let (id, utterance, encoded, source, at, successes, failures) = row?;
            let steps: Vec<Concept> = serde_json::from_str(&encoded).map_err(|e| {
                StoreError::corrupt("pairs", format!("undecodable steps in pair {id}: {e}"))
            })?;
            pairs.push(Pair {
                id,
                utterance,
                steps,
                source: PairSource::parse(&source)?,
                at: ts_from_sql("pairs", at)?,
                successes: successes.max(0) as u32,
                failures: failures.max(0) as u32,
            });
        }
        // Ties break on id so two brains built the same way rank the same way,
        // which is what makes a failed recognition reproducible.
        pairs.sort_by(|a, b| {
            b.standing()
                .partial_cmp(&a.standing())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        pairs.truncate(limit);
        Ok(pairs)
    }

    /// Forget a learned phrasing.
    ///
    /// For phrasings that can no longer produce a working reading, not ones
    /// that merely read something badly. A pair that keeps losing is left
    /// alone: its failure count is the evidence that demotes it.
    pub fn forget_pair(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock();
        let n = conn.execute("DELETE FROM pairs WHERE id = ?1", [id])?;
        Ok(n > 0)
    }

    /// Record how a reading built from this pair turned out.
    ///
    /// A missing id is an error rather than a no-op: the caller believes it is
    /// crediting a specific pair, and silently crediting nothing would hide the
    /// bug for as long as the brain lives.
    pub fn record_pair_outcome(&self, id: i64, succeeded: bool) -> Result<()> {
        let conn = self.conn.lock();
        let changed = if succeeded {
            conn.execute(
                "UPDATE pairs SET successes = successes + 1 WHERE id = ?1",
                params![id],
            )?
        } else {
            conn.execute(
                "UPDATE pairs SET failures = failures + 1 WHERE id = ?1",
                params![id],
            )?
        };
        if changed == 0 {
            return Err(StoreError::missing("pair", id.to_string()));
        }
        Ok(())
    }

    pub fn count_pairs(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM pairs", [], |row| row.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Remove phrasings whose concepts reference symbols not in the store.
    ///
    /// After a rename, stored phrasings still hold the old SymbolIds. The
    /// template builder would load them, the evaluator would find no
    /// realization under the dead name, and the turn would silently return
    /// garbage. Better to drop them and let the system relearn.
    pub fn purge_stale_pairs(&self) -> Result<PurgeReport> {
        let known: std::collections::HashSet<spoon_concept::SymbolId> =
            self.all_symbols()?.into_iter().map(|(id, _)| id).collect();
        let pairs = self.all_pairs(usize::MAX)?;
        let mut removed = 0u32;
        let mut kept = 0u32;
        let mut examples: Vec<String> = Vec::new();
        for pair in &pairs {
            let stale = pair.steps.iter().any(|step| {
                spoon_concept::pre_order(step)
                    .any(|node| node.as_symbol().is_some_and(|sym| !known.contains(&sym)))
            });
            if stale {
                self.forget_pair(pair.id)?;
                removed += 1;
                if examples.len() < 5 {
                    examples.push(pair.utterance.clone());
                }
            } else {
                kept += 1;
            }
        }
        Ok(PurgeReport {
            removed,
            kept,
            examples,
            realizations_retired: 0,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PurgeReport {
    pub removed: u32,
    pub kept: u32,
    pub examples: Vec<String>,
    pub realizations_retired: u32,
}

impl Store {
    /// Remove learned realizations whose bodies reference dead symbols.
    pub fn purge_stale_realizations(&self) -> Result<u32> {
        let known: std::collections::HashSet<spoon_concept::SymbolId> =
            self.all_symbols()?.into_iter().map(|(id, _)| id).collect();
        let all = self.all_realizations()?;
        let mut retired = 0u32;
        for r in &all {
            if matches!(r.provenance, spoon_concept::Provenance::Bootstrap) {
                continue;
            }
            let body_concepts: Vec<&spoon_concept::Concept> = match &r.spec {
                spoon_concept::RealizationSpec::Composed { body } => {
                    spoon_concept::pre_order(body).collect()
                }
                spoon_concept::RealizationSpec::Rule {
                    pattern, produce, ..
                } => {
                    let mut v: Vec<&spoon_concept::Concept> =
                        spoon_concept::pre_order(pattern).collect();
                    v.extend(spoon_concept::pre_order(produce));
                    v
                }
                _ => continue,
            };
            let stale = body_concepts
                .iter()
                .any(|node| node.as_symbol().is_some_and(|sym| !known.contains(&sym)));
            if stale {
                self.retire_realization(&r.name)?;
                retired += 1;
            }
        }
        Ok(retired)
    }
}
