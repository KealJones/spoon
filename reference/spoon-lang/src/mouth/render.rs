//! LLM surface realizer. Calls `Seat::Mouth` with a no-new-facts constraint,
//! runs a faithfulness check, retries once on violation, falls back to
//! `templates::realize` if the LLM can't stay faithful.

use spoon_core::llm::{ChatMessage, LlmClient, LlmConfig, Seat};
use spoon_core::types::response::ResponsePlan;

use super::{faithful, prompt, templates, MouthPath};

/// Context the realizer uses to ground the reply.
pub struct RenderContext {
    /// The raw user utterance for this turn.
    pub user_text: String,
    /// Up to a few (user, assistant) turn pairs for conversational grounding.
    pub prior_turns: Vec<(String, String)>,
}

/// Realize `plan` via the LLM. On faithfulness failure retries once; falls
/// back to templates on a second failure or any network error.
pub async fn render(
    client: &LlmClient,
    cfg: &LlmConfig,
    plan: &ResponsePlan,
    context: &RenderContext,
) -> String {
    render_detail(client, cfg, plan, context).await.0
}

/// Like `render` but also returns which path was taken.
pub(super) async fn render_detail(
    client: &LlmClient,
    cfg: &LlmConfig,
    plan: &ResponsePlan,
    context: &RenderContext,
) -> (String, MouthPath) {
    // Tighter per-attempt deadline for the surface realizer. A mouth that has
    // not replied in a few seconds is not going to help; fall back to templates.
    let mouth_cfg = LlmConfig { timeout_secs: cfg.mouth_timeout_secs, ..cfg.clone() };

    let system = prompt::build_system_prompt();
    let plan_json = serde_json::to_string(plan).unwrap_or_default();

    let mut msgs = vec![ChatMessage::system(&system)];
    for (u, a) in context.prior_turns.iter().take(3) {
        msgs.push(ChatMessage::user(u.as_str()));
        msgs.push(ChatMessage::assistant(a.as_str()));
    }
    msgs.push(ChatMessage::user(&format!(
        "Plan: {plan_json}\nUser message: {}",
        context.user_text
    )));

    // First attempt.
    // qwen3.5:4b uses ~300-1200 tokens of internal reasoning before producing
    // visible content, so 1500 gives enough headroom for thinking + reply.
    let first = match client.chat(Seat::Mouth, &mouth_cfg, msgs.clone(), false, Some(1500)).await {
        Err(_) => return (templates::realize(plan), MouthPath::Template),
        Ok(t) => t,
    };

    let v1 = faithful::check(plan, &first);
    if v1.is_empty() {
        return (first, MouthPath::Llm);
    }

    // Skip retry when any violation is non-retryable. ExtraNumber and ExtraName
    // indicate the model invented content that is not in the plan. A correction
    // prompt cannot un-hallucinate invented numbers or proper names; it will
    // typically produce different hallucinations on the next attempt.
    if v1.iter().any(|v| !is_retryable(&v.kind)) {
        return (templates::realize(plan), MouthPath::Template);
    }

    // Retry: append violation list and ask for a fix. Only reached for style
    // violations (missing mention, forbidden phrase, em-dash, too long) that
    // an explicit correction can plausibly repair.
    let vlist = v1
        .iter()
        .map(|v| format!("- {:?}: {}", v.kind, v.detail))
        .collect::<Vec<_>>()
        .join("\n");
    msgs.push(ChatMessage::assistant(&first));
    msgs.push(ChatMessage::user(&format!(
        "Your previous reply had issues - fix them and reply again.\n{vlist}"
    )));

    match client.chat(Seat::Mouth, &mouth_cfg, msgs, false, Some(1500)).await {
        Ok(second) => {
            if faithful::check(plan, &second).is_empty() {
                (second, MouthPath::LlmRetried)
            } else {
                (templates::realize(plan), MouthPath::Template)
            }
        }
        Err(_) => (templates::realize(plan), MouthPath::Template),
    }
}

/// Whether a violation kind is worth retrying.
/// Style violations (missing mention, forbidden phrase, em-dash, too long) can
/// be repaired by showing the LLM its error and asking again.
/// Hallucination violations (extra number, extra name) cannot: the model
/// invented content from training data, not from the plan, and prompting again
/// typically yields different invented content rather than fixing the root cause.
fn is_retryable(kind: &faithful::ViolationKind) -> bool {
    !matches!(kind, faithful::ViolationKind::ExtraNumber | faithful::ViolationKind::ExtraName)
}
