//! Linux tool implementations.
//!
//! On Linux: delegates to real x11/atspi/input/capture implementations.
//! On other platforms: returns "not implemented" stubs so the crate compiles.

use mcp_server::tool::ToolRegistry;

#[cfg(target_os = "linux")]
mod impl_;

#[cfg(not(target_os = "linux"))]
mod stubs;

pub fn build_registry() -> ToolRegistry {
    #[cfg(target_os = "linux")]
    return impl_::build_registry();

    #[cfg(not(target_os = "linux"))]
    stubs::build_registry()
}

// Keep register_all as alias for backwards compat.
pub fn register_all() -> ToolRegistry { build_registry() }
