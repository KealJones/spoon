//! `spoon-exec` reuses [`spoon_core::SpoonError`] directly rather than defining a
//! parallel error type. Evaluation errors (type errors, undefined
//! variables/procedures, division by zero, budget exhaustion, etc.) are all
//! already modeled there since they're part of the shared vocabulary between
//! the core types and anything that executes them.

pub use spoon_core::SpoonError;
