//! Teacher: filling a gap Spoon met and could not cross.
//!
//! It proposes structure, not executable bodies. Naming what a word means, or
//! which known concepts compose into a missing one, is a different act from
//! writing code that runs, and everything it says arrives at provisional tier
//! so experience still decides.

use std::sync::Arc;

use spoon_concept::{Concept, SymbolTable, parse, render};
use spoon_seat::{LlmClient, LlmError, Message, Seat, Teacher, TeacherAsk, TeacherReply};

pub struct ModelTeacher {
    client: LlmClient,
    table: Arc<SymbolTable>,
}

impl ModelTeacher {
    pub fn new(client: LlmClient, table: Arc<SymbolTable>) -> Self {
        ModelTeacher { client, table }
    }

    fn prompt() -> &'static str {
        r#"You help a reasoning system fill a gap in what it knows. You never write code.

Reply with exactly one line, in one of these forms:

SYNONYM <word> = <concept>
    the word means something the system already has

CONCEPT <name> | <surface forms, comma separated> | <relations, comma separated>
    a genuinely new idea. Relations are concepts like participates<name, thing>
    or subtype-of<name, thing> that give the new idea consequences.

COMPOSE <target> = <body>
    the missing capability is built from ones that exist, written as a concept
    expression using ?0, ?1 for its arguments. Example:
    COMPOSE double = add<?0, ?0>

UNKNOWN <short reason>
    you genuinely cannot say. This is a real answer and better than a guess.

Concepts are written Head<Arg, Arg>, names are kebab-case. No prose, no code fences."#
    }

    fn describe(&self, ask: &TeacherAsk) -> String {
        match ask {
            TeacherAsk::Vocabulary {
                word, utterance, ..
            } => {
                format!("The word \"{word}\" appeared in: \"{utterance}\". What does it mean here?")
            }
            TeacherAsk::Capability { concept, attempted } => format!(
                "No way to carry out {}. Already tried: {}. How could it be built from simpler concepts?",
                render(concept, &self.table),
                if attempted.is_empty() {
                    "nothing".to_string()
                } else {
                    attempted.join(", ")
                }
            ),
            TeacherAsk::Examples { concept, arity } => format!(
                "Give input and output examples for {} which takes {arity} argument(s).",
                render(concept, &self.table)
            ),
            TeacherAsk::Concept { word, context } => {
                format!("\"{word}\" appeared in: \"{context}\". What kind of thing is it?")
            }
        }
    }

    /// Read one line back. Anything unrecognized becomes `Unknown` rather than
    /// a guess, and an admitted blank is worth storing so the same question is
    /// not asked forever.
    fn parse_reply(&self, reply: &str, ask: &TeacherAsk) -> TeacherReply {
        let line = reply
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let unknown = |why: &str| TeacherReply::Unknown {
            why: Arc::from(why),
        };

        if let Some(rest) = line.strip_prefix("SYNONYM ") {
            if let Some((word, concept)) = rest.split_once('=')
                && let Ok(concept) = parse(concept.trim(), &self.table)
            {
                return TeacherReply::Synonym {
                    word: Arc::from(word.trim().trim_matches('"')),
                    concept,
                    confidence: 0.75,
                };
            }
            return unknown("malformed synonym");
        }
        if let Some(rest) = line.strip_prefix("CONCEPT ") {
            let parts: Vec<&str> = rest.split('|').collect();
            let Some(name) = parts.first().map(|n| n.trim()) else {
                return unknown("nameless concept");
            };
            let surface_forms: Vec<Arc<str>> = parts
                .get(1)
                .map(|f| f.split(',').map(|s| Arc::from(s.trim())).collect())
                .unwrap_or_default();
            let relations: Vec<Concept> = parts
                .get(2)
                .map(|r| {
                    r.split(',')
                        .filter_map(|s| parse(s.trim(), &self.table).ok())
                        .collect()
                })
                .unwrap_or_default();
            return TeacherReply::NewConcept {
                concept: Concept::named(name),
                relations,
                surface_forms,
            };
        }
        if let Some(rest) = line.strip_prefix("COMPOSE ")
            && let Some((target, body)) = rest.split_once('=')
            && let Ok(body) = parse(body.trim(), &self.table)
        {
            return TeacherReply::Composition {
                target: Concept::named(target.trim()),
                body,
            };
        }
        if let Some(rest) = line.strip_prefix("UNKNOWN ") {
            return unknown(rest.trim());
        }
        let _ = ask;
        unknown("unrecognized reply")
    }
}

#[async_trait::async_trait]
impl Teacher for ModelTeacher {
    async fn teach(&self, ask: &TeacherAsk) -> Result<TeacherReply, LlmError> {
        let messages = [
            Message::system(Self::prompt()),
            Message::user(self.describe(ask)),
        ];
        let reply = self.client.chat(Seat::Teacher, &messages).await?;
        Ok(self.parse_reply(&reply, ask))
    }
}
