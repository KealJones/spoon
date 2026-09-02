//! The one orchestrator. A turn is: ears -> discourse -> dispatch ->
//! plan/execute -> ResponsePlan -> mouth. This file stays a short orchestrator;
//! the work lives in the modules it calls.
//!
//! Public surface (stable for the `spoon` binary and server):
//! `BrainConfig`, `Brain::open`, `Brain::turn`, `Brain::metrics`, `Brain::snapshot`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use spoon_core::can::Can;
use spoon_core::kernel::{Kernel, PermissionMode};
use spoon_core::llm::{LlmClient, LlmConfig};
use spoon_core::store::{Store, StoreCounts};
use spoon_core::types::*;
use spoon_lang::mouth::{Mouth, MouthPath, RenderContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainConfig {
    /// SQLite file. `None` = in-memory (tests, `--ephemeral`).
    pub db_path: Option<PathBuf>,
    /// No LLM anywhere: ears run native-only, mouth uses templates, teacher off.
    pub offline: bool,
    pub ears_model: String,
    pub mouth_model: String,
    pub teacher_model: String,
    pub permission_mode: PermissionMode,
    /// Emit per-stage trace lines in `TurnResult::trace`.
    pub debug: bool,
}

impl Default for BrainConfig {
    fn default() -> Self {
        BrainConfig {
            db_path: Some(default_db_path()),
            offline: false,
            ears_model: "qwen3.5:4b".into(),
            mouth_model: "qwen3.5:4b".into(),
            teacher_model: "qwen3.5:4b".into(),
            permission_mode: PermissionMode::default(),
            debug: false,
        }
    }
}

pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("SPOON_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".spoon").join("spoon.db")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    pub text: String,
    pub episode: Episode,
    pub mouth_path: String,
    /// Stage-by-stage debug lines (empty unless `BrainConfig::debug`).
    pub trace: Vec<String>,
}

/// Process-lifetime counters for `/debug/metrics` and the REPL `:metrics`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrainMetrics {
    pub turns: u64,
    pub interior_llm_calls: u64,
    pub ears_llm_calls: u64,
    pub mouth_llm_calls: u64,
    pub teacher_llm_calls: u64,
    pub ears_native_hits: u64,
    pub ears_llm_hits: u64,
    pub ears_failed: u64,
    pub synthesis_attempted: u64,
    pub synthesis_succeeded: u64,
    pub store: StoreCounts,
}

/// Inspector view of what Spoon currently knows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub concepts: usize,
    pub actions: usize,
    pub learned_actions: Vec<ActionSummary>,
    pub store: StoreCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSummary {
    pub id: String,
    pub verbs: Vec<String>,
    pub signature: String,
    pub tier: String,
    pub uses: u64,
}

pub struct Brain {
    pub cfg: BrainConfig,
    pub store: Mutex<Store>,
    pub kernel: Kernel,
    pub can: Mutex<Can>,
    pub llm: LlmClient,
    pub mouth: Mouth,
    counters: Counters,
}

#[derive(Default)]
struct Counters {
    turns: AtomicU64,
    ears_native: AtomicU64,
    ears_llm: AtomicU64,
    ears_failed: AtomicU64,
    synth_attempted: AtomicU64,
    synth_succeeded: AtomicU64,
}

impl Brain {
    pub async fn open(cfg: BrainConfig) -> anyhow::Result<Arc<Brain>> {
        let store = match &cfg.db_path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Store::open(p)?
            }
            None => Store::open_memory()?,
        };
        let kernel = Kernel::new();
        let mut can = store.load_can()?;
        for c in kernel.concepts() {
            if can.concept(&c.id).is_none() {
                can.add_concept(c.clone());
            }
        }
        for a in kernel.actions() {
            if can.action(&a.id).is_none() {
                can.add_action(a.clone());
            }
        }

        let llm = LlmClient::new();
        let mouth_cfg = if cfg.offline {
            None
        } else {
            let c = LlmConfig::ollama(&cfg.mouth_model);
            llm.ping(&c).await.then_some(c)
        };
        let mouth = Mouth { cfg: mouth_cfg, client: llm.clone() };

        Ok(Arc::new(Brain {
            cfg,
            store: Mutex::new(store),
            kernel,
            can: Mutex::new(can),
            llm,
            mouth,
            counters: Counters::default(),
        }))
    }

    /// One conversational turn. Never panics on user input; errors become
    /// `Move::Error` in the reply.
    pub async fn turn(&self, session_id: &str, text: &str) -> anyhow::Result<TurnResult> {
        self.counters.turns.fetch_add(1, Ordering::Relaxed);
        let started = std::time::Instant::now();
        let mut trace = Vec::new();

        // Interior stub until ears/plan/discourse are wired: echo as an Explain move.
        let mut plan = ResponsePlan::single(Move::Explain {
            text: format!("interior not wired yet; heard: {text}"),
        });
        plan.finalize();
        if self.cfg.debug {
            trace.push(format!("response: {} moves", plan.moves.len()));
        }

        let ctx = RenderContext { user_text: text.to_string(), prior_turns: vec![] };
        let t_mouth = std::time::Instant::now();
        let (reply, path) = self.mouth.say(&plan, &ctx).await;
        let ms_mouth = t_mouth.elapsed().as_millis() as u64;

        let (ears_llm, mouth_llm, teacher_llm, _) = self.llm.counters.snapshot();
        let metrics = TurnMetrics {
            ears_path: Some(EarsPath::Failed),
            interior_llm_calls: 0,
            ears_llm_calls: ears_llm,
            mouth_llm_calls: mouth_llm,
            teacher_llm_calls: teacher_llm,
            ms_mouth,
            ms_interior: started.elapsed().as_millis() as u64 - ms_mouth,
            ..TurnMetrics::default()
        };
        let mut episode = Episode {
            id: 0,
            session_id: session_id.to_string(),
            at: now_ms(),
            user_text: text.to_string(),
            sce: String::new(),
            clauses: vec![],
            plans: vec![],
            response: plan,
            reply_text: reply.clone(),
            metrics,
            credit: 0,
            keywords: vec![],
        };
        episode.id = self.store.lock().insert_episode(&episode)?;

        Ok(TurnResult {
            text: reply,
            episode,
            mouth_path: match path {
                MouthPath::Llm => "llm",
                MouthPath::LlmRetried => "llm_retried",
                MouthPath::Template => "template",
            }
            .into(),
            trace,
        })
    }

    pub fn metrics(&self) -> BrainMetrics {
        let (ears, mouth, teacher, _) = self.llm.counters.snapshot();
        BrainMetrics {
            turns: self.counters.turns.load(Ordering::Relaxed),
            interior_llm_calls: 0,
            ears_llm_calls: ears as u64,
            mouth_llm_calls: mouth as u64,
            teacher_llm_calls: teacher as u64,
            ears_native_hits: self.counters.ears_native.load(Ordering::Relaxed),
            ears_llm_hits: self.counters.ears_llm.load(Ordering::Relaxed),
            ears_failed: self.counters.ears_failed.load(Ordering::Relaxed),
            synthesis_attempted: self.counters.synth_attempted.load(Ordering::Relaxed),
            synthesis_succeeded: self.counters.synth_succeeded.load(Ordering::Relaxed),
            store: self.store.lock().counts().unwrap_or_default(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let can = self.can.lock();
        let learned_actions = can
            .actions()
            .filter(|a| a.tier != Tier::Kernel)
            .map(|a| ActionSummary {
                id: a.id.0.clone(),
                verbs: a.verbs.clone(),
                signature: format!(
                    "({}) -> {}",
                    a.inputs.iter().map(|i| format!("{}: {}", i.name, i.ty)).collect::<Vec<_>>().join(", "),
                    a.output
                ),
                tier: format!("{:?}", a.tier),
                uses: a.stats.uses,
            })
            .collect();
        Snapshot {
            concepts: can.concepts().count(),
            actions: can.actions().count(),
            learned_actions,
            store: self.store.lock().counts().unwrap_or_default(),
        }
    }
}
