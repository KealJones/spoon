//! The one orchestrator. A turn is: ears -> discourse -> dispatch ->
//! plan/execute -> ResponsePlan -> mouth. This file stays a short orchestrator;
//! the work lives in the modules it calls.
//!
//! Public surface (stable for the `spoon` binary and server):
//! `BrainConfig`, `Brain::open`, `Brain::turn`, `Brain::metrics`, `Brain::snapshot`.
//!
//! Lock order, when more than one is held: sessions -> can -> store -> gate/ears.

mod exec;
mod host;
mod learn;
mod metrics;
mod respond;
mod session;
mod teach;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::anyhow;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use spoon_core::can::Can;
use spoon_core::kernel::{Kernel, PermissionMode};
use spoon_core::llm::{LlmClient, LlmConfig};
use spoon_core::store::Store;
use spoon_core::types::*;

use spoon_lang::ears::gate::SceGate;
use spoon_lang::ears::{self, Ears, Gate};
use spoon_lang::mouth::{Mouth, MouthPath, RenderContext};

use crate::discourse::{self, detect_clause_correction, extract_keywords, ground_all, Correction, DiscourseState, Grounded};
use crate::dispatch::{self, DispatchCtx, Dispatched};
use crate::teacher::Teacher;

use exec::ResumeResult;
pub use host::BrainHost;
pub use learn::LearnOutcome;
use learn::{apply_synonyms, SYNONYMS_KEY};
use metrics::Counters;
pub use metrics::{ActionSummary, BrainMetrics, Snapshot};
use session::{LastAssert, Pending, Session};

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
    /// Present only when online and the model answered a ping.
    teacher: Option<Teacher>,
    /// Async mutex: the ears LLM seat is awaited while this is held, and the
    /// axum handler needs the turn future to be Send. Held for a whole turn.
    ears: tokio::sync::Mutex<Ears>,
    /// Sync mutex, never held across an await; `turn` clones a snapshot for
    /// the async parse gate.
    gate: Mutex<SceGate>,
    sessions: Mutex<HashMap<String, Session>>,
    /// Word -> canonical word, applied to the raw text before the ears.
    synonyms: Mutex<HashMap<String, String>>,
    counters: Counters,
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
        let ping = |model: &str| {
            let c = LlmConfig::ollama(model);
            let llm = llm.clone();
            async move { llm.ping(&c).await.then_some(c) }
        };
        let (mouth_cfg, ears_cfg, teacher_cfg) = if cfg.offline {
            (None, None, None)
        } else {
            (ping(&cfg.mouth_model).await, ping(&cfg.ears_model).await, ping(&cfg.teacher_model).await)
        };
        let mouth = Mouth { cfg: mouth_cfg, client: llm.clone() };
        let teacher = teacher_cfg.map(|c| Teacher::new(llm.clone(), c));

        let data_dir = cfg.data_dir.clone().unwrap_or_else(resolve_data_dir);
        let mut ears = match Ears::from_data_dir(&data_dir) {
            Ok(e) => e,
            Err(_) => Ears::new(
                spoon_lang::ears::lexicon::Lexicon::new(),
                spoon_lang::ears::phrasings::PhrasingStore::new(),
                None,
            ),
        };
        if let Some(ears_cfg) = ears_cfg {
            let load_lex = || {
                spoon_lang::ears::lexicon::Lexicon::load_seed_dir(&data_dir.join("seed"))
                    .unwrap_or_else(|_| spoon_lang::ears::lexicon::Lexicon::new())
            };
            let mut lex = load_lex();
            lex.extend_from_can(&can);
            let phrasings = ears::load_phrasings(&data_dir, &load_lex())
                .unwrap_or_else(|_| spoon_lang::ears::phrasings::PhrasingStore::new());
            ears = Ears::new(lex, phrasings, Some((llm.clone(), ears_cfg)));
        }

        let pairs = store.pairs(0).unwrap_or_default();
        if !pairs.is_empty() {
            ears.add_pairs(&pairs);
        }

        // Learned synonyms survive restarts via the kv table.
        let synonyms: HashMap<String, String> = store
            .kv_get(SYNONYMS_KEY)?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        for (word, canonical) in &synonyms {
            ears.learn_word(word, canonical);
        }

        let gate = SceGate::from_can(&can);

        // Assert self-model seed facts if this is a fresh store.
        if store.counts().unwrap_or_default().facts == 0 {
            for sce_text in &dispatch::self_model_seed() {
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
            teacher,
            ears: tokio::sync::Mutex::new(ears),
            gate: Mutex::new(gate),
            sessions: Mutex::new(HashMap::new()),
            synonyms: Mutex::new(synonyms),
            counters: Counters::default(),
        }))
    }

    /// Run `f` on the session, creating it on first use.
    fn with_session<R>(&self, session_id: &str, f: impl FnOnce(&mut Session) -> R) -> R {
        let mut sessions = self.sessions.lock();
        f(sessions.entry(session_id.to_string()).or_insert_with(Session::new))
    }

    // -----------------------------------------------------------------------
    // Turn
    // -----------------------------------------------------------------------

    pub async fn turn(&self, session_id: &str, text: &str) -> anyhow::Result<TurnResult> {
        let started = Instant::now();
        let mut ears = self.ears.lock().await;
        self.counters.bump(&self.counters.turns);
        let mut out = SyncTurnResult::new(self.llm.counters.snapshot());

        // 1. Pending state: the user may be answering something we asked.
        // Checked before the ears so a bare "yes" or "cd" never costs an LLM call.
        if let Some(plan) = self.answer_pending(session_id, text, &mut ears, &mut out.flags, &mut out.trace) {
            return self.finalize_turn(session_id, text, out.reply(plan), &started).await;
        }

        // 2. Ears. The one place the ears LLM seat can run; the gate is a
        // snapshot so no sync lock is held across the await.
        let heard = apply_synonyms(text, &self.synonyms.lock());
        let gate = self.gate.lock().clone();
        let ears_result = ears.hear(&heard, &gate).await;

        // 3. Everything else is synchronous Spoon code.
        let mut out = self.turn_sync(session_id, text, ears_result, &mut ears, out)?;

        // The teacher seat: the only other LLM call reachable from the turn
        // loop, and it only ever writes a Spec. Spoon code does the rest.
        if let Some(req) = out.teacher.take() {
            out.flags.teacher_fallback = true;
            match self.consult_teacher(&req).await {
                Ok(outcome) => {
                    self.with_session(session_id, |s| {
                        if matches!(s.pending, Some(Pending::UnknownVerb { .. })) {
                            s.pending = None;
                        }
                    });
                    out.plan = self.run_learned(
                        &outcome,
                        &req.verb,
                        &req.signals,
                        &req.sce,
                        session_id,
                        true,
                        &mut out.flags,
                        &mut out.trace,
                    )?;
                }
                Err(e) => {
                    if self.cfg.debug {
                        out.trace.push(format!("teacher: {e}"));
                    }
                }
            }
        }

        self.finalize_turn(session_id, text, out, &started).await
    }

    async fn consult_teacher(&self, req: &TeacherRequest) -> anyhow::Result<LearnOutcome> {
        let teacher = self.teacher.as_ref().ok_or_else(|| anyhow!("no teacher available"))?;
        let known_types: Vec<String> = self.can.lock().concepts().map(|c| c.id.0.clone()).take(30).collect();
        let arg_types: Vec<String> = req.signals.iter().map(|s| s.ty.to_string()).collect();
        let context = format!("The user said: {} Argument types: {}", req.sce, arg_types.join(", "));
        let spec = teacher.spec_for(&req.verb, &context, &known_types).await?;
        self.learn_from_spec(&req.verb, spec)
    }

    /// Step 1 of a turn: if we asked the user something last turn, try to read
    /// this text as the answer. `Some(plan)` means the turn is fully handled.
    fn answer_pending(
        &self,
        session_id: &str,
        text: &str,
        ears: &mut Ears,
        flags: &mut TurnFlags,
        trace: &mut Vec<String>,
    ) -> Option<ResponsePlan> {
        let pending = self.with_session(session_id, |s| s.pending.take())?;
        match pending {
            Pending::Exec { intent, plan, state, kind, present } => {
                match self.resume_exec(text, &intent, plan, state, &kind, &present, session_id, flags, trace) {
                    ResumeResult::Done(plan) => Some(plan),
                    ResumeResult::NotAnAnswer => {
                        if self.cfg.debug {
                            trace.push("pending dropped, fresh turn".into());
                        }
                        None
                    }
                }
            }
            Pending::UnknownVerb { verb, sce, signals } => {
                self.handle_unknown_verb_followup(session_id, text, &verb, &sce, &signals, ears, flags, trace)
            }
        }
    }

    fn turn_sync(
        &self,
        session_id: &str,
        text: &str,
        ears_result: EarsResult,
        ears: &mut Ears,
        mut out: SyncTurnResult,
    ) -> anyhow::Result<SyncTurnResult> {
        let trace = &mut out.trace;
        let returning_user = self.store.lock().last_episode(session_id).ok().flatten().is_some();

        out.ears_path = ears_result.path.clone();
        out.sce = ears_result.sce.clone();
        match &out.ears_path {
            EarsPath::Direct | EarsPath::Phrasing | EarsPath::Retrieval => self.counters.bump(&self.counters.ears_native),
            EarsPath::Llm => {
                self.counters.bump(&self.counters.ears_llm);
                let pair = Pair {
                    id: 0,
                    utterance: text.to_string(),
                    sce: out.sce.clone(),
                    source: "llm".into(),
                    at: now_ms(),
                    credit: 0,
                };
                let _ = self.store.lock().insert_pair(&pair);
                ears.add_pairs(&[pair]);
            }
            EarsPath::Failed => self.counters.bump(&self.counters.ears_failed),
        }
        if self.cfg.debug {
            trace.push(format!("ears: path={:?} clauses={} sce={}", out.ears_path, ears_result.clauses.len(), out.sce));
        }
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
            return Ok(out.reply(plan));
        }
        out.clauses = ears_result.clauses;

        // 3. Corrections stated in SCE act on memory directly.
        let corrections: Vec<Correction> = out.clauses.iter().filter_map(detect_clause_correction).collect();
        if !corrections.is_empty() {
            let mut plan = ResponsePlan::new(vec![]);
            for c in corrections {
                match c {
                    Correction::Synonym { word, means } => plan.push(self.learn_synonym(&word, &means, ears)?),
                    Correction::Meant { text: name } => {
                        for m in self.correct_referent(session_id, &name)?.moves {
                            plan.push(m);
                        }
                    }
                    _ => {}
                }
            }
            return Ok(out.reply(plan));
        }

        // 4. Discourse grounding.
        let grounded = {
            let mut sessions = self.sessions.lock();
            let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
            let can = self.can.lock();
            ground_all(&mut session.discourse, &out.clauses, &can)
        };
        if self.cfg.debug {
            trace.push(format!("grounded: {} clauses", grounded.len()));
        }

        // 5. Dispatch. The brain holds the store here, so the host is memoryless.
        let facts_before = self.store.lock().counts().map(|c| c.facts as i64).unwrap_or(0);
        let at = now_ms();
        let dispatched = {
            let sessions = self.sessions.lock();
            let mut can = self.can.lock();
            let store = self.store.lock();
            let host = BrainHost::memoryless(self.cfg.permission_mode);
            let session = sessions.get(session_id).expect("session created during grounding");
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
            trace.push(format!("dispatch: {}", dispatch_variant_name(&dispatched)));
        }
        if let Some(target) = assert_target(&grounded) {
            let la = LastAssert { target, facts_before, at, sce: out.sce.clone(), ears_path: out.ears_path.clone() };
            self.with_session(session_id, |s| s.last_assert = Some(la));
        }

        // 6. Act on the dispatch.
        let plan = match dispatched {
            Dispatched::Moves(moves) => ResponsePlan::new(moves),
            Dispatched::Plan { intent, moves_before, present } => {
                self.handle_plan(&intent, moves_before, &present, session_id, &mut out.flags, trace)?
            }
            Dispatched::UnknownCapability { verb, signals, sce, fallback } => match self.learn_from_examples(&verb) {
                Ok(outcome) => self.run_learned(&outcome, &verb, &signals, &sce, session_id, false, &mut out.flags, trace)?,
                Err(short) => {
                    out.flags.synthesis_attempted = short.have >= 2;
                    if self.cfg.debug {
                        trace.push(format!("learn: {}", short.why));
                    }
                    if self.teacher.is_some() && short.have == 0 {
                        out.teacher = Some(TeacherRequest { verb: verb.clone(), sce: sce.clone(), signals: signals.clone() });
                    }
                    self.ask_for_examples(session_id, &verb, &sce, &signals, short.have, fallback)
                }
            },
            Dispatched::NeedsTeacher { fallback, .. } => ResponsePlan::new(fallback),
        };
        Ok(out.reply(plan))
    }

    // -----------------------------------------------------------------------
    // Finalize: mouth + episode + return
    // -----------------------------------------------------------------------

    async fn finalize_turn(
        &self,
        session_id: &str,
        text: &str,
        out: SyncTurnResult,
        started: &Instant,
    ) -> anyhow::Result<TurnResult> {
        let SyncTurnResult { sce, clauses, ears_path, mut plan, mut trace, flags, llm_before, .. } = out;
        plan.finalize();
        if self.cfg.debug {
            trace.push(format!("response: {} moves", plan.moves.len()));
        }

        let t_mouth = Instant::now();
        let (reply, path) = if ears_path == EarsPath::Failed {
            // Nothing was understood, so there is nothing to rephrase: the
            // template says exactly what the plan says and never quotes the
            // user's text back (the LLM prompt would carry it).
            (self.mouth.say_offline(&plan), MouthPath::Template)
        } else {
            let prior_turns = self.with_session(session_id, |s| s.prior_turns.clone());
            let ctx = RenderContext { user_text: text.to_string(), prior_turns };
            self.mouth.say(&plan, &ctx).await
        };
        let ms_mouth = t_mouth.elapsed().as_millis() as u64;

        self.with_session(session_id, |s| {
            s.prior_turns.push((text.to_string(), reply.clone()));
            if s.prior_turns.len() > 10 {
                s.prior_turns.remove(0);
            }
            s.discourse.last_clauses = clauses.clone();
        });

        let (ears_after, mouth_after, teacher_after, _) = self.llm.counters.snapshot();
        let (ears_before, mouth_before, teacher_before, _) = llm_before;
        let keywords = extract_keywords(&clauses, text);
        let metrics = TurnMetrics {
            ears_path: Some(ears_path),
            interior_llm_calls: 0,
            ears_llm_calls: ears_after.saturating_sub(ears_before),
            mouth_llm_calls: mouth_after.saturating_sub(mouth_before),
            teacher_llm_calls: teacher_after.saturating_sub(teacher_before),
            plan_steps: flags.plan_steps,
            synthesis_attempted: flags.synthesis_attempted,
            synthesis_succeeded: flags.synthesis_succeeded,
            teacher_fallback: flags.teacher_fallback,
            // An action synthesized this turn is new, not reused.
            reused_learned_action: flags.used_learned_action && !flags.synthesis_succeeded,
            ms_ears: 0,
            ms_interior: (started.elapsed().as_millis() as u64).saturating_sub(ms_mouth),
            ms_mouth,
        };

        let mut episode = Episode {
            id: 0,
            session_id: session_id.to_string(),
            at: now_ms(),
            user_text: text.to_string(),
            sce,
            clauses,
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// What the synchronous part of a turn produced.
struct SyncTurnResult {
    sce: String,
    clauses: Vec<Clause>,
    ears_path: EarsPath,
    plan: ResponsePlan,
    trace: Vec<String>,
    flags: TurnFlags,
    /// Set when an unknown verb should be taken to the teacher.
    teacher: Option<TeacherRequest>,
    llm_before: (u32, u32, u32, u32),
}

/// What happened this turn, for `TurnMetrics`. Filled by the brain submodules.
#[derive(Default)]
pub(crate) struct TurnFlags {
    pub synthesis_attempted: bool,
    pub synthesis_succeeded: bool,
    pub teacher_fallback: bool,
    /// A plan ran an action with a non-Kernel tier.
    pub used_learned_action: bool,
    pub plan_steps: usize,
}

struct TeacherRequest {
    verb: String,
    sce: String,
    signals: Vec<Signal>,
}

impl SyncTurnResult {
    fn new(llm_before: (u32, u32, u32, u32)) -> Self {
        SyncTurnResult {
            sce: String::new(),
            clauses: vec![],
            ears_path: EarsPath::Direct,
            plan: ResponsePlan::new(vec![]),
            trace: vec![],
            flags: TurnFlags::default(),
            teacher: None,
            llm_before,
        }
    }

    fn reply(mut self, plan: ResponsePlan) -> Self {
        self.plan = plan;
        self
    }
}

/// The entity an assertion was about, so "User means Mary." can retarget it:
/// the subject if it is a proper name, else any proper name in the clause.
fn assert_target(gs: &[Grounded]) -> Option<String> {
    gs.iter().filter(|g| matches!(g.clause.act, Act::Assert)).find_map(|g| {
        let named = |var: &str| {
            g.clause.referents.iter().find(|r| r.var == var).and_then(|r| match &r.quant {
                Quant::Named(n) if n != "User" && n != "Assistant" && n != "It" => Some(n.clone()),
                _ => None,
            })
        };
        let subject = match g.clause.conditions.first().and_then(|p| p.args.first()) {
            Some(Term::Var { var }) => named(var),
            _ => None,
        };
        subject.or_else(|| g.clause.referents.iter().find_map(|r| named(&r.var)))
    })
}

fn dispatch_variant_name(d: &Dispatched) -> &'static str {
    match d {
        Dispatched::Moves(_) => "Moves",
        Dispatched::Plan { .. } => "Plan",
        Dispatched::UnknownCapability { .. } => "UnknownCapability",
        Dispatched::NeedsTeacher { .. } => "NeedsTeacher",
    }
}
