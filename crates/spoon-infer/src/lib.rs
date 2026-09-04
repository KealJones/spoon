//! Derivation: answering questions from facts that were never written down in
//! the shape they were asked.
//!
//! This is not a second engine. A rule is an ordinary concept with a `Rule`
//! realization, and `Symmetric` is an ordinary concept whose realization
//! happens to be one. What lives here is the machinery derivation needs and
//! rewriting does not: two-way unification with an occurs check, an index so
//! rule lookup does not scan, and backward chaining with cycle handling.

mod derive;
mod error;
mod index;
mod meta;
mod rule;
mod unify;

pub use derive::{Derivation, DeriveBudget, Engine, Support};
pub use error::{InferError, Result};
pub use index::{DiscriminationTree, RetrievalStats};
pub use meta::{meta_rules, seed_meta_rules};
pub use rule::{RuleIndex, ScanIndex, StoredRule, load_rules};
pub use unify::{Substitution, unify, unify_terms};
