//! Primitive registry. Submodules register actions and concepts into the Kernel.
//!
//! Order: concepts first so that action output types can reference them.

use super::Kernel;

pub mod concepts;
pub mod math;
pub mod value;
pub mod text;
pub mod list;
pub mod json;
pub mod time;
pub mod fs;
pub mod http;
pub mod shell;
pub mod mem;
pub mod dialog;
pub mod know;

pub fn register_all(k: &mut Kernel) {
    concepts::register(k);
    math::register(k);
    value::register(k);
    text::register(k);
    list::register(k);
    json::register(k);
    time::register(k);
    fs::register(k);
    http::register(k);
    shell::register(k);
    mem::register(k);
    dialog::register(k);
    know::register(k);
}
