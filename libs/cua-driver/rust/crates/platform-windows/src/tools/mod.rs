//! Windows tool implementations.
//!
//! On Windows, delegates to real win32/uia/input/capture implementations.
//! On other platforms, returns "not implemented" stubs so the crate still compiles.

use cua_driver_core::tool::ToolRegistry;

#[cfg(target_os = "windows")]
mod impl_;
#[cfg(target_os = "windows")]
pub(crate) mod page;
#[cfg(target_os = "windows")]
pub(crate) mod page_bookmark;
// Pure title parsing for `page_bookmark`; ungated so its tests run on any host.
pub(crate) mod page_title_marker;

#[cfg(not(target_os = "windows"))]
mod stubs;

pub fn build_registry(compat: bool) -> ToolRegistry {
    build_registry_with_provider(compat, None)
}

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    #[cfg(target_os = "windows")]
    return impl_::build_registry_with_provider(compat, provider);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = compat;
        let _ = provider;
        stubs::build_registry()
    }
}
