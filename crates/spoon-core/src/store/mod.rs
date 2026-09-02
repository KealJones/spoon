//! SQLite persistence. TO BE IMPLEMENTED by the store subagent.
//!
//! Contract: every public fn below keeps its signature. The Brain owns one
//! `Store`; everything is synchronous rusqlite.

use std::path::Path;

use crate::can::Can;
use crate::types::*;

pub struct Store {
    conn: rusqlite::Connection,
}

/// Git-friendly, shareable snapshot of learned knowledge (not episodes).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Seed {
    pub version: u32,
    pub name: String,
    pub created_at: i64,
    pub concepts: Vec<Concept>,
    pub actions: Vec<Action>,
    pub pairs: Vec<Pair>,
    pub facts: Vec<Fact>,
    pub stances: Vec<Stance>,
    /// Arbitrary key/value config and lexicon extras (slang tables etc).
    #[serde(default)]
    pub kv: Vec<(String, serde_json::Value)>,
}

/// An opinion Spoon holds. Learned in pretrain or from conversation; revisable.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stance {
    pub id: i64,
    pub topic: String,
    pub stance: String,
    pub reasons: Vec<String>,
    pub confidence: f32,
    pub source: String,
    pub at: i64,
}

pub struct EpisodeQuery<'a> {
    pub session_id: Option<&'a str>,
    pub keywords: &'a [String],
    pub since_ms: Option<i64>,
    pub limit: usize,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let _ = path;
        unimplemented!("store subagent")
    }
    pub fn open_memory() -> anyhow::Result<Store> {
        unimplemented!("store subagent")
    }

    // ---- CAN ----
    pub fn load_can(&self) -> anyhow::Result<Can> {
        unimplemented!("store subagent")
    }
    pub fn save_concept(&self, c: &Concept) -> anyhow::Result<()> {
        let _ = c;
        unimplemented!("store subagent")
    }
    pub fn save_action(&self, a: &Action) -> anyhow::Result<()> {
        let _ = a;
        unimplemented!("store subagent")
    }
    pub fn delete_action(&self, id: &ActionId) -> anyhow::Result<()> {
        let _ = id;
        unimplemented!("store subagent")
    }

    // ---- facts ----
    pub fn insert_fact(&self, f: &Fact) -> anyhow::Result<i64> {
        let _ = f;
        unimplemented!("store subagent")
    }
    /// Facts with `pred`, still valid, where each `Some(v)` in pattern equals
    /// the arg at that position.
    pub fn query_facts(&self, pred: &ActionId, pattern: &[Option<Value>]) -> anyhow::Result<Vec<Fact>> {
        let _ = (pred, pattern);
        unimplemented!("store subagent")
    }
    /// All valid facts mentioning `value` in any arg position.
    pub fn facts_about(&self, value: &Value) -> anyhow::Result<Vec<Fact>> {
        let _ = value;
        unimplemented!("store subagent")
    }
    pub fn invalidate_fact(&self, id: i64, at: i64) -> anyhow::Result<()> {
        let _ = (id, at);
        unimplemented!("store subagent")
    }

    // ---- episodes ----
    pub fn insert_episode(&self, e: &Episode) -> anyhow::Result<i64> {
        let _ = e;
        unimplemented!("store subagent")
    }
    pub fn update_episode_credit(&self, id: i64, credit: i8) -> anyhow::Result<()> {
        let _ = (id, credit);
        unimplemented!("store subagent")
    }
    pub fn episodes(&self, q: &EpisodeQuery<'_>) -> anyhow::Result<Vec<Episode>> {
        let _ = q;
        unimplemented!("store subagent")
    }
    pub fn last_episode(&self, session_id: &str) -> anyhow::Result<Option<Episode>> {
        let _ = session_id;
        unimplemented!("store subagent")
    }

    // ---- pairs (ears training data) ----
    pub fn insert_pair(&self, p: &Pair) -> anyhow::Result<i64> {
        let _ = p;
        unimplemented!("store subagent")
    }
    pub fn pairs(&self, min_credit: i8) -> anyhow::Result<Vec<Pair>> {
        let _ = min_credit;
        unimplemented!("store subagent")
    }
    pub fn update_pair_credit(&self, id: i64, credit: i8) -> anyhow::Result<()> {
        let _ = (id, credit);
        unimplemented!("store subagent")
    }

    // ---- stances ----
    pub fn upsert_stance(&self, s: &Stance) -> anyhow::Result<i64> {
        let _ = s;
        unimplemented!("store subagent")
    }
    pub fn stances(&self, topic_keywords: &[String]) -> anyhow::Result<Vec<Stance>> {
        let _ = topic_keywords;
        unimplemented!("store subagent")
    }

    // ---- kv ----
    pub fn kv_get(&self, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let _ = key;
        unimplemented!("store subagent")
    }
    pub fn kv_set(&self, key: &str, value: &serde_json::Value) -> anyhow::Result<()> {
        let _ = (key, value);
        unimplemented!("store subagent")
    }

    // ---- seed ----
    pub fn export_seed(&self, name: &str) -> anyhow::Result<Seed> {
        let _ = name;
        unimplemented!("store subagent")
    }
    /// Merge a seed in. Existing kernel-tier actions are not overwritten.
    pub fn import_seed(&self, seed: &Seed) -> anyhow::Result<usize> {
        let _ = seed;
        unimplemented!("store subagent")
    }

    pub fn counts(&self) -> anyhow::Result<StoreCounts> {
        unimplemented!("store subagent")
    }
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct StoreCounts {
    pub concepts: usize,
    pub actions: usize,
    pub facts: usize,
    pub episodes: usize,
    pub pairs: usize,
    pub stances: usize,
}
