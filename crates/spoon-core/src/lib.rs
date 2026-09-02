//! spoon-core: shared types, the CAN index, the kernel (IR evaluator +
//! primitives), SQLite store, and the single LLM client.
//!
//! Dependency direction: types <- can <- kernel <- store. Nothing here knows
//! about language, planning, or dialog.

pub mod can;
pub mod kernel;
pub mod llm;
pub mod store;
pub mod types;

pub use can::Can;
pub use kernel::{Budget, Ctx, EvalError, Host, Kernel, NoHost, PermissionMode, Sandbox};
pub use llm::{ChatMessage, LlmClient, LlmConfig, Seat};
pub use store::{Seed, Stance, Store, StoreCounts};
pub use types::*;
