//! Stage 0 kernel: the evaluator for the IR and the hand-written primitives.
//!
//! Contract (owned by the orchestrator; implementations live in `eval.rs` and
//! `prims/`):
//! - `Kernel::new()` registers every primitive and exposes their `Action`
//!   definitions via `Kernel::actions()` so the CAN can be seeded.
//! - `Kernel::call` runs a primitive by id.
//! - `eval::eval_program` runs a `Program` against args, calling back into the
//!   CAN for `Impl::Program` actions and into the kernel for primitives.
//! - Everything is synchronous. Effectful primitives use blocking IO; the
//!   orchestrator runs turns on a blocking thread.

pub mod eval;
pub mod prims;

use std::collections::HashMap;
use std::time::Instant;

use thiserror::Error;

use crate::can::Can;
use crate::types::*;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum EvalError {
    #[error("type error: expected {expected}, got {got} in {at}")]
    Type { expected: String, got: String, at: String },
    #[error("unknown action {0}")]
    UnknownAction(ActionId),
    #[error("arity mismatch for {action}: expected {expected}, got {got}")]
    Arity { action: ActionId, expected: usize, got: usize },
    #[error("budget exceeded: {0}")]
    Budget(String),
    #[error("permission denied: {action} needs {effect:?}")]
    Permission { action: ActionId, effect: Effect },
    #[error("runtime error in {action}: {message}")]
    Runtime { action: ActionId, message: String },
    #[error("{0}")]
    Other(String),
}

impl EvalError {
    pub fn runtime(action: &ActionId, msg: impl Into<String>) -> EvalError {
        EvalError::Runtime { action: action.clone(), message: msg.into() }
    }
    pub fn ty(expected: impl Into<String>, got: &Value, at: impl Into<String>) -> EvalError {
        EvalError::Type { expected: expected.into(), got: got.type_of().to_string(), at: at.into() }
    }
}

/// Execution budget. Synthesis runs candidates with tiny budgets; user turns
/// get generous ones.
#[derive(Debug, Clone)]
pub struct Budget {
    pub max_steps: u64,
    pub max_millis: u64,
    pub steps: u64,
    pub started: Instant,
}

impl Budget {
    pub fn new(max_steps: u64, max_millis: u64) -> Budget {
        Budget { max_steps, max_millis, steps: 0, started: Instant::now() }
    }
    pub fn generous() -> Budget {
        Budget::new(2_000_000, 20_000)
    }
    pub fn tiny() -> Budget {
        Budget::new(2_000, 20)
    }
    pub fn tick(&mut self) -> Result<(), EvalError> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(EvalError::Budget(format!("{} steps", self.max_steps)));
        }
        if self.steps % 256 == 0 && self.started.elapsed().as_millis() as u64 > self.max_millis {
            return Err(EvalError::Budget(format!("{} ms", self.max_millis)));
        }
        Ok(())
    }
}

/// Permission mode for effectful primitives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    AlwaysAsk,
    #[default]
    AskWrites,
    Bypass,
}

impl PermissionMode {
    /// Does this effect need a confirmation prompt under this mode?
    pub fn needs_confirmation(self, effect: Effect) -> bool {
        match self {
            PermissionMode::Bypass => false,
            PermissionMode::AskWrites => matches!(effect, Effect::Write | Effect::Shell),
            PermissionMode::AlwaysAsk => effect != Effect::Pure,
        }
    }
}

/// What the kernel may touch on this machine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Sandbox {
    /// Absolute directory prefixes the fs primitives may read/write.
    pub fs_roots: Vec<String>,
    pub allow_network: bool,
    pub allow_shell: bool,
    pub shell_timeout_secs: u64,
    pub http_timeout_secs: u64,
    pub max_read_bytes: usize,
}

impl Default for Sandbox {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Sandbox {
            fs_roots: vec![home, "/tmp".into()],
            allow_network: true,
            allow_shell: true,
            shell_timeout_secs: 20,
            http_timeout_secs: 20,
            max_read_bytes: 2_000_000,
        }
    }
}

impl Sandbox {
    pub fn path_allowed(&self, p: &str) -> bool {
        let canon = std::path::Path::new(p);
        let abs = if canon.is_absolute() {
            canon.to_path_buf()
        } else {
            std::env::current_dir().map(|d| d.join(canon)).unwrap_or_else(|_| canon.to_path_buf())
        };
        let abs = abs.to_string_lossy().to_string();
        self.fs_roots.iter().any(|r| abs.starts_with(r.as_str()))
    }
}

/// Services the host (the Brain) provides to primitives that need memory or
/// knowledge. Kept as a trait so the kernel can be tested without a store.
pub trait Host: Send + Sync {
    /// Facts matching `pred` where each Some(arg) must equal.
    fn facts(&self, pred: &ActionId, pattern: &[Option<Value>]) -> Vec<Fact> {
        let _ = (pred, pattern);
        vec![]
    }
    /// Recent or relevant episodes as short text summaries.
    fn recall(&self, query: &str, limit: usize) -> Vec<String> {
        let _ = (query, limit);
        vec![]
    }
    fn now_ms(&self) -> i64 {
        now_ms()
    }
    /// Current permission mode; the executor consults it before effects.
    fn permission_mode(&self) -> PermissionMode {
        PermissionMode::default()
    }
}

pub struct NoHost;
impl Host for NoHost {}

pub struct Ctx<'a> {
    pub can: &'a Can,
    pub kernel: &'a Kernel,
    pub host: &'a dyn Host,
    pub budget: Budget,
    /// Effects at or above this level are refused by `Kernel::call` with
    /// `EvalError::Permission`. The executor sets this after prompting.
    pub max_effect: Effect,
}

impl<'a> Ctx<'a> {
    pub fn new(can: &'a Can, kernel: &'a Kernel, host: &'a dyn Host) -> Ctx<'a> {
        Ctx { can, kernel, host, budget: Budget::generous(), max_effect: Effect::Shell }
    }
    pub fn pure(can: &'a Can, kernel: &'a Kernel, host: &'a dyn Host, budget: Budget) -> Ctx<'a> {
        Ctx { can, kernel, host, budget, max_effect: Effect::Pure }
    }
}

pub type PrimFn = fn(&mut Ctx<'_>, &[Value]) -> Result<Value, EvalError>;

pub struct Kernel {
    prims: HashMap<ActionId, PrimFn>,
    defs: Vec<Action>,
    concepts: Vec<Concept>,
    pub sandbox: Sandbox,
}

impl Default for Kernel {
    fn default() -> Self {
        Kernel::new()
    }
}

impl Kernel {
    pub fn new() -> Kernel {
        let mut k = Kernel {
            prims: HashMap::new(),
            defs: Vec::new(),
            concepts: Vec::new(),
            sandbox: Sandbox::default(),
        };
        prims::register_all(&mut k);
        k
    }

    /// Called by `prims::register_all`.
    pub fn register(&mut self, def: Action, f: PrimFn) {
        self.prims.insert(def.id.clone(), f);
        self.defs.push(def);
    }
    pub fn register_concept(&mut self, c: Concept) {
        self.concepts.push(c);
    }

    /// Action definitions for every primitive (to seed the CAN).
    pub fn actions(&self) -> &[Action] {
        &self.defs
    }
    /// Kernel concepts (User, Assistant, Person, Thing, ...).
    pub fn concepts(&self) -> &[Concept] {
        &self.concepts
    }
    pub fn has(&self, id: &ActionId) -> bool {
        self.prims.contains_key(id)
    }
    pub fn call(&self, ctx: &mut Ctx<'_>, id: &ActionId, args: &[Value]) -> Result<Value, EvalError> {
        let f = self.prims.get(id).ok_or_else(|| EvalError::UnknownAction(id.clone()))?;
        if let Some(def) = self.defs.iter().find(|d| &d.id == id) {
            if def.effect > ctx.max_effect {
                return Err(EvalError::Permission { action: id.clone(), effect: def.effect });
            }
        }
        f(ctx, args)
    }
}
