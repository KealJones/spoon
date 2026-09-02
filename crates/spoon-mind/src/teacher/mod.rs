//! Teacher: the third LLM seat. Writes specs, phrasings, concept models,
//! stances, and curricula. Never writes executable bodies; the synthesizer
//! does that.
//!
//! brain.rs is the only production caller. Each method calls the LLM once,
//! parses the result, and retries once on failure before returning Err.

mod curriculum;
mod prompts;
mod spec;
mod stance;

pub use curriculum::{Lesson, parse_lessons_json};
pub use spec::{parse_concept_json, parse_pairs_json, parse_spec_json, parse_type};
pub use stance::{TaughtStance, parse_stance_json};

use spoon_core::{ChatMessage, Concept, LlmClient, LlmConfig, Pair, Seat, Spec};

// ---------------------------------------------------------------------------
// Public struct
// ---------------------------------------------------------------------------

pub struct Teacher {
    pub client: LlmClient,
    pub cfg: LlmConfig,
}

impl Teacher {
    pub fn new(client: LlmClient, cfg: LlmConfig) -> Teacher {
        Teacher { client, cfg }
    }

    // -- Public API ----------------------------------------------------------

    /// Request a spec for a capability Spoon lacks.
    ///
    /// `capability` is a short name or description; `context` is the user
    /// utterance or failed goal that prompted this request.
    pub async fn spec_for(
        &self,
        capability: &str,
        context: &str,
        known_types: &[String],
    ) -> anyhow::Result<Spec> {
        let spec_id = format!(
            "teacher:{}:{}",
            capability.replace(' ', "_"),
            chrono::Utc::now().timestamp_millis()
        );
        let sys = prompts::spec_prompt(capability, context, known_types);
        let msgs = vec![ChatMessage::system(sys)];
        let id = spec_id.clone();
        self.chat_with_retry(msgs, move |raw| parse_spec_json(raw, &id))
            .await
    }

    /// Generate messy utterances that should route to `sce`.
    pub async fn phrasings_for(
        &self,
        sce: &str,
        verb: &str,
        n: usize,
    ) -> anyhow::Result<Vec<Pair>> {
        let sce_owned = sce.to_string();
        let sys = prompts::phrasings_prompt(sce, verb, n);
        let msgs = vec![ChatMessage::system(sys)];
        self.chat_with_retry(msgs, move |raw| parse_pairs_json(raw, &sce_owned))
            .await
    }

    /// Propose a concept model for an unknown noun.
    pub async fn concept_for(
        &self,
        noun: &str,
        context: &str,
        known_concepts: &[String],
    ) -> anyhow::Result<Concept> {
        let sys = prompts::concept_prompt(noun, context, known_concepts);
        let msgs = vec![ChatMessage::system(sys)];
        self.chat_with_retry(msgs, |raw| parse_concept_json(raw)).await
    }

    /// Produce an opinion Spoon can hold and defend on a human topic.
    pub async fn stance_for(
        &self,
        topic: &str,
        context: &str,
    ) -> anyhow::Result<TaughtStance> {
        let sys = prompts::stance_prompt(topic, context);
        let msgs = vec![ChatMessage::system(sys)];
        self.chat_with_retry(msgs, |raw| parse_stance_json(raw)).await
    }

    /// Generate a pretrain curriculum of `n` lessons.
    pub async fn curriculum(
        &self,
        n: usize,
        existing_verbs: &[String],
        themes: &[String],
    ) -> anyhow::Result<Vec<Lesson>> {
        let sys = prompts::curriculum_prompt(n, existing_verbs, themes);
        let msgs = vec![ChatMessage::system(sys)];
        self.chat_with_retry(msgs, |raw| parse_lessons_json(raw)).await
    }

    // -- Private helpers -----------------------------------------------------

    /// Call the teacher seat once; on parse/validation failure retry once,
    /// appending the error so the LLM can self-correct.
    async fn chat_with_retry<T, F>(
        &self,
        msgs: Vec<ChatMessage>,
        parse: F,
    ) -> anyhow::Result<T>
    where
        F: Fn(&str) -> Result<T, String>,
    {
        let raw = self
            .client
            .chat(Seat::Teacher, &self.cfg, msgs.clone(), true, Some(1200))
            .await?;
        match parse(&raw) {
            Ok(v) => Ok(v),
            Err(e) => {
                let mut retry = msgs;
                retry.push(ChatMessage::assistant(raw));
                retry.push(ChatMessage::user(format!(
                    "Your previous answer was rejected: {e}. Return corrected JSON only."
                )));
                let raw2 = self
                    .client
                    .chat(Seat::Teacher, &self.cfg, retry, true, Some(1200))
                    .await?;
                parse(&raw2)
                    .map_err(|e2| anyhow::anyhow!("teacher retry failed: {e2}"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared parsing utilities (used by submodules via `super::`)
// ---------------------------------------------------------------------------

/// Strip markdown code fences from LLM output before JSON parsing.
///
/// Handles both ` ```json ... ``` ` and ` ``` ... ``` ` fences.
pub fn strip_fences(s: &str) -> &str {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```json") {
        let rest = rest.trim_start_matches(['\n', '\r']);
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
    }
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.trim_start_matches(['\n', '\r']);
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
    }
    s
}

/// Reject any JSON that contains code-like keys or values.
///
/// Forbidden object keys: "code", "program", "body", "impl".
/// Forbidden value patterns: "fn ", "=>", "def " in any string; "{" in
/// strings longer than 40 chars.
pub fn check_no_code(v: &serde_json::Value) -> Result<(), String> {
    match v {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if matches!(key.as_str(), "code" | "program" | "body" | "impl") {
                    return Err("teacher must not write code".into());
                }
                check_no_code(val)?;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                check_no_code(item)?;
            }
        }
        serde_json::Value::String(s) => {
            if s.contains("fn ") || s.contains("=>") || s.contains("def ") {
                return Err("teacher must not write code".into());
            }
            if s.len() > 40 && s.contains('{') {
                return Err("teacher must not write code".into());
            }
        }
        _ => {}
    }
    Ok(())
}
