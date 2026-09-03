//! What the store knows about a concept beyond its identity.
//!
//! A concept's identity is structural (see [`crate::Concept`]). Everything
//! else -- what it is called, how to operationalize it, how well that has been
//! working, where it came from -- lives here.
//!
//! Only *stored* forms live in this module. The runtime `Realization`, which
//! carries native function pointers and needs evaluator context, is defined in
//! `spoon-eval`. A [`RealizationSpec`] is the serializable description that
//! survives a restart; native code is referenced by name and re-bound from the
//! native registry at load.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::concept::Concept;

/// Authority a realization needs in order to run.
///
/// Learning a capability does not grant permission to exercise every side
/// effect it implies. Capability and authority stay separate concerns: the
/// evaluator checks the effect level against the active permission mode before
/// executing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Effect {
    /// No observation, no mutation. Safe to run, cache, and reorder.
    Pure,
    /// Reads outside state (store queries, filesystem reads) but changes
    /// nothing.
    Read,
    /// Mutates local state (store writes, filesystem writes).
    Write,
    /// Touches the network.
    Network,
    /// Spawns a process.
    Shell,
}

impl Effect {
    /// Effects are ordered by how much authority they require. The effect of a
    /// composed realization is the maximum over its parts.
    pub fn rank(self) -> u8 {
        match self {
            Effect::Pure => 0,
            Effect::Read => 1,
            Effect::Write => 2,
            Effect::Network => 3,
            Effect::Shell => 4,
        }
    }

    pub fn join(self, other: Effect) -> Effect {
        if self.rank() >= other.rank() { self } else { other }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Pure => "pure",
            Effect::Read => "read",
            Effect::Write => "write",
            Effect::Network => "network",
            Effect::Shell => "shell",
        }
    }
}

/// Name of a native implementation in the evaluator's registry.
///
/// Function pointers cannot be persisted, so a stored native realization keeps
/// the registry key instead. On load the evaluator re-binds the key to a live
/// function. A key with no registered function is a load-time error, not a
/// silent no-op: a brain that references a native Spoon no longer has is
/// broken, and must say so.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeId(pub Arc<str>);

impl NativeId {
    pub fn new(name: impl AsRef<str>) -> Self {
        NativeId(Arc::from(name.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The persistable description of one way to operationalize a concept.
///
/// A concept may have several of these at once. They compete, and experience
/// decides which one wins in a given context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RealizationSpec {
    /// Rust code, referenced by registry key.
    Native { native: NativeId },

    /// Built from other concepts. The body may contain holes, which bind
    /// positionally to the arguments of the call being realized: `Hole(0)` is
    /// the first argument. `double` is `Add<Hole(0), Hole(0)>`.
    Composed { body: Concept },

    /// A rewrite rule. When `pattern` matches and `condition` holds, the
    /// matched concept is replaced by `produce` under the same bindings.
    ///
    /// This is how inference happens. `Symmetric` is an ordinary concept whose
    /// realization is a rule; there is no separate inference engine.
    Rule {
        pattern: Concept,
        /// Additional concept that must be derivable for the rule to fire.
        /// `None` means the rule fires whenever the pattern matches.
        condition: Option<Concept>,
        produce: Concept,
    },

    /// An LLM call. `prompt` is a concept template rendered into text;
    /// `parse` describes how the response is turned back into concepts.
    Neural { prompt: Concept, parse: Concept },

    /// Something outside the process: HTTP, a subprocess, a database.
    External { spec: Concept },
}

impl RealizationSpec {
    pub fn kind(&self) -> RealizationKind {
        match self {
            RealizationSpec::Native { .. } => RealizationKind::Native,
            RealizationSpec::Composed { .. } => RealizationKind::Composed,
            RealizationSpec::Rule { .. } => RealizationKind::Rule,
            RealizationSpec::Neural { .. } => RealizationKind::Neural,
            RealizationSpec::External { .. } => RealizationKind::External,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RealizationKind {
    Native,
    Composed,
    Rule,
    Neural,
    External,
}

impl RealizationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RealizationKind::Native => "native",
            RealizationKind::Composed => "composed",
            RealizationKind::Rule => "rule",
            RealizationKind::Neural => "neural",
            RealizationKind::External => "external",
        }
    }
}

/// One stored realization, with the evidence that decides whether it gets
/// chosen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Realization {
    /// The concept this realizes.
    pub target: Concept,
    /// Stable identity of this realization, so evidence can be attached to it.
    pub name: Arc<str>,
    pub spec: RealizationSpec,
    pub effect: Effect,
    pub activation: Activation,
    pub provenance: Provenance,
    pub tier: Tier,
}

/// ACT-R base-level activation plus outcome counts.
///
/// `B = ln(sum over accesses of (now - t)^-d)`. Recency and frequency collapse
/// into one number; the decay exponent `d` defaults to 0.5, the standard ACT-R
/// value. Success rate multiplies in separately when selecting among competing
/// realizations, because a frequently used realization that keeps failing
/// should lose to a rarely used one that works.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activation {
    pub uses: u64,
    pub successes: u64,
    pub failures: u64,
    /// Millisecond timestamps of recent accesses, newest last. Bounded: only
    /// the most recent `ACCESS_WINDOW` entries are kept, because the decay term
    /// makes older ones contribute almost nothing.
    pub accesses: Vec<i64>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl Activation {
    /// How many access timestamps to retain. Beyond this the decayed
    /// contribution of an entry is negligible.
    pub const ACCESS_WINDOW: usize = 64;

    /// Standard ACT-R decay exponent.
    pub const DECAY: f64 = 0.5;

    pub fn new(now: DateTime<Utc>) -> Self {
        Activation {
            uses: 0,
            successes: 0,
            failures: 0,
            accesses: Vec::new(),
            created_at: now,
            last_used_at: None,
        }
    }

    /// Record one access. `succeeded` folds into the outcome counts that
    /// weight selection.
    pub fn record(&mut self, at: DateTime<Utc>, succeeded: bool) {
        self.uses += 1;
        if succeeded {
            self.successes += 1;
        } else {
            self.failures += 1;
        }
        self.accesses.push(at.timestamp_millis());
        if self.accesses.len() > Self::ACCESS_WINDOW {
            let excess = self.accesses.len() - Self::ACCESS_WINDOW;
            self.accesses.drain(..excess);
        }
        self.last_used_at = Some(at);
    }

    /// Base-level activation at `now`.
    ///
    /// Returns `None` when there is no history, which callers should treat as
    /// "unknown", not "zero": a brand new realization has no evidence against
    /// it and should still be reachable.
    pub fn base_level(&self, now: DateTime<Utc>) -> Option<f64> {
        if self.accesses.is_empty() {
            return None;
        }
        let now_ms = now.timestamp_millis();
        let mut sum = 0.0f64;
        for &t in &self.accesses {
            // Clamp to 1 ms so a same-millisecond access does not divide by
            // zero and blow the score up to infinity.
            let age_secs = ((now_ms - t).max(1) as f64) / 1000.0;
            sum += age_secs.powf(-Self::DECAY);
        }
        if sum <= 0.0 { None } else { Some(sum.ln()) }
    }

    /// Laplace-smoothed success rate. Smoothing keeps a single early failure
    /// from permanently burying an otherwise good realization, and gives an
    /// unused realization a neutral 0.5 rather than a divide by zero.
    pub fn success_rate(&self) -> f64 {
        (self.successes as f64 + 1.0) / (self.uses as f64 + 2.0)
    }
}

/// Where a stored concept or realization came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provenance {
    /// Shipped with Spoon at bootstrap.
    Bootstrap,
    /// Asserted by the user in conversation.
    User { episode: Option<u64> },
    /// Produced by the Teacher seat.
    Teacher { episode: Option<u64> },
    /// Found by the synthesizer from examples.
    Synthesized { episode: Option<u64> },
    /// Derived by consolidating repeated structure.
    Consolidated,
    /// Inferred by a rule rather than asserted directly.
    Inferred,
    /// Loaded from an imported seed file.
    Imported { seed: Arc<str> },
    /// Read from an external source, with a citation.
    External { source: Arc<str> },
}

/// Maturity of a stored artifact.
///
/// Tier is not a privilege ladder: a consolidated concept is not a different
/// kind of thing from a provisional one, and both are the same kind of thing
/// as a bootstrap concept. It records how much evidence stands behind the
/// artifact, which feeds selection and eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    /// Shipped at bootstrap. Never evicted.
    Kernel,
    /// Newly learned, little evidence yet.
    Provisional,
    /// Earned its place through repeated successful reuse.
    Consolidated,
    /// Superseded or repeatedly harmful. Retained for provenance, never
    /// selected.
    Deprecated,
}

/// Everything the store knows about a named concept.
///
/// Ground concepts get one of these only when something is asserted about
/// them, which is what keeps `Add<42, 1>` from writing rows for 42 and 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConceptMeta {
    /// The concept this describes.
    pub concept: Concept,
    /// How this concept is said in natural language. Drives the ears
    /// vocabulary and the mouth's word choice. Ordered by preference.
    pub surface_forms: Vec<Arc<str>>,
    pub activation: Activation,
    pub provenance: Provenance,
    pub tier: Tier,
    /// Free-form note for humans reading the inspector. Never load-bearing.
    pub note: Option<Arc<str>>,
}

impl ConceptMeta {
    pub fn new(concept: Concept, provenance: Provenance, tier: Tier, now: DateTime<Utc>) -> Self {
        ConceptMeta {
            concept,
            surface_forms: Vec::new(),
            activation: Activation::new(now),
            provenance,
            tier,
            note: None,
        }
    }

    pub fn with_surface_forms<S: AsRef<str>>(mut self, forms: impl IntoIterator<Item = S>) -> Self {
        self.surface_forms = forms.into_iter().map(|s| Arc::from(s.as_ref())).collect();
        self
    }

    pub fn with_note(mut self, note: impl AsRef<str>) -> Self {
        self.note = Some(Arc::from(note.as_ref()));
        self
    }

    /// Preferred surface form, falling back to `None` when the concept has
    /// never been given one.
    pub fn primary_surface(&self) -> Option<&str> {
        self.surface_forms.first().map(|s| s.as_ref())
    }
}
