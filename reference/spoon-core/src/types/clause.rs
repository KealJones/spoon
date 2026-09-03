//! The output of the ears: a list of DRS-style clauses.
//!
//! One SCE sentence becomes one `Clause`. The interior dispatches on `act`.
//! Referents are discourse variables; predicates relate them. Nothing here is
//! executable yet; `Intent` (see `intent.rs`) is what the planner consumes.

use serde::{Deserialize, Serialize};

use super::value::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Declarative: store as facts. "John owns a dog."
    Assert,
    /// "Assistant, close the door!"
    Command,
    Question { kind: QuestionKind },
    /// "If X then Y." Stored as a policy/rule, not executed now.
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// "Does John own a dog?" / "Is Mary present?"
    YesNo,
    /// "Should Assistant save X?" - a suggestion seeking agreement.
    Should,
    /// "Who owns a dog?" - answer is an entity; `focus` names the variable asked about.
    Who { focus: String },
    /// "What is the wellbeing of Assistant?" / "What is X?"
    What { focus: String },
    /// "Which person should own the task?"
    Which { focus: String },
    /// "How many apples does Ben own?"
    HowMany { focus: String },
    /// "Where is X?" / "When does X happen?" (rare in SCE, kept for completeness)
    Where { focus: String },
    When { focus: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quant {
    /// "a dog": introduces a new referent.
    Indef,
    /// "the dog": must resolve to an existing referent (or becomes new with a warning).
    Def,
    Every,
    No,
    AtLeast(u32),
    AtMost(u32),
    Exactly(u32),
    /// "10 apples"
    Count(u32),
    /// Proper name or minted referent (`Object-X`).
    Named(String),
    /// Literal value: number, quoted string, path, url, date.
    Literal(Value),
    /// Interrogative variable: "who", "what".
    Wh,
}

/// A discourse referent introduced by a noun phrase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Referent {
    /// Variable name unique within the turn: "x1", "x2".
    pub var: String,
    /// Head noun lemma (lowercase singular), e.g. "dog". None for pure literals.
    pub noun: Option<String>,
    pub quant: Quant,
    /// Adjectives / modifiers: "red", "banned".
    #[serde(default)]
    pub mods: Vec<String>,
    /// Possessor variable for "Mary's phone" / "User's question".
    #[serde(default)]
    pub owner: Option<String>,
    /// Source text span for provenance and phrasing induction.
    #[serde(default)]
    pub span: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "term", rename_all = "snake_case")]
pub enum Term {
    Var { var: String },
    Value { value: Value },
    /// Embedded clause: "says that <clause>", "wants that <clause>".
    Sub { clause: Box<Clause> },
    /// Arithmetic expression to be evaluated: `3 / 500 * 3600`.
    Arith { expr: ArithExpr },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum ArithExpr {
    Num { value: f64 },
    Ref { var: String },
    Neg { of: Box<ArithExpr> },
    Bin { op: ArithOp, lhs: Box<ArithExpr>, rhs: Box<ArithExpr> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modal {
    Must,
    Should,
    May,
    Can,
}

/// A predicate over terms. `pred` is a verb/relation lemma as it appeared in
/// SCE ("owns", "calculate", "greets", "is-smarter-than"). Binding to an
/// `ActionId` happens in the interior, so the ears never need the CAN to parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pred {
    pub pred: String,
    pub args: Vec<Term>,
    #[serde(default)]
    pub negated: bool,
    #[serde(default)]
    pub modal: Option<Modal>,
    /// Prepositional adjuncts: ("toward", Term), ("at", Term), ("to", Term).
    #[serde(default)]
    pub adjuncts: Vec<(String, Term)>,
    /// Copula predications: "Mary is smart" -> pred "be", `attr: Some("smart")`.
    #[serde(default)]
    pub attr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clause {
    pub act: Act,
    pub referents: Vec<Referent>,
    /// Conditions that hold (or, for Command, the thing to do; for Question,
    /// the thing asked).
    pub conditions: Vec<Pred>,
    /// For `Act::Rule`: antecedent is `conditions`, consequent is `then`.
    #[serde(default)]
    pub then: Vec<Pred>,
    /// Referents introduced in the consequent of a rule.
    #[serde(default)]
    pub then_referents: Vec<Referent>,
    /// The SCE sentence this came from.
    pub sce: String,
}

impl Clause {
    pub fn referent(&self, var: &str) -> Option<&Referent> {
        self.referents
            .iter()
            .chain(self.then_referents.iter())
            .find(|r| r.var == var)
    }
    pub fn is_question(&self) -> bool {
        matches!(self.act, Act::Question { .. })
    }
}

/// How the ears produced the clauses. Logged on every turn for weaning metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarsPath {
    /// Input already parsed as SCE.
    Direct,
    /// Matched a learned slotted phrasing.
    Phrasing,
    /// Retrieval + alignment produced a parse.
    Retrieval,
    /// The LLM normalizer was needed.
    Llm,
    /// Nothing worked; interior gets a clarification signal.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarsResult {
    pub clauses: Vec<Clause>,
    pub path: EarsPath,
    /// The SCE text (normalized) that parsed. Stored with the utterance as a
    /// training pair when `path == Llm`.
    pub sce: String,
    pub confidence: f32,
    /// Words the normalizer could not map to the lexicon. Candidates for new
    /// concepts or synonyms.
    #[serde(default)]
    pub unknown_words: Vec<String>,
}
