//! The model path.
//!
//! The model's job is narrow on purpose: decompose an utterance into a short
//! sequence of concept operations drawn from a vocabulary it is handed. It is
//! not asked to compute anything, resolve anything, or decide anything. Its
//! output has to parse into concepts, so a hallucination fails loudly instead
//! of becoming a confident wrong answer.

use std::sync::Arc;

use spoon_concept::{Concept, SymbolTable, parse};
use spoon_seat::{Ears, Heard, LlmClient, LlmError, Message, Seat};

use crate::native::NativeEars;

pub struct ModelEars {
    client: LlmClient,
    native: NativeEars,
}

impl ModelEars {
    pub fn new(client: LlmClient) -> Self {
        ModelEars {
            client,
            native: NativeEars::new(),
        }
    }

    /// Built fresh each turn from the activation-ranked vocabulary, so the
    /// concepts this user actually reaches for sit at the top where the model
    /// will see them.
    fn prompt(vocabulary: &[Arc<str>]) -> String {
        let known = vocabulary
            .iter()
            .map(|w| w.as_ref())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"You translate messy human speech into concept expressions. You never compute, resolve, or answer anything. You only translate.

Write ONE line per step, in the order the speaker said them. Each line is one of:

assert-that<CONCEPT>   the speaker is stating something true
ask<CONCEPT>           the speaker is asking whether something holds, or asking for a value
do<CONCEPT>            the speaker wants something done or computed
chat<CONCEPT>          social talk with no request in it
correction<STEP>       the speaker is repairing what they just said; STEP is the replacement

A CONCEPT is written Head<Arg, Arg>. Arguments are concepts, quoted "text", numbers, true/false, or ?0 for something unspecified. Names are kebab-case.

Examples:
  "john has a dog"            -> assert-that<owns<john, dog>>
  "who owns a dog"            -> ask<owns<?0, dog>>
  "add 2 and 3"               -> do<add<2, 3>>
  "hey"                       -> chat<greet<>>
  "the weights, no the scores" -> do<sum<weights>>
                                  correction<do<sum<scores>>>

If a word means nothing you can express, write it as unknown<"the word"> inside the concept rather than guessing.

Concepts currently known: {known}

Prefer a known concept. Invent a new kebab-case name only when nothing fits.
Reply with the steps and nothing else. No prose, no explanation, no code fences."#
        )
    }

    /// Read the model's reply back into concepts.
    ///
    /// A line that does not parse is dropped rather than guessed at, and any
    /// `unknown<"word">` it contains is collected as a lead for the Teacher. If
    /// nothing parses the whole reading fails, which is correct: the model
    /// produced something Spoon cannot act on.
    fn parse_steps(reply: &str, table: &SymbolTable) -> (Vec<Concept>, Vec<Arc<str>>) {
        let mut steps = Vec::new();
        let mut unknown = Vec::new();
        for line in reply.lines() {
            let line = line.trim().trim_start_matches("- ").trim();
            if line.is_empty() || line.starts_with("```") {
                continue;
            }
            let Ok(concept) = parse(line, table) else {
                continue;
            };
            for node in spoon_concept::pre_order(&concept) {
                if node.head_symbol() == Some(spoon_concept::SymbolId::of("unknown"))
                    && let Some(word) = node
                        .arg(0)
                        .and_then(|a| a.as_ground())
                        .and_then(|g| g.as_str())
                {
                    unknown.push(Arc::from(word));
                }
            }
            steps.push(concept);
        }
        (steps, unknown)
    }
}

#[async_trait::async_trait]
impl Ears for ModelEars {
    async fn hear(&self, text: &str, vocabulary: &[Arc<str>]) -> Result<Heard, LlmError> {
        let messages = [
            Message::system(Self::prompt(vocabulary)),
            Message::user(text.to_string()),
        ];
        let reply = self.client.chat(Seat::Ears, &messages).await?;

        let table = SymbolTable::new();
        let (steps, unknown) = Self::parse_steps(&reply, &table);
        if steps.is_empty() {
            return Err(LlmError::Unusable {
                seat: "ears",
                detail: format!("nothing parsed from: {reply}"),
            });
        }
        // Confidence drops with each word the model could not place, because a
        // reading full of holes is a reading Spoon should be less willing to
        // act on.
        let confidence = (0.85 - 0.15 * unknown.len() as f64).max(0.2);
        let names = table.entries().into_iter().map(|(_, name)| name).collect();
        Ok(Heard {
            steps,
            unknown,
            confidence,
            used_model: true,
            names,
        })
    }

    fn hear_native(&self, text: &str) -> Option<Heard> {
        self.native.hear_native(text)
    }
}
