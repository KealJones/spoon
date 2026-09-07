//! Mouth: turns a `ResponsePlan` into English.
//!
//! Two paths:
//! - `templates` - deterministic, offline-safe, no LLM.
//! - `render`    - LLM surface realizer (Seat::Mouth), falls back to templates.

pub mod faithful;
pub mod prompt;
pub mod render;
pub mod templates;

pub use render::RenderContext;

use spoon_core::llm::{LlmClient, LlmConfig};
use spoon_core::types::response::{Move, ResponsePlan};

/// Which realization path was taken on a given turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouthPath {
    Llm,
    LlmRetried,
    Template,
}

/// The mouth component. Hold one per Spoon instance.
pub struct Mouth {
    /// If `None`, always uses templates (offline mode).
    pub cfg: Option<LlmConfig>,
    pub client: LlmClient,
}

impl Mouth {
    /// Realize `plan` with the best available path and report which one was used.
    pub async fn say(&self, plan: &ResponsePlan, ctx: &RenderContext) -> (String, MouthPath) {
        match &self.cfg {
            None => (templates::realize(plan), MouthPath::Template),
            Some(cfg) => {
                if should_skip_llm(plan) {
                    return (templates::realize(plan), MouthPath::Template);
                }
                render::render_detail(&self.client, cfg, plan, ctx).await
            }
        }
    }

    /// Offline realization: templates only, no async.
    pub fn say_offline(&self, plan: &ResponsePlan) -> String {
        templates::realize(plan)
    }
}

/// True when the plan is a pure data-report and the LLM cannot plausibly
/// improve on the template output.
///
/// The faithfulness guard allows no new facts, numbers, or names beyond what
/// the plan contains. For data moves the template already expresses all of
/// that content correctly, so calling the LLM only adds latency and a chance
/// of a faithfulness failure.
///
/// Skips LLM when ALL moves are:
///   Answer, YesNo, Result - structured values with fully determined content
///   Learned, CannotDo, Error, Refuse - state reports
///   Elicit, AskPermission, AskExamples, UnknownWord - structured dialog prompts
///
/// Keeps LLM for any social or discursive move:
///   Greet, Farewell, Thanks, Ack, Empathize, Reflect - social register
///   Opinion, Advise, Recall, Explain, Info, Clarify - stance or discourse
///
/// Conservative: a plan with ANY social move keeps the LLM path. A wrong skip
/// degrades reply quality, which is worse than a slow reply.
pub fn should_skip_llm(plan: &ResponsePlan) -> bool {
    !plan.moves.is_empty() && plan.moves.iter().all(move_is_pure_data)
}

fn move_is_pure_data(m: &Move) -> bool {
    matches!(
        m,
        Move::Answer { .. }
            | Move::YesNo { .. }
            | Move::Result { .. }
            | Move::Learned { .. }
            | Move::CannotDo { .. }
            | Move::Error { .. }
            | Move::Refuse { .. }
            | Move::Elicit { .. }
            | Move::AskPermission { .. }
            | Move::AskExamples { .. }
            | Move::UnknownWord { .. }
    )
}
