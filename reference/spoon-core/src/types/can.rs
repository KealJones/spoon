//! Concept Action Network: the self-grown ontology and capability library.
//!
//! Concepts are what Spoon can talk about (nouns). Actions are what Spoon can
//! do (verbs). Relations between concepts are actions too: `owns(Person, Thing)
//! -> Bool` is an action with a `Relation` role, so "John owns a dog" and
//! "who owns a dog" both route through the same graph.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::ir::Program;
use super::value::{Type, Value};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConceptId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionId(pub String);

impl From<&str> for ConceptId {
    fn from(s: &str) -> Self {
        ConceptId(s.to_string())
    }
}
impl From<String> for ConceptId {
    fn from(s: String) -> Self {
        ConceptId(s)
    }
}
impl From<&str> for ActionId {
    fn from(s: &str) -> Self {
        ActionId(s.to_string())
    }
}
impl From<String> for ActionId {
    fn from(s: String) -> Self {
        ActionId(s)
    }
}
impl fmt::Display for ConceptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Property {
    pub name: String,
    pub ty: Type,
    pub required: bool,
    pub max: Cardinality,
    /// Viv visibility: may the planner project this property out of its parent
    /// mid-plan? `false` = only as a goal.
    #[serde(default = "default_true")]
    pub projectable: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConceptKind {
    /// Wraps a primitive type with a name (e.g. `Temperature` over Float).
    Primitive { ty: Type },
    Structure { properties: Vec<Property> },
    Enum { symbols: Vec<String> },
    /// A category of named individuals: `Person`, `Dog`. Instances are
    /// `Value::Name`s stored as facts (`is_a(Rex, Dog)`).
    Entity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Concept {
    pub id: ConceptId,
    pub kind: ConceptKind,
    /// is-a. Multiple inheritance allowed.
    #[serde(default)]
    pub extends: Vec<ConceptId>,
    /// Viv role-of: a typed view over another concept (`ArrivalAirport` of `Airport`).
    #[serde(default)]
    pub role_of: Option<ConceptId>,
    /// Surface nouns that denote this concept. Lowercase singular lemmas.
    #[serde(default)]
    pub nouns: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tier: Tier,
    #[serde(default)]
    pub provenance: Provenance,
}

impl Concept {
    pub fn entity(id: &str, nouns: &[&str]) -> Concept {
        Concept {
            id: id.into(),
            kind: ConceptKind::Entity,
            extends: vec![],
            role_of: None,
            nouns: nouns.iter().map(|s| s.to_string()).collect(),
            description: String::new(),
            tier: Tier::Kernel,
            provenance: Provenance::Kernel,
        }
    }
    pub fn ty(&self) -> Type {
        match &self.kind {
            ConceptKind::Primitive { ty } => ty.clone(),
            _ => Type::Concept(self.id.clone()),
        }
    }
}

/// Side-effect class of an action. Drives permission prompts and tells the
/// synthesizer what it may enumerate (pure and read only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Pure,
    Read,
    Write,
    Network,
    Shell,
}

impl Effect {
    pub fn max(self, other: Effect) -> Effect {
        if other > self { other } else { self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Hand-built Stage 0. Never evicted.
    Kernel,
    /// Learned, not yet proven by reuse.
    #[default]
    Provisional,
    /// Reused enough to be trusted and preferred by retrieval.
    Consolidated,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "src", rename_all = "snake_case")]
pub enum Provenance {
    #[default]
    Kernel,
    /// Taught explicitly by the user in conversation.
    User { utterance: String },
    /// Produced by the synthesizer against a spec.
    Synthesized { spec_id: String },
    /// Consolidated out of repeated plan fragments.
    Consolidated { from: Vec<String> },
    /// Proposed by the teacher during pretrain and verified.
    Teacher { lesson_id: String },
    Import { seed: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub ty: Type,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default = "one")]
    pub max: Cardinality,
    /// Default when missing, before prompting the user.
    #[serde(default)]
    pub default: Option<Value>,
}

fn one() -> Cardinality {
    Cardinality::One
}

impl Input {
    pub fn required(name: &str, ty: Type) -> Input {
        Input { name: name.into(), ty, required: true, max: Cardinality::One, default: None }
    }
    pub fn optional(name: &str, ty: Type, default: Option<Value>) -> Input {
        Input { name: name.into(), ty, required: false, max: Cardinality::One, default }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "impl", rename_all = "snake_case")]
pub enum Impl {
    /// Implemented in Rust in the kernel, looked up by id.
    Primitive,
    Program { program: Program },
}

/// What grammatical role the action plays in SCE. Determines how the parser
/// and realizer treat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// "Assistant, calculate X!" - a thing Spoon does.
    #[default]
    Command,
    /// "John owns a dog" - a predicate over entities, storable as a fact.
    Relation,
    /// "the length of X" - a function used inside noun phrases.
    Function,
    /// "User greets Assistant" - a conversational move.
    Dialog,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Stats {
    pub uses: u64,
    pub successes: u64,
    pub failures: u64,
    /// Unix ms of last use, for ACT-R style activation.
    pub last_used: Option<i64>,
    /// Sparse history of use timestamps (bounded) for base-level activation.
    #[serde(default)]
    pub history: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: ActionId,
    pub inputs: Vec<Input>,
    pub output: Type,
    pub effect: Effect,
    pub imp: Impl,
    #[serde(default)]
    pub role: Role,
    /// Verb lemmas / verb phrases that denote this action in SCE. First is
    /// canonical. Multi-word phrases use hyphens (`looks-for`).
    #[serde(default)]
    pub verbs: Vec<String>,
    /// Natural phrasings harvested from usage ("4 twice", "double it"). Feed the
    /// retrieval index, not the grammar.
    #[serde(default)]
    pub phrasings: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tier: Tier,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub stats: Stats,
}

impl Action {
    pub fn primitive(
        id: &str,
        verbs: &[&str],
        inputs: Vec<Input>,
        output: Type,
        effect: Effect,
        description: &str,
    ) -> Action {
        Action {
            id: id.into(),
            inputs,
            output,
            effect,
            imp: Impl::Primitive,
            role: Role::Command,
            verbs: verbs.iter().map(|s| s.to_string()).collect(),
            phrasings: vec![],
            description: description.into(),
            tier: Tier::Kernel,
            provenance: Provenance::Kernel,
            stats: Stats::default(),
        }
    }
    pub fn with_role(mut self, role: Role) -> Action {
        self.role = role;
        self
    }
    pub fn arity(&self) -> usize {
        self.inputs.len()
    }
    pub fn input_types(&self) -> Vec<Type> {
        self.inputs.iter().map(|i| i.ty.clone()).collect()
    }
    pub fn is_pure(&self) -> bool {
        self.effect == Effect::Pure
    }
    pub fn canonical_verb(&self) -> &str {
        self.verbs.first().map(String::as_str).unwrap_or(&self.id.0)
    }
}

/// A stored proposition: `pred(args)` with truth and validity time.
/// Facts are the semantic memory; episodes are the autobiographical one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: i64,
    pub pred: ActionId,
    pub args: Vec<Value>,
    pub truth: bool,
    /// Modal: None = plain assertion, Some("should"|"must"|"may"|"can").
    #[serde(default)]
    pub modal: Option<String>,
    pub asserted_at: i64,
    /// Bi-temporal: when this stopped being true, if ever.
    #[serde(default)]
    pub invalidated_at: Option<i64>,
    /// Who said it: "user", "assistant", "teacher", or a named third party.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub episode_id: Option<i64>,
}
