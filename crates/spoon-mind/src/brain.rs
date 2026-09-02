//! The one orchestrator. A turn is: ears -> discourse -> dispatch ->
//! plan/execute -> ResponsePlan -> mouth. This file stays a short orchestrator;
//! the work lives in the modules it calls.
//!
//! Public surface (stable for the `spoon` binary and server):
//! `BrainConfig`, `Brain::open`, `Brain::turn`, `Brain::metrics`, `Brain::snapshot`.

mod exec;
mod host;
mod learn;
mod respond;
mod session;

use std::collections::HashMap;
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

use spoon_lang::ears::{self, Ears, Gate};
use spoon_lang::ears::gate::SceGate;
use spoon_lang::mouth::{Mouth, MouthPath, RenderContext};

use crate::discourse::{self, DiscourseState, ground_all, extract_keywords};
use crate::dispatch::{self, DispatchCtx, Dispatched};

use exec::ResumeResult;
use host::BrainHost;
use learn::learn_from_examples;
use session::{Pending, Session};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

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
    /// Root data directory (seed, prompts). Resolved at open time.
    pub data_dir: Option<PathBuf>,
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
            data_dir: None,
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

/// Walk up from cwd looking for `data/seed/lexicon.json`, else `./data`.
fn resolve_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SPOON_DATA") {
        return PathBuf::from(p);
    }
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for _ in 0..6 {
        if dir.join("data/seed/lexicon.json").exists() {
            return dir.join("data");
        }
        if !dir.pop() {
            break;
        }
    }
    PathBuf::from("data")
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    pub text: String,
    pub episode: Episode,
    pub mouth_path: String,
    pub trace: Vec<String>,
}

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

// ---------------------------------------------------------------------------
// Brain
// ---------------------------------------------------------------------------

pub struct Brain {
    pub cfg: BrainConfig,
    pub store: Mutex<Store>,
    pub kernel: Kernel,
    pub can: Mutex<Can>,
    pub llm: LlmClient,
    pub mouth: Mouth,
    ears: Mutex<Ears>,
    gate: Mutex<SceGate>,
    sessions: Mutex<HashMap<String, Session>>,
    counters: Counters,
    data_dir: PathBuf,
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

        let data_dir = cfg.data_dir.clone().unwrap_or_else(resolve_data_dir);

        // Build ears
        let ears_llm = if cfg.offline {
            None
        } else {
            let ec = LlmConfig::ollama(&cfg.ears_model);
            llm.ping(&ec).await.then(|| (llm.clone(), ec))
        };

        let mut ears = match Ears::from_data_dir(&data_dir) {
            Ok(e) => e,
            Err(_) => {
                let lex = spoon_lang::ears::lexicon::Lexicon::new();
                let ps = spoon_lang::ears::phrasings::PhrasingStore::new();
                Ears::new(lex, ps, None)
            }
        };
        if let Some(llm_pair) = ears_llm {
            ears = Ears::new(
                {
                    let mut lex = match spoon_lang::ears::lexicon::Lexicon::load_seed_dir(
                        &data_dir.join("seed"),
                    ) {
                        Ok(l) => l,
                        Err(_) => spoon_lang::ears::lexicon::Lexicon::new(),
                    };
                    lex.extend_from_can(&can);
                    lex
                },
                {
                    let lex_for_phr = match spoon_lang::ears::lexicon::Lexicon::load_seed_dir(
                        &data_dir.join("seed"),
                    ) {
                        Ok(l) => l,
                        Err(_) => spoon_lang::ears::lexicon::Lexicon::new(),
                    };
                    ears::load_phrasings(&data_dir, &lex_for_phr).unwrap_or_else(|_| {
                        spoon_lang::ears::phrasings::PhrasingStore::new()
                    })
                },
                Some(llm_pair),
            );
        }

        // Load stored pairs into ears
        let pairs = store.pairs(0).unwrap_or_default();
        if !pairs.is_empty() {
            ears.add_pairs(&pairs);
        }

        // Build gate from CAN
        let gate = SceGate::from_can(&can);

        // Assert self-model seed facts if this is a fresh store
        let counts = store.counts().unwrap_or_default();
        if counts.facts == 0 {
            let seed_sentences = dispatch::self_model_seed();
            for sce_text in &seed_sentences {
                if let Ok(clauses) = gate.parse(sce_text) {
                    let mut disc = DiscourseState::default();
                    let gs = ground_all(&mut disc, &clauses, &can);
                    let mut fw = discourse::FactWriter { can: &mut can, store: &store };
                    for g in &gs {
                        let _ = discourse::assert_grounded(&mut fw, g, "seed", None);
                    }
                }
            }
        }

        Ok(Arc::new(Brain {
            cfg,
            store: Mutex::new(store),
            kernel,
            can: Mutex::new(can),
            llm,
            mouth,
            ears: Mutex::new(ears),
            gate: Mutex::new(gate),
            sessions: Mutex::new(HashMap::new()),
            counters: Counters::default(),
            data_dir,
        }))
    }

    // -----------------------------------------------------------------------
    // Turn
    // -----------------------------------------------------------------------

    pub async fn turn(
        &self,
        session_id: &str,
        text: &str,
    ) -> anyhow::Result<TurnResult> {
        let started = std::time::Instant::now();
        let sync_result = self.turn_sync(session_id, text)?;
        self.finalize_turn(
            session_id, text, &sync_result.sce, &sync_result.clauses,
            sync_result.ears_path, sync_result.plan, sync_result.prior_turns,
            &started, sync_result.trace,
            sync_result.ears_before, sync_result.mouth_before, sync_result.teacher_before,
        ).await
    }

    fn turn_sync(
        &self,
        session_id: &str,
        text: &str,
    ) -> anyhow::Result<SyncTurnResult> {
        self.counters.turns.fetch_add(1, Ordering::Relaxed);
        let mut trace: Vec<String> = Vec::new();
        let (ears_before, mouth_before, teacher_before, _) = self.llm.counters.snapshot();

        let returning_user = {
            let store = self.store.lock();
            store
                .last_episode(session_id)
                .ok()
                .flatten()
                .is_some()
        };

        // 2. Handle pending state
        {
            let mut sessions = self.sessions.lock();
            let session = sessions
                .entry(session_id.to_string())
                .or_insert_with(Session::new);

            if let Some(pending) = session.pending.take() {
                match pending {
                    Pending::Exec { intent, plan, mut state, kind } => {
                        let resume_result = self.resume_exec(
                            text, &intent, &plan, &mut state, &kind, session, &mut trace,
                        );
                        match resume_result {
                            ResumeResult::Done(plan) => {
                                let prior = session.prior_turns.clone();
                                return Ok(SyncTurnResult {
                                    sce: String::new(), clauses: vec![], ears_path: EarsPath::Direct,
                                    plan, prior_turns: prior, trace, ears_before, mouth_before, teacher_before,
                                });
                            }
                            ResumeResult::StillPending(plan, new_pending) => {
                                session.pending = Some(new_pending);
                                let prior = session.prior_turns.clone();
                                return Ok(SyncTurnResult {
                                    sce: String::new(), clauses: vec![], ears_path: EarsPath::Direct,
                                    plan, prior_turns: prior, trace, ears_before, mouth_before, teacher_before,
                                });
                            }
                            ResumeResult::NotAnAnswer => {
                                if self.cfg.debug {
                                    trace.push("pending dropped, fresh turn".into());
                                }
                            }
                        }
                    }
                    Pending::UnknownVerb { verb, sce, signals } => {
                        let unknown_verb_result = self.handle_unknown_verb_followup(
                            text, &verb, &sce, &signals, session, &mut trace,
                        );
                        if let Some(plan) = unknown_verb_result {
                            let prior = session.prior_turns.clone();
                            return Ok(SyncTurnResult {
                                sce: String::new(), clauses: vec![], ears_path: EarsPath::Direct,
                                plan, prior_turns: prior, trace, ears_before, mouth_before, teacher_before,
                            });
                        }
                    }
                }
            }
        }

        // 3. Ears
        let ears_result = {
            let ears = self.ears.lock();
            let gate = self.gate.lock();
            ears.hear_native(text, &*gate)
        };

        let ears_result = match ears_result {
            Some(r) => r,
            None => {
                let ears = self.ears.lock();
                let gate = self.gate.lock();
                ears.hear_native(text, &*gate)
                    .unwrap_or_else(|| ears.failed_result(text))
            }
        };

        let ears_path = ears_result.path.clone();
        let sce_text = ears_result.sce.clone();

        match &ears_path {
            EarsPath::Direct | EarsPath::Phrasing | EarsPath::Retrieval => {
                self.counters.ears_native.fetch_add(1, Ordering::Relaxed);
            }
            EarsPath::Llm => {
                self.counters.ears_llm.fetch_add(1, Ordering::Relaxed);
                let pair = Pair {
                    id: 0,
                    utterance: text.to_string(),
                    sce: sce_text.clone(),
                    source: "llm".into(),
                    at: now_ms(),
                    credit: 0,
                };
                let _ = self.store.lock().insert_pair(&pair);
                self.ears.lock().add_pairs(&[pair]);
            }
            EarsPath::Failed => {
                self.counters.ears_failed.fetch_add(1, Ordering::Relaxed);
            }
        }

        if self.cfg.debug {
            trace.push(format!(
                "ears: path={:?} clauses={} sce={}",
                ears_path, ears_result.clauses.len(), &sce_text
            ));
        }

        // 3b. Failed ears
        if ears_result.clauses.is_empty() {
            let unknown = if ears_result.unknown_words.is_empty() {
                String::new()
            } else {
                format!(" (unknown: {})", ears_result.unknown_words.join(", "))
            };
            let plan = ResponsePlan::single(Move::Clarify {
                question: format!("i didn't quite get that{unknown}. could you rephrase?"),
                options: vec![],
                slot_type: None,
            });
            let prior = {
                let sessions = self.sessions.lock();
                sessions.get(session_id).map(|s| s.prior_turns.clone()).unwrap_or_default()
            };
            return Ok(SyncTurnResult {
                sce: sce_text, clauses: vec![], ears_path: EarsPath::Failed,
                plan, prior_turns: prior, trace, ears_before, mouth_before, teacher_before,
            });
        }

        // 4. Discourse grounding
        let clauses = ears_result.clauses;
        let grounded = {
            let can = self.can.lock();
            let mut sessions = self.sessions.lock();
            let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
            ground_all(&mut session.discourse, &clauses, &can)
        };

        if self.cfg.debug {
            trace.push(format!("grounded: {} clauses", grounded.len()));
        }

        // 5. Dispatch
        let dispatch_result = {
            let mut can = self.can.lock();
            let store = self.store.lock();
            let host = BrainHost {
                permission_mode: self.cfg.permission_mode,
            };
            let sessions_guard = self.sessions.lock();
            let session = sessions_guard.get(session_id).unwrap();
            let mut dctx = DispatchCtx {
                can: &mut can,
                store: &store,
                kernel: &self.kernel,
                host: &host,
                session_id,
                episode_id: None,
                returning_user,
            };
            dispatch::dispatch_turn(&mut dctx, &grounded, &session.discourse)?
        };

        if self.cfg.debug {
            trace.push(format!("dispatch: {:?}", dispatch_variant_name(&dispatch_result)));
        }

        // 6. Handle dispatch result
        let plan = match dispatch_result {
            Dispatched::Moves(moves) => ResponsePlan::new(moves),

            Dispatched::Plan { intent, moves_before } => {
                self.handle_plan(
                    &intent, moves_before, session_id, &mut trace,
                )?
            }

            Dispatched::UnknownCapability { verb, signals, sce, fallback } => {
                let learn_result = {
                    let can = self.can.lock();
                    let store = self.store.lock();
                    learn_from_examples(&verb, &can, &store, &self.kernel)
                };

                match learn_result {
                    Ok((action, desc)) => {
                        self.register_learned_action(&action)?;
                        self.counters.synth_attempted.fetch_add(1, Ordering::Relaxed);
                        self.counters.synth_succeeded.fetch_add(1, Ordering::Relaxed);

                        let intent = Intent {
                            goal: Goal::Action { action: action.id.clone() },
                            signals,
                            routes: vec![action.id.clone()],
                            sce: sce.clone(),
                        };
                        let mut plan = self.handle_plan(
                            &intent, vec![], session_id, &mut trace,
                        )?;
                        plan.push(Move::Learned {
                            what: format!("learned '{}': {}", verb, desc),
                        });
                        plan
                    }
                    Err(_) => {
                        let mut sessions = self.sessions.lock();
                        let session = sessions
                            .entry(session_id.to_string())
                            .or_insert_with(Session::new);
                        session.pending = Some(Pending::UnknownVerb {
                            verb: verb.clone(),
                            sce,
                            signals,
                        });
                        // Add verb as a noun so "the V of X is Y" parses
                        self.gate.lock().lex.add_noun(&verb);
                        ResponsePlan::new(fallback)
                    }
                }
            }

            Dispatched::NeedsTeacher { fallback, .. } => {
                ResponsePlan::new(fallback)
            }
        };

        let prior = {
            let sessions = self.sessions.lock();
            sessions.get(session_id).map(|s| s.prior_turns.clone()).unwrap_or_default()
        };
        Ok(SyncTurnResult {
            sce: sce_text, clauses, ears_path, plan, prior_turns: prior,
            trace, ears_before, mouth_before, teacher_before,
        })
    }


    // -----------------------------------------------------------------------
    // Finalize: mouth + episode + return
    // -----------------------------------------------------------------------

    async fn finalize_turn(
        &self,
        session_id: &str,
        text: &str,
        sce: &str,
        clauses: &[Clause],
        ears_path: EarsPath,
        mut plan: ResponsePlan,
        prior_turns: Vec<(String, String)>,
        started: &std::time::Instant,
        mut trace: Vec<String>,
        ears_before: u32,
        mouth_before: u32,
        teacher_before: u32,
    ) -> anyhow::Result<TurnResult> {
        plan.finalize();

        if self.cfg.debug {
            trace.push(format!("response: {} moves", plan.moves.len()));
        }

        let ctx = RenderContext {
            user_text: text.to_string(),
            prior_turns,
        };
        let t_mouth = std::time::Instant::now();
        let (reply, path) = self.mouth.say(&plan, &ctx).await;
        let ms_mouth = t_mouth.elapsed().as_millis() as u64;

        // Update session after the await
        {
            let mut sessions = self.sessions.lock();
            let session = sessions
                .entry(session_id.to_string())
                .or_insert_with(Session::new);
            session.prior_turns.push((text.to_string(), reply.clone()));
            if session.prior_turns.len() > 10 {
                session.prior_turns.remove(0);
            }
            session.discourse.last_clauses = clauses.to_vec();
        }

        let (ears_after, mouth_after, teacher_after, _) = self.llm.counters.snapshot();
        let keywords = extract_keywords(clauses, text);

        let metrics = TurnMetrics {
            ears_path: Some(ears_path),
            interior_llm_calls: 0,
            ears_llm_calls: ears_after.saturating_sub(ears_before),
            mouth_llm_calls: mouth_after.saturating_sub(mouth_before),
            teacher_llm_calls: teacher_after.saturating_sub(teacher_before),
            plan_steps: 0,
            synthesis_attempted: false,
            synthesis_succeeded: false,
            teacher_fallback: false,
            reused_learned_action: false,
            ms_ears: 0,
            ms_interior: started.elapsed().as_millis() as u64 - ms_mouth,
            ms_mouth,
        };

        let mut episode = Episode {
            id: 0,
            session_id: session_id.to_string(),
            at: now_ms(),
            user_text: text.to_string(),
            sce: sce.to_string(),
            clauses: clauses.to_vec(),
            plans: vec![],
            response: plan,
            reply_text: reply.clone(),
            metrics,
            credit: 0,
            keywords,
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

    // -----------------------------------------------------------------------
    // Metrics / Snapshot
    // -----------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct SyncTurnResult {
    sce: String,
    clauses: Vec<Clause>,
    ears_path: EarsPath,
    plan: ResponsePlan,
    prior_turns: Vec<(String, String)>,
    trace: Vec<String>,
    ears_before: u32,
    mouth_before: u32,
    teacher_before: u32,
}

fn dispatch_variant_name(d: &Dispatched) -> &'static str {
    match d {
        Dispatched::Moves(_) => "Moves",
        Dispatched::Plan { .. } => "Plan",
        Dispatched::UnknownCapability { .. } => "UnknownCapability",
        Dispatched::NeedsTeacher { .. } => "NeedsTeacher",
    }
}
