//! The concept: the one representation everything in Spoon is made of.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::id::{ConceptId, Ground, HoleId, SymbolId};

/// Everything in Spoon is a Concept. Concepts come in three shapes.
///
/// - **Atomic**: indivisible. Has identity, nothing to decompose. `Greg`,
///   `Add`, `42`.
/// - **Compound**: one concept applied to others. `FriendWith<Greg, Keal>`.
/// - **Hole**: the one shape that is not itself a concept. A gap inside a rule
///   pattern or a partially resolved expression.
///
/// Atomic and compound are equal citizens. Both can be stored, carry
/// realizations, participate in relationships, and accumulate activation
/// stats. `Employment<Greg, Workiva>` takes `Role<..., SoftwareEngineer>` the
/// same way `Greg` does. There is no privileged primitive layer underneath.
///
/// # Why `Arc`
///
/// Evaluation is term rewriting: subterms are copied constantly as rules fire
/// and realizations expand. Owning the children outright would make every
/// rewrite step deep-copy the tree. `Arc` makes `clone` O(1) and lets shared
/// subterms stay shared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Concept {
    Atomic(ConceptId),
    Compound {
        head: Arc<Concept>,
        args: Arc<[Concept]>,
    },
    Hole(HoleId),
}

/// Stable structural digest of a concept.
///
/// Used as the store's exact-match index key. Unlike [`std::hash::Hash`],
/// which is randomized per process, this is a blake3 digest that is identical
/// across restarts, machines, and rebuilds. Two concepts with the same
/// `ContentId` are structurally the same concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContentId(pub [u8; 32]);

impl ContentId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, for logs and inspector URLs.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for byte in self.0.iter() {
            s.push_str(&format!("{byte:02x}"));
        }
        s
    }

    /// Short prefix for human-facing output where the full digest is noise.
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            out[i] = ((hi << 4) | lo) as u8;
        }
        Some(ContentId(out))
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Concept {
    // ---- constructors ----

    /// A named atomic concept: `Greg`, `Add`, `FriendWith`.
    pub fn named(name: &str) -> Self {
        Concept::Atomic(ConceptId::named(name))
    }

    /// An atomic concept identified by a ground value.
    pub fn ground(value: Ground) -> Self {
        Concept::Atomic(ConceptId::Ground(value))
    }

    pub fn symbol(id: SymbolId) -> Self {
        Concept::Atomic(ConceptId::Named(id))
    }

    pub fn int(v: i64) -> Self {
        Concept::ground(Ground::Int(v))
    }

    pub fn float(v: f64) -> Self {
        Concept::ground(Ground::Float(v))
    }

    pub fn bool(v: bool) -> Self {
        Concept::ground(Ground::Bool(v))
    }

    pub fn text(v: impl AsRef<str>) -> Self {
        Concept::ground(Ground::text(v))
    }

    pub fn bytes(v: impl AsRef<[u8]>) -> Self {
        Concept::ground(Ground::bytes(v))
    }

    pub fn json(v: serde_json::Value) -> Self {
        Concept::ground(Ground::json(v))
    }

    pub fn datetime(v: chrono::DateTime<chrono::Utc>) -> Self {
        Concept::ground(Ground::DateTime(v))
    }

    pub fn hole(index: u32) -> Self {
        Concept::Hole(HoleId(index))
    }

    /// Apply a head concept to arguments. A zero-arg application is a bare
    /// mention or assertion of the head in context, which is distinct from the
    /// head standing alone.
    pub fn apply(head: Concept, args: impl Into<Arc<[Concept]>>) -> Self {
        Concept::Compound {
            head: Arc::new(head),
            args: args.into(),
        }
    }

    /// Convenience for the common case: a named head applied to arguments.
    /// `Concept::call("friend-with", [Concept::named("greg"), Concept::named("keal")])`
    pub fn call(head: &str, args: impl IntoIterator<Item = Concept>) -> Self {
        let args: Vec<Concept> = args.into_iter().collect();
        Concept::Compound {
            head: Arc::new(Concept::named(head)),
            args: Arc::from(args),
        }
    }

    // ---- shape queries ----

    pub fn is_atomic(&self) -> bool {
        matches!(self, Concept::Atomic(_))
    }

    pub fn is_compound(&self) -> bool {
        matches!(self, Concept::Compound { .. })
    }

    pub fn is_hole(&self) -> bool {
        matches!(self, Concept::Hole(_))
    }

    /// True when this concept's identity is a ground value.
    pub fn is_ground(&self) -> bool {
        matches!(self, Concept::Atomic(ConceptId::Ground(_)))
    }

    /// True when this concept is an atomic concept identified by a name.
    pub fn is_named(&self) -> bool {
        matches!(self, Concept::Atomic(ConceptId::Named(_)))
    }

    // ---- accessors ----

    pub fn as_atomic(&self) -> Option<&ConceptId> {
        match self {
            Concept::Atomic(id) => Some(id),
            _ => None,
        }
    }

    pub fn as_ground(&self) -> Option<&Ground> {
        match self {
            Concept::Atomic(ConceptId::Ground(g)) => Some(g),
            _ => None,
        }
    }

    pub fn as_symbol(&self) -> Option<SymbolId> {
        match self {
            Concept::Atomic(ConceptId::Named(s)) => Some(*s),
            _ => None,
        }
    }

    pub fn as_hole(&self) -> Option<HoleId> {
        match self {
            Concept::Hole(h) => Some(*h),
            _ => None,
        }
    }

    pub fn head(&self) -> Option<&Concept> {
        match self {
            Concept::Compound { head, .. } => Some(head),
            _ => None,
        }
    }

    pub fn args(&self) -> &[Concept] {
        match self {
            Concept::Compound { args, .. } => args,
            _ => &[],
        }
    }

    pub fn arg(&self, index: usize) -> Option<&Concept> {
        self.args().get(index)
    }

    /// Number of arguments. Atomic concepts and holes have arity 0.
    pub fn arity(&self) -> usize {
        self.args().len()
    }

    /// The symbol of the head, when the head is a named atomic concept.
    ///
    /// This is the primary store index key: `FriendWith<Greg, Keal>` files
    /// under `FriendWith`. Returns `None` for compounds whose head is itself a
    /// compound (partial application) and for non-compounds.
    pub fn head_symbol(&self) -> Option<SymbolId> {
        self.head().and_then(|h| h.as_symbol())
    }

    /// Depth of the deepest branch. An atomic concept or hole is depth 1.
    pub fn depth(&self) -> usize {
        match self {
            Concept::Atomic(_) | Concept::Hole(_) => 1,
            Concept::Compound { head, args } => {
                let deepest_arg = args.iter().map(|a| a.depth()).max().unwrap_or(0);
                1 + head.depth().max(deepest_arg)
            }
        }
    }

    /// Total node count, counting the head and every argument recursively.
    /// Used as the cost metric for synthesis budgets and abstraction utility.
    pub fn size(&self) -> usize {
        match self {
            Concept::Atomic(_) | Concept::Hole(_) => 1,
            Concept::Compound { head, args } => {
                1 + head.size() + args.iter().map(|a| a.size()).sum::<usize>()
            }
        }
    }

    // ---- identity ----

    /// Stable structural digest, identical across processes and machines.
    ///
    /// This is what the store indexes on for exact matches and what
    /// consolidation compares when deciding whether two subterms are the same
    /// shape. Discriminant bytes separate the three shapes so a compound can
    /// never collide with an atomic concept that happens to digest the same
    /// bytes.
    pub fn content_id(&self) -> ContentId {
        let mut hasher = blake3::Hasher::new();
        self.write_digest(&mut hasher);
        ContentId(*hasher.finalize().as_bytes())
    }

    fn write_digest(&self, hasher: &mut blake3::Hasher) {
        match self {
            Concept::Atomic(id) => {
                hasher.update(&[0x10]);
                id.write_digest(hasher);
            }
            Concept::Compound { head, args } => {
                hasher.update(&[0x11]);
                // Length is mixed in so that f(a, g(b)) and f(a, g, b) cannot
                // produce the same digest.
                hasher.update(&(args.len() as u32).to_le_bytes());
                head.write_digest(hasher);
                for arg in args.iter() {
                    arg.write_digest(hasher);
                }
            }
            Concept::Hole(h) => {
                hasher.update(&[0x12]);
                hasher.update(&h.0.to_le_bytes());
            }
        }
    }
}
