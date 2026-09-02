//! Primitive registry. TO BE IMPLEMENTED by the kernel subagent.
//!
//! Each submodule exposes `pub fn register(k: &mut Kernel)`. Ids are
//! namespaced: `math.add`, `text.upper`, `list.map`, `json.get`, `time.now`,
//! `fs.read`, `http.get`, `shell.run`, `mem.recall`, `dialog.greet`, ...

use super::Kernel;

pub fn register_all(k: &mut Kernel) {
    let _ = k;
}
