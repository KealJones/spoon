//! The turn loop.
//!
//! Ears, then the interior, then the mouth, then the episode. The interior is
//! ordinary code from end to end: nothing between hearing and speaking consults
//! a model, and `TurnMetrics::interior_model_calls` is asserted to be zero.

mod brain;
mod correct;
mod credit;
mod episode;
mod reconcile;
mod resolve;

pub use brain::{Brain, BrainConfig, Seats, TurnResult};
pub use correct::{Correction, apply as apply_correction, is_correction};
pub use credit::{Blame, Stage, assign as assign_blame, assign_all};
pub use episode::{EarsPath, Episode, MouthPath, TraceStep, TurnMetrics};
pub use reconcile::{Reconciliation, reconcile, remember};
pub use resolve::{Move, resolve};
