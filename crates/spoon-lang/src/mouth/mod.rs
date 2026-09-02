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
use spoon_core::types::response::ResponsePlan;

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
            Some(cfg) => render::render_detail(&self.client, cfg, plan, ctx).await,
        }
    }

    /// Offline realization: templates only, no async.
    pub fn say_offline(&self, plan: &ResponsePlan) -> String {
        templates::realize(plan)
    }
}
