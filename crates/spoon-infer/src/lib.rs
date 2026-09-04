//! Derivation: answering questions from facts that were never written down in
//! the shape they were asked.
//!
//! This is not a second engine. A rule is an ordinary concept with a `Rule`
//! realization, and `Symmetric` is an ordinary concept whose realization
//! happens to be one. What lives here is the machinery derivation needs and
//! rewriting does not: two-way unification with an occurs check, an index so
//! rule lookup does not scan, and backward chaining with cycle handling.

mod error;
mod rule;
mod unify;

pub use error::{InferError, Result};
pub use rule::{RuleIndex, ScanIndex, StoredRule, load_rules};
pub use unify::{Substitution, unify, unify_terms};
