//! The natives Spoon is born knowing.
//!
//! None of these is privileged. Each is an ordinary concept that happens to
//! ship with a `Native` realization, living in the same system as concepts
//! learned years later. A learned realization that performs better in context
//! is supposed to win, and nothing here prevents that.
//!
//! Registering a native only makes the code reachable. The concept and its
//! realization still have to exist in the store, which is what [`seed`] does.

pub mod arith;
pub mod collections;
pub mod data;
pub mod io;
pub mod text;

mod seed;

pub use seed::{SeedStats, seed_bootstrap};

use spoon_eval::NativeRegistry;

/// Every bootstrap native, assembled.
pub fn bootstrap() -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    arith::register(&mut registry);
    collections::register(&mut registry);
    text::register(&mut registry);
    data::register(&mut registry);
    io::register(&mut registry);
    registry
}
