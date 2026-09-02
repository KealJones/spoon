//! Host implementation for the kernel executor.
//! Since Store is not Sync, we use NoHost for execution and provide facts
//! through the store query path when needed.

use spoon_core::kernel::{Host, NoHost, PermissionMode};
use spoon_core::types::*;

/// A Host that provides `now_ms` and permission mode. Facts and recall are
/// handled by the brain querying the store directly (under the Mutex) rather
/// than through the Host trait, since Store is not Sync.
pub struct BrainHost {
    pub permission_mode: PermissionMode,
}

impl Host for BrainHost {
    fn now_ms(&self) -> i64 {
        now_ms()
    }

    fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }
}
