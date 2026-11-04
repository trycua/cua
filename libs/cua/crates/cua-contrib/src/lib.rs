//! Contrib sandbox providers for the cua SDK.
//!
//! Each provider implements [`cua_sandbox_core::Provider`] for one
//! third-party platform and is compiled only with its cargo feature
//! (`e2b`, `daytona`, `modal`; `all` for every one). The core distribution
//! builds none of them; the CLI and SDK enable them with `--features
//! contrib` (or `contrib-<name>`), and [`providers`] is what a runtime
//! registers.
//!
//! Every provider takes the image the core resolved and pinned (any OCI
//! registry image, including the canonical `ghcr.io/trycua/*` images) and
//! maps it onto the platform: a direct pull, or a template / snapshot build
//! cached by the image digest ([`common::template_key`]). Credentials come
//! from the provider's environment variable or `cua auth provider set
//! <name>` ([`common::CredentialStore`]) and are never logged.

pub mod common;
#[cfg(feature = "daytona")]
pub mod daytona;
#[cfg(feature = "e2b")]
pub mod e2b;
pub mod image_config;
#[cfg(feature = "modal")]
pub mod modal;
#[cfg(feature = "testing")]
pub mod testing;

use cua_sandbox_core::Provider;
use std::sync::Arc;

/// One row of [`catalog`]: a provider this crate knows, compiled in or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEntry {
    /// Location word (`--on <name>`).
    pub name: &'static str,
    /// The environment variables its credentials come from.
    pub credential_env: &'static [&'static str],
    /// Whether this build includes it.
    pub built: bool,
}

/// Every provider this crate implements, with whether this build has it.
pub fn catalog() -> Vec<ProviderEntry> {
    vec![
        ProviderEntry {
            name: "e2b",
            credential_env: E2B_ENV,
            built: cfg!(feature = "e2b"),
        },
        ProviderEntry {
            name: "daytona",
            credential_env: DAYTONA_ENV,
            built: cfg!(feature = "daytona"),
        },
        ProviderEntry {
            name: "modal",
            credential_env: MODAL_ENV,
            built: cfg!(feature = "modal"),
        },
    ]
}

/// E2B credential variables.
pub const E2B_ENV: &[&str] = &["E2B_API_KEY"];
/// Daytona credential variables.
pub const DAYTONA_ENV: &[&str] = &["DAYTONA_API_KEY"];
/// Modal credential variables (both are needed).
pub const MODAL_ENV: &[&str] = &["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"];

/// The providers this build includes, configured from the environment and
/// the credential store. Registering one never performs I/O; a provider
/// without credentials fails its first call with
/// `ContribNotConfigured`, naming the variable to set.
#[allow(unused_mut, clippy::vec_init_then_push)]
pub fn providers() -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<Arc<dyn Provider>> = Vec::new();
    #[cfg(feature = "e2b")]
    out.push(Arc::new(e2b::E2b::from_env()));
    #[cfg(feature = "daytona")]
    out.push(Arc::new(daytona::Daytona::from_env()));
    #[cfg(feature = "modal")]
    out.push(Arc::new(modal::Modal::from_env()));
    out
}
