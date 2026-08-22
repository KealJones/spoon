//! SQLite-backed storage for the EKG knowledge graph: concepts,
//! relationships, and procedures, along with the metadata that makes
//! them trustworthy (confidence, contracts, lifecycle).

mod error;
mod schema;
mod store;

pub use error::{GraphError, Result};
pub use store::KnowledgeStore;
