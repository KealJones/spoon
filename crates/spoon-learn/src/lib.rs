//! Turning experience into reusable ability.
//!
//! Synthesis builds a capability from worked examples. Consolidation notices
//! that Spoon keeps building the same shape and gives it a name. Both produce
//! ordinary stored concepts, so nothing downstream can tell the difference
//! between what Spoon was born with and what it worked out.

pub mod consolidate;
pub mod synth;

pub use consolidate::{
    Abstraction, ConsolidateConfig, apply_abstraction, consolidate, consolidate_store, evict_unused,
};
pub use synth::{
    SynthBudget, SynthError, SynthLimit, SynthOutcome, SynthStats, learn_from_spec, synthesize,
};
