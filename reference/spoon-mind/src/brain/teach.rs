//! What `spoon teach` and `spoon bench` need from the Brain beyond `turn`:
//! the teacher seat, the CAN's verbs, and the stores that a lesson fills
//! (pairs for the ears, stances for opinions). Nothing here calls an LLM;
//! the caller talks to `teacher()` and hands the results back as data.

use std::path::PathBuf;

use anyhow::bail;

use spoon_core::store::Stance;
use spoon_core::types::*;
use spoon_lang::ears::gate::SceGate;
use spoon_lang::ears::{Ears, Gate};

use crate::discourse::{self, ground_all, AssertOutcome};
use crate::teacher::{TaughtStance, Teacher};

use super::learn::apply_synonyms;
use super::{Brain, Session};

impl Brain {
    /// The teacher seat: present when online and the model answered a ping.
    pub fn teacher(&self) -> Option<&Teacher> {
        self.teacher.as_ref()
    }

    /// Root data directory (seed, prompts, bench corpora).
    pub fn data_dir(&self) -> PathBuf {
        self.cfg.data_dir.clone().unwrap_or_else(super::resolve_data_dir)
    }

    /// Every verb the CAN routes, sorted. The curriculum gets this so the
    /// teacher does not propose what Spoon already does.
    pub fn known_verbs(&self) -> Vec<String> {
        let mut verbs: Vec<String> = self.can.lock().verbs().cloned().collect();
        verbs.sort();
        verbs
    }

    /// Concept ids the teacher may use as parameter types in a spec.
    pub fn known_types(&self) -> Vec<String> {
        self.can.lock().concepts().map(|c| c.id.0.clone()).collect()
    }

    /// Does `sce` parse to at least one clause under the current lexicon?
    pub fn parses(&self, sce: &str) -> bool {
        self.gate.lock().parse(sce).map(|c| !c.is_empty()).unwrap_or(false)
    }

    /// Persist (utterance -> SCE) pairs and teach them to the ears, the way
    /// `turn` keeps the pairs the LLM seat produces. Returns how many were
    /// new to the store.
    pub async fn add_pairs(&self, pairs: &[Pair]) -> anyhow::Result<usize> {
        if pairs.is_empty() {
            return Ok(0);
        }
        let stamped: Vec<Pair> = pairs
            .iter()
            .map(|p| Pair { at: if p.at == 0 { now_ms() } else { p.at }, ..p.clone() })
            .collect();
        let fresh = {
            let store = self.store.lock();
            let before = store.counts()?.pairs;
            for p in &stamped {
                store.insert_pair(p)?;
            }
            store.counts()?.pairs - before
        };
        self.ears.lock().await.add_pairs(&stamped);
        Ok(fresh)
    }

    /// Assert a statement straight into memory with `source` ("teacher"),
    /// the way the self-model seed is asserted at open: pristine SCE parses
    /// through the gate, anything else must resolve natively in the ears (no
    /// LLM). Referents ground in `session_id`'s discourse state so `the cat`
    /// resolves across a lesson. Returns how many clauses were stored (facts,
    /// universals or rules); a contradiction with memory stores nothing.
    pub async fn assert_sce(&self, session_id: &str, text: &str, source: &str) -> anyhow::Result<usize> {
        let heard = apply_synonyms(text, &self.synonyms.lock());
        let clauses = {
            let ears = self.ears.lock().await;
            let gate = self.gate.lock().clone();
            match gate.parse(&heard) {
                Ok(c) if !c.is_empty() => c,
                _ => match ears.hear_native(&heard, &gate) {
                    Some(r) if !r.clauses.is_empty() && r.path != EarsPath::Failed => r.clauses,
                    _ => bail!("not understood without the LLM: {text}"),
                },
            }
        };
        if !clauses.iter().all(|c| matches!(c.act, Act::Assert | Act::Rule)) {
            bail!("not a statement: {text}");
        }

        let mut sessions = self.sessions.lock();
        let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
        let mut can = self.can.lock();
        let store = self.store.lock();
        let grounded = ground_all(&mut session.discourse, &clauses, &can);
        let mut fw = discourse::FactWriter { can: &mut can, store: &store };
        let mut stored = 0;
        for g in &grounded {
            if !matches!(discourse::assert_grounded(&mut fw, g, source, None)?, AssertOutcome::Contradiction { .. }) {
                stored += 1;
            }
        }
        session.discourse.last_clauses = clauses;
        Ok(stored)
    }

    /// Persist a taught stance where `store.stances` (opinion questions)
    /// finds it. Counterpoints are not stored.
    pub fn add_stance(&self, taught: &TaughtStance) -> anyhow::Result<Stance> {
        let mut stance = Stance {
            id: 0,
            topic: taught.topic.trim().to_string(),
            stance: taught.stance.trim().to_string(),
            reasons: taught.reasons.clone(),
            confidence: taught.confidence,
            source: "teacher".into(),
            at: now_ms(),
        };
        stance.id = self.store.lock().upsert_stance(&stance)?;
        Ok(stance)
    }

    /// Run `f` with the ears and the parse gate, for the ears benches.
    /// Blocks on the ears lock: call from a blocking thread, never from
    /// async code.
    pub fn with_ears_blocking<R>(&self, f: impl FnOnce(&Ears, &SceGate) -> R) -> R {
        let ears = self.ears.blocking_lock();
        let gate = self.gate.lock().clone();
        f(&ears, &gate)
    }
}
