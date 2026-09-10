//! Teacher: the seat that teaches Spoon new meaning.
//!
//! It introduces concepts, relates them, and composes realizations out of
//! concepts. That is teaching. The synthesizer is the dumb layer that may only
//! compose from operators that already exist and check them against examples.
//!
//! Nothing it says is trusted as kernel. Everything lands at provisional tier.

use std::sync::Arc;

use spoon_concept::{Concept, SymbolTable, parse};
use spoon_ears::pycall;
use spoon_seat::{
    LlmClient, LlmError, Message, Seat, Spec, Taught, Teacher, TeacherAsk, TeacherReply,
};

pub struct ModelTeacher {
    client: LlmClient,
    table: Arc<SymbolTable>,
}

impl ModelTeacher {
    pub fn new(client: LlmClient, table: Arc<SymbolTable>) -> Self {
        ModelTeacher { client, table }
    }

    fn known(vocabulary: &[Arc<str>]) -> String {
        if vocabulary.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nConcepts Spoon already has:\n{}\n",
                vocabulary
                    .iter()
                    .map(|v| v.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    /// One lesson, as many lines as the meaning needs.
    ///
    /// A small model handed a menu of reply forms picked the cheapest item.
    /// This seat is a large model whose job is to teach, so the protocol is
    /// the lesson, not a single compose-from-known line.
    fn teach_prompt(vocabulary: &[Arc<str>]) -> String {
        format!(
            "You teach Spoon. Spoon's mind is Concepts: named ideas that can \
relate to each other and that can have realizations, which are concepts in \
and concepts out. You are not searching a program space. You are introducing \
meaning.\n\n\
Write as many lines as the lesson needs. Each line is one of:\n\n\
SYNONYM <word> = <concept>\n    \
the word already means a concept Spoon has\n\n\
CONCEPT <name> | <surface forms, comma separated> | <relations, comma separated>\n    \
a new idea. Relations give it consequences, for example \
participates(name, thing) or subtype-of(name, thing). You may introduce \
several. Put CONCEPT lines before any COMPOSE that uses them.\n\n\
COMPOSE <target> = <body>\n    \
how that concept is realized in terms of other concepts, using ?0 ?1 for \
its arguments. The body may use concepts you just introduced. This is \
meaning, not a native function.\n\n\
EXAMPLES <target> : <in> -> <out> ; <in> -> <out> ; <in> -> <out>\n    \
optional. The synthesizer searches a tiny body that fits these. Teaching \
does not depend on them. At least three, and they must be literally correct.\n\n\
UNKNOWN <short reason>\n    \
only if you genuinely cannot teach this.\n\n\
Prefer a concept Spoon already has when it already means the right thing. \
If the meaning needs a concept Spoon does not have, introduce it. Do not \
refuse just because a name is missing from the list below. Do not write \
Rust. Do not write a body whose outermost call is the target itself.\n\n\
Concepts are written head(arg, arg). Names are kebab-case. Arguments are \
concepts, quoted \"text\", numbers, true/false, or ?0 ?1.\n\
Reply with the lesson lines and nothing else. No prose, no code fences.\
{}",
            Self::known(vocabulary)
        )
    }

    fn prompt(ask: &TeacherAsk, vocabulary: &[Arc<str>]) -> String {
        match ask {
            TeacherAsk::Reading { .. } => format!(
                "You check whether a sentence was understood correctly, and \
correct it if not.\n\n\
Reply in exactly this form, one step per line:\n\n\
READING\n\
assert-that(CONCEPT)   the speaker stated something true\n\
ask(CONCEPT)           the speaker asked whether something holds, or for a value\n\
do(CONCEPT)            the speaker wants something done or computed\n\
chat(CONCEPT)          social talk with no request in it\n\n\
Example:\n\
READING\n\
do(max-of(list(4, 9, 2, 7)))\n\n\
If the reading shown to you is already right, reply exactly: CORRECT\n\n\
After the steps you may add one line beginning RULE, stating a general lesson \
if the mistake would repeat on other sentences. Write it as an instruction to \
whoever reads the next sentence, not as a remark about this one. Omit it when \
the mistake was particular to this sentence.\n\n\
Example:\n\
READING\n\
do(max-of(list(4, 9, 2, 7)))\n\
RULE Write a list as list(a, b, c), never list([a, b, c]).\n\n\
A list is written list(a, b, c) with the items as separate arguments, never \
list([a, b, c]). Use the concepts listed below and match their stated shapes.\n\n\
Concepts are written head(arg, arg). Names are kebab-case.\n\
If you genuinely cannot answer, reply exactly: UNKNOWN <short reason>\
{}",
                Self::known(vocabulary)
            ),
            TeacherAsk::Vocabulary { .. }
            | TeacherAsk::Concept { .. }
            | TeacherAsk::Capability { .. }
            | TeacherAsk::Examples { .. } => Self::teach_prompt(vocabulary),
        }
    }

    fn describe(&self, ask: &TeacherAsk) -> String {
        match ask {
            TeacherAsk::Vocabulary {
                word, utterance, ..
            } => {
                format!(
                    "The word \"{word}\" appeared in: \"{utterance}\". Teach Spoon what it means here."
                )
            }
            TeacherAsk::Capability {
                concept,
                attempted,
                utterance,
                unknown,
            } => {
                let unknown = if unknown.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nWords the ears could not place: {}",
                        unknown
                            .iter()
                            .map(|w| w.as_ref())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                format!(
                    "Someone said: {utterance:?}{unknown}\n\n\
No way to carry out {}. Already tried: {}.\n\n\
Teach Spoon the missing meaning. Introduce any concepts it needs, relate them, \
and compose realizations. The synthesizer will separately hunt a tiny body \
from examples if you give them; that is optional.",
                    pycall::render_pycall(concept, &self.table),
                    if attempted.is_empty() {
                        "nothing".to_string()
                    } else {
                        attempted.join(", ")
                    }
                )
            }
            TeacherAsk::Examples { concept, arity } => format!(
                "Give input and output examples for {} which takes {arity} argument(s), \
and teach any missing meaning around it.",
                pycall::render_pycall(concept, &self.table)
            ),
            TeacherAsk::Reading {
                utterance,
                heard,
                trouble,
            } => format!(
                "Someone said: {utterance:?}\n\nIt was read as:\n{heard}\n\n\
Acting on that went wrong: {trouble}\n\nWas the reading right, and if not what should it be?"
            ),
            TeacherAsk::Concept { word, context } => {
                format!(
                    "\"{word}\" appeared in: \"{context}\". Teach Spoon what kind of thing it is."
                )
            }
        }
    }

    fn parse_replies(&self, reply: &str) -> Vec<TeacherReply> {
        let trimmed = reply.trim();
        if trimmed.is_empty() {
            return vec![TeacherReply::Unknown {
                why: Arc::from("empty reply"),
            }];
        }
        if trimmed
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case("CORRECT"))
            && !trimmed
                .lines()
                .any(|l| l.trim().eq_ignore_ascii_case("READING"))
        {
            return vec![TeacherReply::Unknown {
                why: Arc::from("the reading was already correct"),
            }];
        }
        if trimmed
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case("READING"))
        {
            return vec![self.parse_reading(reply)];
        }

        let mut replies = Vec::new();
        for line in reply.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("```") || line.starts_with('#') {
                continue;
            }
            if let Some(parsed) = self.parse_line(line) {
                replies.push(parsed);
            }
        }
        if replies.is_empty() {
            vec![TeacherReply::Unknown {
                why: Arc::from("unrecognized reply"),
            }]
        } else if replies
            .iter()
            .all(|r| matches!(r, TeacherReply::Unknown { .. }))
        {
            replies
        } else {
            replies
                .into_iter()
                .filter(|r| !matches!(r, TeacherReply::Unknown { .. }))
                .collect()
        }
    }

    fn parse_line(&self, line: &str) -> Option<TeacherReply> {
        let unknown = |why: &str| TeacherReply::Unknown {
            why: Arc::from(why),
        };
        if let Some(rest) = line.strip_prefix("SYNONYM ") {
            if let Some((word, concept)) = rest.split_once('=') {
                let concept_str = concept.trim();
                let parsed = pycall::parse_pycall(concept_str, &self.table)
                    .ok()
                    .or_else(|| parse(concept_str, &self.table).ok());
                if let Some(concept) = parsed {
                    return Some(TeacherReply::Synonym {
                        word: Arc::from(word.trim().trim_matches('"')),
                        concept,
                        confidence: 0.75,
                    });
                }
            }
            return Some(unknown("malformed synonym"));
        }
        if let Some(rest) = line.strip_prefix("CONCEPT ") {
            let parts: Vec<&str> = rest.split('|').collect();
            let Some(name) = parts.first().map(|n| n.trim()) else {
                return Some(unknown("nameless concept"));
            };
            if name.is_empty() {
                return Some(unknown("nameless concept"));
            }
            let surface_forms: Vec<Arc<str>> = parts
                .get(1)
                .map(|f| {
                    f.split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(Arc::from)
                        .collect()
                })
                .unwrap_or_default();
            let relations: Vec<Concept> = parts
                .get(2)
                .map(|r| {
                    r.split(',')
                        .filter_map(|s| {
                            let s = s.trim();
                            pycall::parse_pycall(s, &self.table)
                                .ok()
                                .or_else(|| parse(s, &self.table).ok())
                        })
                        .collect()
                })
                .unwrap_or_default();
            return Some(TeacherReply::NewConcept {
                concept: Concept::named(name),
                relations,
                surface_forms,
            });
        }
        if let Some(rest) = line.strip_prefix("COMPOSE ")
            && let Some((target, body)) = rest.split_once('=')
        {
            let body_str = body.trim();
            // Accept paren notation first, angle-bracket as fallback so old
            // model replies still land.
            let parsed = pycall::parse_pycall(body_str, &self.table)
                .ok()
                .or_else(|| parse(body_str, &self.table).ok());
            if let Some(body) = parsed {
                return Some(TeacherReply::Composition {
                    target: Concept::named(target.trim()),
                    body,
                });
            }
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
                    .filter_map(|i| {
                        let i = i.trim();
                        pycall::parse_pycall(i, &self.table)
                            .ok()
                            .or_else(|| parse(i, &self.table).ok())
                    })
                    .collect();
                let o = output.trim();
                let Some(parsed_output) = pycall::parse_pycall(o, &self.table)
                    .ok()
                    .or_else(|| parse(o, &self.table).ok())
                else {
                    continue;
                };
                if !parsed_inputs.is_empty() {
                    examples.push((parsed_inputs, parsed_output));
                }
            }
            if examples.len() < 3 {
                return Some(unknown("too few usable examples"));
            }
            return Some(TeacherReply::Spec(Spec {
                target: Concept::named(target.trim()),
                examples,
                note: None,
            }));
        }
        if let Some(rest) = line.strip_prefix("UNKNOWN ") {
            return Some(unknown(rest.trim()));
        }
        None
    }

    fn parse_reading(&self, reply: &str) -> TeacherReply {
        let unknown = |why: &str| TeacherReply::Unknown {
            why: Arc::from(why),
        };
        let steps: Vec<Concept> = reply
            .lines()
            .skip_while(|l| !l.trim().eq_ignore_ascii_case("READING"))
            .skip(1)
            .take_while(|l| !l.trim().starts_with("RULE "))
            .filter_map(|l| {
                let s = l.trim();
                pycall::parse_pycall(s, &self.table)
                    .ok()
                    .or_else(|| parse(s, &self.table).ok())
            })
            .filter(|c: &Concept| c.is_compound())
            .collect();
        if steps.is_empty() {
            return unknown("a reading with no usable steps");
        }
        let lesson = reply
            .lines()
            .find_map(|l| l.trim().strip_prefix("RULE "))
            .map(|r| Arc::from(r.trim()));
        TeacherReply::Reading { steps, lesson }
    }
}

#[async_trait::async_trait]
impl Teacher for ModelTeacher {
    async fn teach(&self, ask: &TeacherAsk, vocabulary: &[Arc<str>]) -> Result<Taught, LlmError> {
        let messages = [
            Message::system(Self::prompt(ask, vocabulary)),
            Message::user(self.describe(ask)),
        ];
        let (raw, exchange) = self
            .client
            .chat_with_exchange(Seat::Teacher, &messages)
            .await?;
        Ok(Taught {
            replies: self.parse_replies(&raw),
            exchange: Some(exchange),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_seat::{LlmConfig, SeatCounters};

    fn teacher() -> ModelTeacher {
        ModelTeacher::new(
            LlmClient::new(
                LlmConfig::ollama("unused"),
                Arc::new(SeatCounters::default()),
            ),
            Arc::new(SymbolTable::new()),
        )
    }

    #[test]
    fn a_lesson_can_mint_several_concepts_and_compose_them() {
        let replies = teacher().parse_replies(
            "CONCEPT adorableness-score | cute, adorable |\n\
             CONCEPT most-adorable | cutest, most adorable |\n\
             COMPOSE most-adorable = list-max-of<?0>\n",
        );
        assert_eq!(replies.len(), 3);
        assert!(matches!(
            &replies[0],
            TeacherReply::NewConcept { concept, .. } if concept == &Concept::named("adorableness-score")
        ));
        assert!(matches!(
            &replies[1],
            TeacherReply::NewConcept { concept, .. } if concept == &Concept::named("most-adorable")
        ));
        assert!(matches!(
            &replies[2],
            TeacherReply::Composition { target, body }
                if target == &Concept::named("most-adorable")
                    && body.head_symbol() == Some(spoon_concept::SymbolId::of("list-max-of"))
        ));
    }

    #[test]
    fn compose_without_examples_is_still_a_lesson() {
        let replies = teacher().parse_replies("COMPOSE double = add<?0, ?0>");
        assert!(matches!(
            &replies[0],
            TeacherReply::Composition { target, .. } if target == &Concept::named("double")
        ));
    }

    #[test]
    fn compose_paren_syntax_accepted() {
        let replies = teacher().parse_replies("COMPOSE double = add(?0, ?0)");
        assert!(
            matches!(
                &replies[0],
                TeacherReply::Composition { target, body }
                    if target == &Concept::named("double")
                        && body.head_symbol() == Some(spoon_concept::SymbolId::of("add"))
            ),
            "paren COMPOSE body must parse"
        );
    }

    #[test]
    fn capability_prompt_does_not_forbid_inventing_concepts() {
        let prompt = ModelTeacher::teach_prompt(&[]);
        assert!(
            prompt.contains("If the meaning needs a concept Spoon does not have, introduce it")
        );
        assert!(!prompt.contains("If you cannot express it with the concepts listed, say UNKNOWN"));
        assert!(!prompt.contains("Reply with ONE line"));
    }
}
