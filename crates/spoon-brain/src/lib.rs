//! The turn loop.
//!
//! Ears, then the interior, then the mouth, then the episode. The interior is
//! ordinary code from end to end: nothing between hearing and speaking consults
//! a model, and `TurnMetrics::interior_model_calls` is asserted to be zero.

mod brain;
mod episode;
mod resolve;

pub use brain::{Brain, BrainConfig, Seats, TurnResult};
pub use episode::{EarsPath, Episode, MouthPath, TurnMetrics};
pub use resolve::{Move, resolve};
