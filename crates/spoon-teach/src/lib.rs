//! Teacher: filling a gap Spoon met and could not cross.
//!
//! It proposes structure, not executable bodies. Naming what a word means, or
//! which known concepts compose into a missing one, is a different act from
//! writing code that runs, and everything it says arrives at provisional tier
//! so experience still decides.

use std::sync::Arc;

use spoon_concept::{Concept, SymbolTable, parse, render};
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
                "\n\nHey Homie, You are a Teacher. You are here to help assist \"spoon\" \
become smarter and more capable. Its entire mind is built using \"Concepts\" \
and right now it isnt very good at figuring out stuff on its own. That is why
we need your help. These are the things that spoon currently knows as concepts.
                \n\nConcepts available to build from: {}\n\nUse these. \
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
DO the thing. If you cannot express it with the concepts listed, say UNKNOWN.\n\n\
If the notes below say a form of this concept already exists, you are adding a \
second one for the input it could not handle, not replacing it. Both are kept \
and whichever works is used. Your body may call the existing form: reversing \
text can use the list reversal, given something either side to convert.\n\n{common}{known}"
            ),

            TeacherAsk::Reading { .. } => format!(
                "You check whether a sentence was understood correctly, and \
correct it if not.\n\n\
Reply in exactly this form, one step per line:\n\n\
READING\n\
assert-that<CONCEPT>   the speaker stated something true\n\
ask<CONCEPT>           the speaker asked whether something holds, or for a value\n\
do<CONCEPT>            the speaker wants something done or computed\n\
chat<CONCEPT>          social talk with no request in it\n\n\
Example:\n\
READING\n\
do<max-of<list<4, 9, 2, 7>>>\n\n\
If the reading shown to you is already right, reply exactly: CORRECT\n\n\
After the steps you may add one line beginning RULE, stating a general lesson \
if the mistake would repeat on other sentences. Write it as an instruction to \
whoever reads the next sentence, not as a remark about this one. Omit it when \
the mistake was particular to this sentence.\n\n\
Example:\n\
READING\n\
do<max-of<list<4, 9, 2, 7>>>\n\
RULE Write a list as list<a, b, c>, never list<[a, b, c]>.\n\n\
A list is written list<a, b, c> with the items as separate arguments, never \
list<[a, b, c]>. Use the concepts listed below and match their stated shapes.\n\n{common}{known}"
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
            TeacherAsk::Reading {
                utterance,
                heard,
                trouble,
            } => format!(
                "Someone said: {utterance:?}\n\nIt was read as:\n{heard}\n\n\
Acting on that went wrong: {trouble}\n\nWas the reading right, and if not what should it be?"
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
        if line.eq_ignore_ascii_case("CORRECT") {
            // The reading was fine, so the trouble lies elsewhere. Saying so is
            // useful: it rules out the ears and points at the capability.
            return unknown("the reading was already correct");
        }
        if line.eq_ignore_ascii_case("READING") {
            let steps: Vec<Concept> = reply
                .lines()
                .skip_while(|l| !l.trim().eq_ignore_ascii_case("READING"))
                .skip(1)
                .take_while(|l| !l.trim().starts_with("RULE "))
                .filter_map(|l| parse(l.trim(), &self.table).ok())
                // A step is a move, so it is always a compound. A bare atom
                // parses cleanly and means nothing, which is how a stray
                // protocol word ends up as the answer: a reply of "READING"
                // followed by "CORRECT" parsed the second line into a concept
                // named CORRECT, and Spoon reported that as the result of
                // "31 to the power of 2".
                .filter(|c: &Concept| c.is_compound())
                .collect();
            if steps.is_empty() {
                return unknown("a reading with no usable steps");
            }
            let lesson = reply
                .lines()
                .find_map(|l| l.trim().strip_prefix("RULE "))
                .map(|r| Arc::from(r.trim()));
            return TeacherReply::Reading { steps, lesson };
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
    ) -> Result<Taught, LlmError> {
        let messages = [
            Message::system(Self::prompt(ask, vocabulary)),
            Message::user(self.describe(ask)),
        ];
        let (raw, exchange) = self
            .client
            .chat_with_exchange(Seat::Teacher, &messages)
            .await?;
        Ok(Taught {
            reply: self.parse_reply(&raw, ask),
            exchange: Some(exchange),
        })
    }
}
