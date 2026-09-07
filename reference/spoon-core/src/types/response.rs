//! What the interior decided to say. The mouth turns this into English.
//!
//! Every fact in the reply must be present here. The renderer may not add
//! values; the faithfulness check verifies every `Value` in the plan appears
//! in the surface text.

use serde::{Deserialize, Serialize};

use super::can::{ActionId, Effect};
use super::value::{Type, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "move", rename_all = "snake_case")]
pub enum Move {
    // --- grounding / social ---
    Greet { returning: bool },
    Farewell,
    Thanks,
    /// Acknowledge a stored assertion. `summary` is a short SCE-ish restatement.
    Ack { summary: String },
    /// Emotional attunement. `feeling` is what we inferred ("frustrated"),
    /// `about` the topic.
    Empathize { feeling: String, about: String },
    /// Reflect the user's situation back before advising.
    Reflect { summary: String },

    // --- content ---
    /// Answer to a question. `values` are the actual answers; `question` is the SCE.
    Answer { question: String, values: Vec<Value>, source: Option<String> },
    /// A yes/no answer with optional supporting fact.
    YesNo { question: String, answer: bool, because: Option<String> },
    /// Result of executing a command.
    Result { action: ActionId, value: Value, steps: usize },
    /// A stance Spoon holds, with its reasons. Revisable.
    Opinion { topic: String, stance: String, reasons: Vec<String>, confidence: f32 },
    /// Advice with options and tradeoffs, grounded in recalled facts.
    Advise { situation: String, options: Vec<AdviceOption>, leaning: Option<String> },
    /// Recalled episode(s) relevant to the topic.
    Recall { topic: String, episodes: Vec<String> },
    /// Explain what Spoon did or why.
    Explain { text: String },
    /// Free informational content that is already decided (e.g. from a
    /// knowledge source). `source` must be set.
    Info { text: String, source: String },

    // --- dialog management ---
    /// Ask the user to pick or supply something.
    Clarify { question: String, options: Vec<String>, slot_type: Option<Type> },
    /// Missing required input for a plan.
    Elicit { input_name: String, ty: Type, for_action: ActionId },
    /// Confirm before a side effect.
    AskPermission { action: ActionId, effect: Effect, description: String },
    /// Spoon needs I/O examples to synthesize a capability.
    AskExamples { capability: String, signature: String },
    /// A word Spoon does not know; ask what it means.
    UnknownWord { word: String, guess: Option<String> },
    Refuse { reason: String },
    /// Spoon learned something durable this turn.
    Learned { what: String },
    /// Honest failure.
    CannotDo { what: String, reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdviceOption {
    pub option: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
}

/// Tone hints for the realizer. Chosen by dialog capabilities, not the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Casual,
    Warm,
    Direct,
    Playful,
    Serious,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponsePlan {
    pub moves: Vec<Move>,
    #[serde(default)]
    pub tone: Tone,
    /// Values that must appear verbatim (after `render()`) in the output.
    /// Filled by `ResponsePlan::finalize`.
    #[serde(default)]
    pub must_mention: Vec<String>,
}

impl ResponsePlan {
    pub fn new(moves: Vec<Move>) -> ResponsePlan {
        let mut rp = ResponsePlan { moves, tone: Tone::Casual, must_mention: vec![] };
        rp.finalize();
        rp
    }
    pub fn single(m: Move) -> ResponsePlan {
        ResponsePlan::new(vec![m])
    }
    pub fn push(&mut self, m: Move) {
        self.moves.push(m);
        self.finalize();
    }
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }
    /// Recompute `must_mention` from the moves.
    pub fn finalize(&mut self) {
        let mut out = Vec::new();
        for m in &self.moves {
            match m {
                Move::Answer { values, .. } => out.extend(values.iter().map(Value::render)),
                Move::Result { value, .. } => out.push(value.render()),
                Move::YesNo { answer, .. } => out.push(if *answer { "yes" } else { "no" }.into()),
                _ => {}
            }
        }
        out.retain(|s| !s.is_empty() && s.len() < 200);
        self.must_mention = out;
    }
    /// True if the plan is only a clarification / elicitation (the turn is a
    /// question back to the user).
    pub fn is_asking(&self) -> bool {
        !self.moves.is_empty()
            && self.moves.iter().all(|m| {
                matches!(
                    m,
                    Move::Clarify { .. }
                        | Move::Elicit { .. }
                        | Move::AskPermission { .. }
                        | Move::AskExamples { .. }
                        | Move::UnknownWord { .. }
                )
            })
    }
}
