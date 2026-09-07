//! Native code, and how the evaluator finds it.

use std::collections::HashMap;
use std::sync::Arc;

use spoon_concept::{Concept, Effect, NativeId};

use crate::ctx::Ctx;
use crate::error::EvalResult;

/// A native implementation.
///
/// Eager natives receive arguments already reduced. Lazy ones receive them
/// untouched and reduce what they need through the context handle. Either way
/// the signature is the same, so the registry can hold them together.
pub type NativeFn = fn(&mut dyn Ctx, &[Concept]) -> EvalResult;

/// How many arguments a native accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    Exact(usize),
    AtLeast(usize),
    Between(usize, usize),
    Any,
}

impl Arity {
    pub fn accepts(self, n: usize) -> bool {
        match self {
            Arity::Exact(k) => n == k,
            Arity::AtLeast(k) => n >= k,
            Arity::Between(lo, hi) => n >= lo && n <= hi,
            Arity::Any => true,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Arity::Exact(k) => format!("exactly {k} argument(s)"),
            Arity::AtLeast(k) => format!("at least {k} argument(s)"),
            Arity::Between(lo, hi) => format!("between {lo} and {hi} arguments"),
            Arity::Any => "any number of arguments".to_string(),
        }
    }
}

/// Which arguments the evaluator reduces before applying a realization.
///
/// `If` that evaluates both branches is wrong, and `Quote` exists precisely to
/// stop evaluation, so eagerness cannot be universal. Most natives want it
/// though, which is why it is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArgStrategy {
    /// Reduce every argument first.
    #[default]
    Eager,
    /// Hand them over untouched.
    Lazy,
    /// Reduce only the arguments whose bit is set. `If` sets bit 0 and leaves
    /// its branches alone.
    Selective(u64),
}

impl ArgStrategy {
    pub fn evaluates(self, index: usize) -> bool {
        match self {
            ArgStrategy::Eager => true,
            ArgStrategy::Lazy => false,
            ArgStrategy::Selective(mask) => index < 64 && (mask >> index) & 1 == 1,
        }
    }

    /// Convenience for the common shape: reduce the first `n` arguments.
    pub fn first(n: u32) -> Self {
        ArgStrategy::Selective(if n >= 64 { u64::MAX } else { (1u64 << n) - 1 })
    }
}

/// One registered native: the function plus everything the evaluator needs to
/// call it safely.
#[derive(Clone)]
pub struct NativeEntry {
    pub func: NativeFn,
    pub arity: Arity,
    pub args: ArgStrategy,
    /// The authority this code actually needs.
    ///
    /// A stored realization also declares an effect, and the evaluator takes
    /// the maximum of the two. A realization claiming `Pure` must not be able
    /// to smuggle in a native that opens a socket.
    pub effect: Effect,
    /// Short description, for the inspector and for error messages.
    pub doc: &'static str,
}

impl std::fmt::Debug for NativeEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeEntry")
            .field("arity", &self.arity)
            .field("args", &self.args)
            .field("effect", &self.effect)
            .field("doc", &self.doc)
            .finish()
    }
}

/// Name to implementation.
///
/// Function pointers cannot be persisted, so a stored realization keeps a
/// [`NativeId`] and the registry re-binds it at run time. A key with no
/// registered function is a hard error when it is reached, not a silent skip:
/// a brain referencing a native Spoon no longer ships is broken, and saying so
/// beats quietly looking dumber.
#[derive(Debug, Default, Clone)]
pub struct NativeRegistry {
    entries: HashMap<NativeId, NativeEntry>,
}

impl NativeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a native. Returns the previous entry if the key was taken,
    /// which the caller should generally treat as a bug rather than ignore.
    pub fn register(
        &mut self,
        name: &str,
        func: NativeFn,
        arity: Arity,
        args: ArgStrategy,
        effect: Effect,
        doc: &'static str,
    ) -> Option<NativeEntry> {
        self.entries.insert(
            NativeId::new(name),
            NativeEntry {
                func,
                arity,
                args,
                effect,
                doc,
            },
        )
    }

    /// Register a pure, eager native, which is the overwhelming majority.
    pub fn pure(
        &mut self,
        name: &str,
        func: NativeFn,
        arity: Arity,
        doc: &'static str,
    ) -> Option<NativeEntry> {
        self.register(name, func, arity, ArgStrategy::Eager, Effect::Pure, doc)
    }

    pub fn get(&self, id: &NativeId) -> Option<&NativeEntry> {
        self.entries.get(id)
    }

    pub fn contains(&self, id: &NativeId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every registered key, sorted. The inspector lists these, and startup
    /// validation checks stored realizations against them.
    pub fn names(&self) -> Vec<NativeId> {
        let mut out: Vec<NativeId> = self.entries.keys().cloned().collect();
        out.sort();
        out
    }

    pub fn entries(&self) -> impl Iterator<Item = (&NativeId, &NativeEntry)> {
        self.entries.iter()
    }

    /// Fold another registry in. Used to assemble the bootstrap set from
    /// per-domain modules without any of them knowing about the others.
    pub fn extend_from(&mut self, other: &NativeRegistry) {
        for (id, entry) in other.entries.iter() {
            self.entries.insert(id.clone(), entry.clone());
        }
    }
}

/// Helper for natives that need a specific ground shape and want a clean type
/// error otherwise.
pub fn type_error(native: &str, expected: &str, got: &Concept) -> crate::error::EvalError {
    crate::error::EvalError::Type {
        native: Arc::from(native),
        expected: expected.to_string(),
        got: got.clone(),
    }
}

/// Helper for natives that fail for their own reasons.
pub fn native_error(native: &str, message: impl Into<String>) -> crate::error::EvalError {
    crate::error::EvalError::Native {
        native: Arc::from(native),
        message: message.into(),
    }
}
