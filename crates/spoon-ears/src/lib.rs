//! Ears: messy human language into concepts.
//!
//! Native first, model only on a miss. Every utterance the native path handles
//! is one the model did not have to, which is the weaning curve. The model's
//! output is not trusted either: it has to parse into concepts, so a
//! hallucination fails loudly rather than becoming a confident wrong answer.

mod model;
mod native;

pub use model::ModelEars;
pub use native::NativeEars;
