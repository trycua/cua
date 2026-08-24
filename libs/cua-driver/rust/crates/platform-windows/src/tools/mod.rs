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

#[cfg(not(target_os = "windows"))]
mod stubs;

pub fn build_registry(compat: bool) -> ToolRegistry {
    build_registry_with_provider(compat, None)
}

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    build_registry_with_provider_and_secret_broker(compat, provider, None)
}

pub fn build_registry_with_provider_and_secret_broker(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
    secret_broker: Option<std::sync::Arc<cua_driver_core::credentials::SecretBroker>>,
) -> ToolRegistry {
    #[cfg(target_os = "windows")]
    return impl_::build_registry_with_provider_and_secret_broker(compat, provider, secret_broker);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = compat;
        let _ = provider;
        let _ = secret_broker;
        stubs::build_registry()
    }
}
