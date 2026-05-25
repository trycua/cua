//! Windows tool implementations.
//!
//! On Windows, delegates to real win32/uia/input/capture implementations.
//! On other platforms, returns "not implemented" stubs so the crate still compiles.

use mcp_server::tool::ToolRegistry;

#[cfg(target_os = "windows")]
mod impl_;
#[cfg(target_os = "windows")]
pub(crate) mod page;
#[cfg(target_os = "windows")]
pub(crate) mod page_bookmark;

#[cfg(not(target_os = "windows"))]
mod stubs;

pub fn build_registry(compat: bool) -> ToolRegistry {
    #[cfg(target_os = "windows")]
    return impl_::build_registry(compat);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = compat;
        stubs::build_registry()
    }
}
