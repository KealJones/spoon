//! LLM prompt assembly for the ears normalizer seat.
//!
//! The LLM is only invoked when the native recognizer fails. It receives:
//!  - the system prompt from `data/prompts/normalizer_base.md`
//!  - a vocabulary block (known verbs/nouns that overlap the input)
//!  - up to 8 (utterance -> sce) few-shot pairs
//!  - the utterance to normalize
//!
//! `build_prompt` is pure and unit-testable without an LLM.

use spoon_core::llm::{ChatMessage, LlmClient, LlmConfig, Seat};

/// Maximum vocabulary words to include in the prompt.
const MAX_VOCAB: usize = 60;
/// Maximum few-shot examples.
const MAX_SHOTS: usize = 8;
/// Maximum chars for the system (base) message to keep total prompt under 10k chars.
const MAX_BASE_CHARS: usize = 6000;

/// Build the chat messages for the normalizer.
///
/// - `base`: contents of `data/prompts/normalizer_base.md`
/// - `vocab`: known words relevant to the utterance (capped at MAX_VOCAB)
/// - `shots`: (messy_utterance, sce) pairs (capped at MAX_SHOTS)
/// - `utterance`: the raw input to normalize
pub fn build_prompt(
    base: &str,
    vocab: &[String],
    shots: &[(String, String)],
    utterance: &str,
) -> Vec<ChatMessage> {
    let vocab_capped = &vocab[..vocab.len().min(MAX_VOCAB)];
    let shots_capped = &shots[..shots.len().min(MAX_SHOTS)];

    let vocab_block = if vocab_capped.is_empty() {
        "(none)".to_string()
    } else {
        vocab_capped.join(", ")
    };

    // Truncate the base prompt to stay under the per-prompt budget.
    // Prefer keeping the rules section (before EXAMPLES) and removing the verbose examples.
    let base_truncated = truncate_base(base, MAX_BASE_CHARS);

    // Fill the {{VOCABULARY}} and {{CONTEXT}} placeholders in the base prompt
    let system_text = base_truncated
        .replace("{{VOCABULARY}}", &vocab_block)
        .replace("{{CONTEXT}}", "(none)");

    let mut messages = vec![ChatMessage::system(system_text)];

    // Few-shot examples as alternating user/assistant pairs
    for (utt, sce) in shots_capped {
        messages.push(ChatMessage::user(utt.clone()));
        messages.push(ChatMessage::assistant(sce.clone()));
    }

    // The actual utterance to normalize
    messages.push(ChatMessage::user(utterance.to_string()));

    messages
}

/// Truncate `base` to at most `max_chars`, preferring to cut at the EXAMPLES section.
fn truncate_base(base: &str, max_chars: usize) -> String {
    if base.len() <= max_chars {
        return base.to_string();
    }
    // Try cutting at the examples section header
    if let Some(pos) = base.find("# EXAMPLES") {
        let before = &base[..pos];
        if before.len() <= max_chars {
            return format!("{}# [examples omitted for brevity]", before);
        }
        return format!("{}...", &before[..max_chars]);
    }
    // Just hard-truncate with note
    format!("{}...[truncated]", &base[..max_chars])
}

/// Build the repair messages for a failed parse.
/// Sends: original utterance, the failing SCE, the parser error, and a tight repair hint.
pub fn build_repair_prompt(
    utterance: &str,
    bad_sce: &str,
    parse_error: &str,
    vocab: &[String],
) -> Vec<ChatMessage> {
    let repair_system = "\
Fix ONE SCE parse error. Output only the corrected SCE text.\n\
Key rules:\n\
- Every command ends with `!` (Assistant, VERB PHRASE!)\n\
- No `if` inside an embedded question - use `Does X know the answer?`\n\
- Every noun needs a determiner (a/the/every/some/no/N)\n\
- Conditionals need `if ... then ...` with no comma before `then`\n\
- No `tell X that Y says/believes that` - split into two sentences\n\
- Phrasal verbs hyphenated: `looks-for`, `knocks-out`\n\
Fix ONLY what the error names. Output one line.";

    let vocab_block = if vocab.is_empty() {
        String::new()
    } else {
        format!("\nVocabulary: {}", vocab[..vocab.len().min(20)].join(", "))
    };

    let user_msg = format!(
        "Original utterance: {}\nFailing SCE: {}\nParser error: {}{}\nCorrected SCE:",
        utterance, bad_sce, parse_error, vocab_block
    );

    vec![
        ChatMessage::system(repair_system.to_string()),
        ChatMessage::user(user_msg),
    ]
}
/// Removes: markdown code fences, "Output:" / "Result:" prefixes, leading/trailing whitespace.
pub fn strip_non_sce(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Remove code fences
    if s.starts_with("```") {
        let end = s.find("```\n").or_else(|| s.rfind("```"));
        if let Some(pos) = end {
            s = s[3..pos].trim().to_string(); // between fences
        }
    }

    // Remove "Output:", "Result:", "SCE:" prefixes
    for prefix in &["Output:", "Result:", "SCE:", "output:", "result:", "sce:"] {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].trim().to_string();
        }
    }

    // Remove "Okay," "Sure," etc.
    for prefix in &["Okay,", "Sure,", "Alright,", "Here is", "Here's"] {
        if s.starts_with(prefix) {
            // Drop the first sentence
            if let Some(pos) = s.find(|c| c == '.' || c == '!' || c == '?') {
                s = s[pos + 1..].trim().to_string();
            }
        }
    }

    s.trim().to_string()
}

/// Call the LLM normalizer seat. Returns the raw string output.
/// Increments `Seat::Ears` counter on the client.
pub async fn call_llm_normalizer(
    client: &LlmClient,
    cfg: &LlmConfig,
    messages: Vec<ChatMessage>,
) -> anyhow::Result<String> {
    client.chat(Seat::Ears, cfg, messages, false, Some(200)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_under_budget() {
        let base = include_str!("../../../../data/prompts/normalizer_base.md");
        let vocab: Vec<String> = (0..60).map(|i| format!("word{}", i)).collect();
        let shots: Vec<(String, String)> = (0..8)
            .map(|i| (format!("utterance {}", i), format!("User does something {}.", i)))
            .collect();
        let utterance = "what is 7 times 9";
        let msgs = build_prompt(base, &vocab, &shots, utterance);
        let total_chars: usize = msgs.iter().map(|m| m.content.len()).sum();
        assert!(
            total_chars < 10_000,
            "prompt too large: {} chars",
            total_chars
        );
    }

    #[test]
    fn strip_non_sce_removes_prefix() {
        assert_eq!(strip_non_sce("Output: User greets Assistant."), "User greets Assistant.");
        assert_eq!(strip_non_sce("  User greets Assistant.  "), "User greets Assistant.");
    }
}
