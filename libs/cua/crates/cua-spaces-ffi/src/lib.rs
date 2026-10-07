// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces app export (source-available, FSL-1.1-MIT): one UniFFI
//! library for the SwiftUI app that carries the MIT cua SDK (`cua_sdk`
//! namespace, unchanged) and, in the `cua_spaces_ffi` namespace, what only
//! the Spaces apps use:
//!
//! - the app core's view models and state machines
//!   ([`cua_spaces_app_core`], mirrored in [`app_core_types`]) and the
//!   [`KeyvaultClient`] both shells run;
//! - host-side teleport ([`Teleport`], from [`teleport(cua)`](teleport())):
//!   providers, manifests, sends, the app catalog, plans, runs and window
//!   drags.
//!
//! Loading the library registers the Cua Spaces extensions
//! ([`cua_spaces_ext`]) for every runtime the SDK builds in this process, so
//! an embedded `Cua` has teleport, the Cua Volume, persistent agents and the
//! Keyvault exactly like `cua daemon` in the Cua Spaces build.

mod app_core;
mod app_core_types;
pub mod media_decode;
mod teleport;
mod teleport_app;

pub use app_core::*;
pub use app_core_types::*;
pub use teleport::*;
pub use teleport_app::*;

uniffi::setup_scaffolding!("cua_spaces_ffi");

/// Registers the Cua Spaces extensions (teleport, the Cua Volume, persistent
/// agents, the Keyvault) for every runtime the SDK builds in this process.
/// Idempotent; the SwiftUI app calls it once at launch, before its first
/// `Cua`.
#[uniffi::export]
pub fn cua_spaces_register() {
    cua_spaces_ext::daemon::register();
}
