//! The store, JSON, and the outside world.
//!
//! Everything here declares an effect above `Pure`, so the permission layer
//! decides whether it runs. The split is by what the natives touch:
//!
//! - [`store`]: belief and recall. Reads and writes the brain.
//! - [`json`]: parsing and picking apart foreign documents. Pure.
//! - [`time`]: the clock and arithmetic over it. Pure except `now`, which
//!   observes something outside the computation.

mod json;
mod store;
mod time;

use spoon_concept::{Concept, Ground};
use spoon_eval::{EvalError, NativeRegistry, type_error};

/// Build a `List<a, b, c>` concept.
///
/// Lists belong to `collections.rs`; this is the same shape spelled out here
/// so a store query returning many rows does not make this module depend on
/// that one for a single constructor.
pub(crate) fn list_of(items: Vec<Concept>) -> Concept {
    Concept::call("list", items)
}

/// Pull text out of a concept, or say precisely what was wrong.
pub(crate) fn want_text<'a>(native: &str, c: &'a Concept) -> Result<&'a str, EvalError> {
    c.as_ground()
        .and_then(Ground::as_str)
        .ok_or_else(|| type_error(native, "a text value", c))
}

/// Pull an integer out of a concept.
pub(crate) fn want_int(native: &str, c: &Concept) -> Result<i64, EvalError> {
    c.as_ground()
        .and_then(Ground::as_i64)
        .ok_or_else(|| type_error(native, "an integer", c))
}

pub fn register(registry: &mut NativeRegistry) {
    store::register(registry);
    json::register(registry);
    time::register(registry);
}
