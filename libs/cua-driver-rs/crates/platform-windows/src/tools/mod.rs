//! Windows tool implementations.
//!
//! On Windows, delegates to real win32/uia/input/capture implementations.
//! On other platforms, returns "not implemented" stubs so the crate still compiles.

use mcp_server::tool::ToolRegistry;

#[cfg(target_os = "windows")]
mod impl_;

#[cfg(not(target_os = "windows"))]
mod stubs;

pub fn build_registry() -> ToolRegistry {
    #[cfg(target_os = "windows")]
    return impl_::build_registry();

    #[cfg(not(target_os = "windows"))]
    stubs::build_registry()
}
