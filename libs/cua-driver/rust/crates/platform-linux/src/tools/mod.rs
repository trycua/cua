//! Linux tool implementations.
//!
//! On Linux: delegates to real x11/atspi/input/capture implementations.
//! On other platforms: returns "not implemented" stubs so the crate compiles.

use cua_driver_core::tool::ToolRegistry;

#[cfg(target_os = "linux")]
mod impl_;
#[cfg(target_os = "linux")]
pub(crate) mod page;

#[cfg(not(target_os = "linux"))]
mod stubs;

pub fn build_registry(compat: bool) -> ToolRegistry {
    build_registry_with_provider(compat, None)
}

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    #[cfg(target_os = "linux")]
    #[allow(clippy::needless_return)] // a cfg-gated early return, not a tail
    {
        // Visibly targeted pointer actions push cursor-hook events (see
        // `impl_::emit_cursor_hook`), so an embedder such as cua-spacesd can
        // publish the agent's cursor as presence without polling.
        cua_driver_core::cursor_hook::declare_cursor_hook_emitter();
        return impl_::build_registry_with_provider(compat, provider);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = compat;
        let _ = provider;
        stubs::build_registry()
    }
}

// Keep register_all as alias for backwards compat.
pub fn register_all() -> ToolRegistry {
    build_registry(false)
}
