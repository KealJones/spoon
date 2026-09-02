//! Counters, metrics and the CAN snapshot exposed by the Brain.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use spoon_core::store::StoreCounts;
use spoon_core::types::*;

use super::Brain;

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

/// Process-lifetime counters. LLM seat counters live on the `LlmClient`.
#[derive(Default)]
pub(super) struct Counters {
    pub turns: AtomicU64,
    pub ears_native: AtomicU64,
    pub ears_llm: AtomicU64,
    pub ears_failed: AtomicU64,
    pub synth_attempted: AtomicU64,
    pub synth_succeeded: AtomicU64,
}

impl Counters {
    pub fn bump(&self, c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

impl Brain {
    pub fn metrics(&self) -> BrainMetrics {
        let (ears, mouth, teacher, _) = self.llm.counters.snapshot();
        let c = &self.counters;
        BrainMetrics {
            turns: c.turns.load(Ordering::Relaxed),
            interior_llm_calls: 0,
            ears_llm_calls: ears as u64,
            mouth_llm_calls: mouth as u64,
            teacher_llm_calls: teacher as u64,
            ears_native_hits: c.ears_native.load(Ordering::Relaxed),
            ears_llm_hits: c.ears_llm.load(Ordering::Relaxed),
            ears_failed: c.ears_failed.load(Ordering::Relaxed),
            synthesis_attempted: c.synth_attempted.load(Ordering::Relaxed),
            synthesis_succeeded: c.synth_succeeded.load(Ordering::Relaxed),
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
                    a.inputs
                        .iter()
                        .map(|i| format!("{}: {}", i.name, i.ty))
                        .collect::<Vec<_>>()
                        .join(", "),
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
