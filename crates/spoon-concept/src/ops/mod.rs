//! Structural operations over [`Concept`](crate::Concept).
//!
//! `concept.rs` defines what a concept *is*. This module defines what you can
//! *do* to one without knowing anything about the store, the evaluator, or the
//! rule engine. Everything above this layer (evaluation, rule application,
//! synthesis, consolidation) is written in terms of these primitives, so they
//! live here once instead of being re-derived in every crate.
//!
//! The module is split by concern rather than kept as one file:
//!
//! - [`traverse`]: visiting, addressing, and rebuilding at a position.
//! - [`subst`]: holes and what happens when you fill them.
//! - [`matching`]: comparing two terms structurally.
//! - [`rewrite`]: small builders that keep `Arc` sharing intact.
//!
//! # Arc discipline
//!
//! `Concept::clone` is O(1) by design. Every operation here that returns a new
//! term reuses the existing `Arc` for any subtree it did not change, so
//! rewriting one leaf of a large term allocates along one spine rather than
//! copying the tree. Code that rebuilds unconditionally defeats the whole
//! reason `Concept` holds `Arc` children.

pub mod matching;
pub mod rewrite;
pub mod subst;
pub mod traverse;

pub use matching::{alpha_equivalent, anti_unify, generalizes};
pub use rewrite::{flatten_spine, map_args, try_map_args};
pub use subst::{
    Bindings, hole_count, holes, is_ground_term, max_hole, rename_holes, substitute,
    substitute_positional,
};
pub use traverse::{
    Path, PathStep, PostOrder, PreOrder, at_path, find_first, positions, post_order, pre_order,
    replace_at, subterms, walk,
};
