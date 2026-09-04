//! Teacher: filling a gap Spoon met and could not cross.
//!
//! It proposes structure, not executable bodies. Naming what a word means, or
//! which known concepts compose into a missing one, is a different act from
//! writing code that runs, and everything it says arrives at provisional tier
//! so experience still decides.

use std::sync::Arc;

use spoon_concept::{Concept, SymbolTable, parse, render};
use spoon_seat::{LlmClient, LlmError, Message, Seat, Spec, Teacher, TeacherAsk, TeacherReply};

pub struct ModelTeacher {
    client: LlmClient,
    table: Arc<SymbolTable>,
}

impl ModelTeacher {
    pub fn new(client: LlmClient, table: Arc<SymbolTable>) -> Self {
        ModelTeacher { client, table }
    }

    /// The reply forms, tailored to what was actually asked.
    ///
    /// Offering every form for every question does not work: handed a menu, a
    /// small model reliably picks the cheapest item on it, so asking how to
    /// build a capability came back as a synonym. Each ask now names the one
    /// form that answers it, with UNKNOWN as the only alternative.
    fn prompt(ask: &TeacherAsk, vocabulary: &[Arc<str>]) -> String {
        let known = if vocabulary.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nConcepts available to build from: {}\n\nUse these. \
Do not invent a concept that is not listed unless nothing listed can express it.",
                vocabulary
                    .iter()
                    .map(|v| v.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let common = "Concepts are written Head<Arg, Arg>. Names are kebab-case. Arguments are \
concepts, quoted \"text\", numbers, true/false, or ?0 ?1 for a function's arguments. \
Reply with ONE line and nothing else: no prose, no explanation, no code fences. \
If you genuinely cannot answer, reply exactly: UNKNOWN <short reason>";

        match ask {
            TeacherAsk::Vocabulary { .. } | TeacherAsk::Concept { .. } => format!(
                "You say what an unfamiliar word means to a reasoning system.\n\n\
Reply in ONE of these two forms:\n\n\
SYNONYM <word> = <concept>\n    \
the word means something the system already has\n\n\
CONCEPT <name> | <surface forms, comma separated> | <relations, comma separated>\n    \
a genuinely new idea. Relations give it consequences, for example\n    \
participates<name, thing> or subtype-of<name, thing>\n\n{common}{known}"
            ),

            TeacherAsk::Capability { .. } => format!(
                "You say how a missing capability could be BUILT from simpler ones.\n\n\
Reply in exactly this form:\n\n\
COMPOSE <target> = <body>\n    \
the body is a concept expression using ?0, ?1 for the arguments\n\n\
Examples:\n\
COMPOSE double = add<?0, ?0>\n\
COMPOSE average = div<sum<?0>, count<?0>>\n\
COMPOSE longest = max-of<map<?0, text-length>>\n\n\
Do NOT reply with a synonym. The word is not the problem; the system cannot \
DO the thing. If you cannot express it with the concepts listed, say UNKNOWN.\n\n{common}{known}"
            ),

            TeacherAsk::Examples { .. } => format!(
                "You give worked input and output examples so a program synthesizer \
can search for a body that fits them.\n\n\
Reply in exactly this form, on one line:\n\n\
EXAMPLES <target> : <in> -> <out> ; <in> -> <out> ; <in> -> <out>\n\n\
Multiple inputs to one example are separated by commas.\n\n\
Examples:\n\
EXAMPLES double : 3 -> 6 ; 5 -> 10 ; 0 -> 0\n\
EXAMPLES add-two : 1, 2 -> 3 ; 10, 5 -> 15\n\
EXAMPLES count-of : [1, 2, 3] -> 3 ; [] -> 0\n\n\
Give at least three examples. They must be literally correct: the synthesizer \
verifies every one and discards anything that fails even a single case.\n\n{common}{known}"
            ),
        }
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
        if let Some(rest) = line.strip_prefix("EXAMPLES ")
            && let Some((target, cases)) = rest.split_once(':')
        {
            let mut examples = Vec::new();
            for case in cases.split(';') {
                let Some((inputs, output)) = case.split_once("->") else {
                    continue;
                };
                let parsed_inputs: Vec<Concept> = inputs
                    .split(',')
                    .filter_map(|i| parse(i.trim(), &self.table).ok())
                    .collect();
                let Ok(parsed_output) = parse(output.trim(), &self.table) else {
                    continue;
                };
                if !parsed_inputs.is_empty() {
                    examples.push((parsed_inputs, parsed_output));
                }
            }
            // Three minimum. Two examples fit far too many programs, and the
            // search returns the first thing that matches, so a body that
            // memorizes both would be accepted and called learned.
            if examples.len() < 3 {
                return unknown("too few usable examples");
            }
            return TeacherReply::Spec(Spec {
                target: Concept::named(target.trim()),
                examples,
                note: None,
            });
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
    async fn teach(
        &self,
        ask: &TeacherAsk,
        vocabulary: &[Arc<str>],
    ) -> Result<TeacherReply, LlmError> {
        let messages = [
            Message::system(Self::prompt(ask, vocabulary)),
            Message::user(self.describe(ask)),
        ];
        let reply = self.client.chat(Seat::Teacher, &messages).await?;
        Ok(self.parse_reply(&reply, ask))
    }
}
