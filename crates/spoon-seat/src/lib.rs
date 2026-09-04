//! The three places a language model is allowed to sit.
//!
//! Ears turn messy human language into concepts. Mouth turns a structured
//! response back into prose. Teacher fills a gap when Spoon meets something it
//! cannot do. Nothing else may call a model, and the interior in particular
//! never does: planning, choosing, deriving, and executing are ordinary code.
//!
//! Every seat is behind a trait with a working offline implementation, so
//! removing the model removes fluency and nothing else. If taking the LLM away
//! removes competence beyond parsing slang and phrasing replies, the design has
//! gone wrong.

mod client;
mod seats;

pub use client::{LlmClient, LlmConfig, LlmError, Message, Role, SeatCounters, Transport};
pub use seats::{
    Ears, Exchange, Heard, Mouth, MouthReply, Seat, Spec, Taught, Teacher, TeacherAsk,
    TeacherReply, Turn,
};
