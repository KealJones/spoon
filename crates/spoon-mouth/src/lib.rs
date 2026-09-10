//! Mouth: a structured result back into prose.
//!
//! The template path is the reference. The model makes it sound human, and
//! anything it produces is checked against the values it was given: a mouth
//! that drops or invents a number has not made the answer friendlier, it has
//! made it wrong.

use std::sync::Arc;

use spoon_concept::{Concept, ConceptId, Ground, SymbolTable, render};
use spoon_seat::{LlmClient, LlmError, Message, Mouth, MouthReply, Seat};

/// Deterministic rendering. Offline mode, and the fallback whenever the model
/// fails its check.
pub struct TemplateMouth {
    /// Shared with the brain rather than copied.
    ///
    /// Names are learned mid-conversation, and a mouth holding a snapshot taken
    /// at startup prints hex for every one of them. The whole reply becomes
    /// unreadable exactly when Spoon has just learned something.
    table: Arc<SymbolTable>,
}

/// Render a concept in paren notation for LLM-facing text.
///
/// Ground values (strings, numbers, booleans) render the same as in angle
/// notation. Named atoms and compounds use `head(arg, arg)` form so that any
/// concept expression appearing in a mouth prompt speaks the same language as
/// the ears and teacher.
fn render_pycall_concept(concept: &Concept, table: &SymbolTable) -> String {
    let mut out = String::new();
    write_pycall(&mut out, concept, table);
    out
}

fn write_pycall(out: &mut String, concept: &Concept, table: &SymbolTable) {
    match concept {
        Concept::Atomic(ConceptId::Named(symbol)) => match table.resolve(*symbol) {
            Some(name) => out.push_str(&name),
            None => out.push_str(&format!("#{:016x}", symbol.as_u64())),
        },
        Concept::Atomic(ConceptId::Ground(_)) => {
            out.push_str(&render(concept, table));
        }
        Concept::Hole(h) => out.push_str(&format!("?{}", h.as_u32())),
        Concept::Compound { head, args } => {
            if head.is_compound() {
                out.push('(');
                write_pycall(out, head, table);
                out.push(')');
            } else {
                write_pycall(out, head, table);
            }
            out.push('(');
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_pycall(out, arg, table);
            }
            out.push(')');
        }
    }
}

/// The heads the templates recognize.
///
/// Registered on construction because a symbol id is computed from its name,
/// so a store that has never been told the spelling can only print hex. These
/// are Spoon's own response vocabulary and it should never have to look them up.
const RESPONSE_HEADS: &[&str] = &[
    "noted",
    "answer",
    "partial",
    "unknown",
    "needs-permission",
    "error",
    "did-not-understand",
    "nothing-to-say",
    "cannot-yet",
    "greet",
    "acknowledge-thanks",
    "farewell",
    "chat",
    "list-of",
    "assert-that",
    "ask",
    "do",
    "correction",
];

impl TemplateMouth {
    pub fn new(table: Arc<SymbolTable>) -> Self {
        for name in RESPONSE_HEADS {
            table.intern(name);
        }
        TemplateMouth { table }
    }

    fn plain(&self, concept: &Concept) -> String {
        match concept.as_ground() {
            Some(Ground::Text(t)) => t.to_string(),
            Some(Ground::Int(i)) => i.to_string(),
            Some(Ground::Float(f)) => format!("{f}"),
            Some(Ground::Bool(b)) => if *b { "yes" } else { "no" }.to_string(),
            _ => render_pycall_concept(concept, &self.table),
        }
    }

    fn template(&self, response: &Concept) -> String {
        let head = response
            .head_symbol()
            .map(|h| self.table.display(h))
            .unwrap_or_default();
        let arg = response.arg(0);
        match (head.as_str(), arg) {
            ("noted", Some(c)) => format!("noted: {}", self.plain(c)),
            ("answer", Some(c)) => self.plain(c),
            ("partial", Some(c)) => {
                format!("{} (some of that I could not work out)", self.plain(c))
            }
            ("unknown", Some(c)) => format!("I do not know about {}", self.plain(c)),
            // Names what was wanted rather than echoing how it was written
            // down, because the notation is Spoon's business and the reader
            // asked a question.
            ("cannot-yet", Some(c)) => {
                let what = c.head_symbol().map(|h| self.table.display(h));
                match what {
                    Some(name) => format!("I cannot do {name} yet"),
                    None => "I cannot work that out yet".to_string(),
                }
            }
            ("needs-permission", Some(c)) => {
                format!("that needs your say-so first: {}", self.plain(c))
            }
            ("error", Some(c)) => format!("that went wrong: {}", self.plain(c)),
            ("did-not-understand", _) => "I did not follow that one".to_string(),
            ("nothing-to-say", _) => "ok".to_string(),
            ("greet", _) => "hey, what do you need?".to_string(),
            ("acknowledge-thanks", _) => "anytime".to_string(),
            ("farewell", _) => "see you".to_string(),
            _ => self.plain(response),
        }
    }
}

#[async_trait::async_trait]
impl Mouth for TemplateMouth {
    async fn say(
        &self,
        response: &Concept,
        must_mention: &[Concept],
    ) -> Result<MouthReply, LlmError> {
        Ok(MouthReply::native(self.say_native(response, must_mention)))
    }

    fn say_native(&self, response: &Concept, _must_mention: &[Concept]) -> String {
        self.template(response)
    }
}

/// Is this response just a value, with nothing around it to explain?
fn is_bare_value(response: &Concept) -> bool {
    response
        .head_symbol()
        .is_some_and(|h| h == spoon_concept::SymbolId::of("answer"))
        && response.arity() == 1
        && response.arg(0).is_some_and(|a| {
            // A ground scalar. A list or a JSON blob does read better with a
            // sentence around it, so those still go to the model.
            matches!(
                a.as_ground(),
                Some(Ground::Int(_) | Ground::Float(_) | Ground::Bool(_))
            ) || a
                .as_ground()
                .and_then(Ground::as_str)
                .is_some_and(|t| t.len() < 40)
        })
}

/// The model path, with the template underneath it.
pub struct ModelMouth {
    client: LlmClient,
    template: TemplateMouth,
}

impl ModelMouth {
    pub fn new(client: LlmClient, table: Arc<SymbolTable>) -> Self {
        ModelMouth {
            client,
            template: TemplateMouth::new(table),
        }
    }

    /// Does the rendering still carry everything it was required to say?
    ///
    /// Structural rather than semantic: it catches a dropped or altered value,
    /// which is the failure that actually matters, and does not pretend to
    /// catch drift in tone or emphasis.
    fn faithful(text: &str, must_mention: &[Concept], table: &SymbolTable) -> bool {
        must_mention.iter().all(|value| {
            let rendered = match value.as_ground() {
                Some(Ground::Text(t)) => t.to_string(),
                Some(Ground::Int(i)) => i.to_string(),
                Some(Ground::Float(f)) => format!("{f}"),
                Some(Ground::Bool(b)) => if *b { "yes" } else { "no" }.to_string(),
                _ => render_pycall_concept(value, table),
            };
            rendered.trim().is_empty() || text.to_lowercase().contains(&rendered.to_lowercase())
        })
    }
}

#[async_trait::async_trait]
impl Mouth for ModelMouth {
    async fn say(
        &self,
        response: &Concept,
        must_mention: &[Concept],
    ) -> Result<MouthReply, LlmError> {
        let baseline = self.template.say_native(response, must_mention);

        if is_bare_value(response) {
            return Ok(MouthReply::native(baseline));
        }
        let prompt = r#"Rewrite the given line as one short, casual sentence a person would actually say.

Rules:
- Keep every number, name and quoted string exactly as given. Do not round, reformat, or drop any.
- Add NOTHING. No trivia, no context, no commentary, no explanation of what anything is.
- Never mention "system message", "the line", or anything about this instruction.
- One sentence. Under 20 words. Lowercase is fine.
- If the line is already a fine sentence, repeat it unchanged.

Reply with the sentence only."#;
        let messages = [
            Message::system(prompt),
            Message::user(format!("System message: {baseline}")),
        ];
        let (rendered, exchange) = self
            .client
            .chat_with_exchange(Seat::Mouth, &messages)
            .await?;
        let rendered = rendered.trim().trim_matches('"').to_string();

        let padded = rendered.len() > baseline.len() * 3 + 40
            || rendered.to_lowercase().contains("system message");
        if rendered.is_empty()
            || padded
            || !Self::faithful(&rendered, must_mention, &self.template.table)
        {
            return Ok(MouthReply::native(baseline));
        }
        Ok(MouthReply {
            text: rendered,
            exchange: Some(exchange),
        })
    }

    fn say_native(&self, response: &Concept, must_mention: &[Concept]) -> String {
        self.template.say_native(response, must_mention)
    }
}
