// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Your own cloud account as a sandbox location: `--on aws`, `gcp`,
//! `modal` for sandboxes (the SDK, `cua sb`, cua-bench) and, on top, for
//! Spaces.
//!
//! | Provider | How a sandbox runs | Credentials |
//! |---|---|---|
//! | `aws` (EC2) | one small VM per sandbox; Docker runs the image | the AWS CLI's profiles, SSO, assume-role |
//! | `gcp` (Compute Engine) | the same | the gcloud CLI's sign-in |
//! | `modal` (Sandboxes) | one Modal sandbox (gVisor, or Modal's VM runtime) | `~/.modal.toml` profiles |
//!
//! Every cloud sandbox's cua-spacesd dials out to the cua.ai relay as a
//! machine of the signed-in account, so it needs no inbound port; the SDK
//! reaches it there ([`relay`]). Everything Cua creates is tagged
//! ([`model::tags`]) and recorded ([`Store`]) before it exists; delete,
//! stop, start and [`sweep`] act only on resources the cloud shows with
//! this home's tags.
//!
//! - [`CloudApi`]: what one cloud implements (the `aws`, `gcp`, `modal`
//!   modules).
//! - [`CloudSandboxes`]: a cloud as a sandbox-layer provider.
//! - [`Clouds`]: `cua cloud` (connect, test, status, disconnect, sweep).
//! - [`install`]: registers both on a `SandboxesBuilder`.

use std::path::Path;
use std::sync::Arc;

pub mod api;
#[cfg(feature = "aws")]
pub mod aws;
pub mod bootstrap;
#[cfg(feature = "gcp")]
pub mod gcp;
pub mod manager;
#[cfg(feature = "modal")]
pub mod modal;
pub mod model;
pub mod provider;
pub mod relay;
pub mod state;
pub mod sweep;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use api::{CloudApi, Result, Target, Tested};
pub use manager::Clouds;
pub use model::{Connection, ProvisionSpec, Resource, Tier};
pub use provider::CloudSandboxes;
pub use relay::{Join, RelayAccess};
pub use state::Store;

/// Every cloud this build includes (its cargo features).
#[allow(clippy::vec_init_then_push)]
pub fn builtin_apis() -> Vec<Arc<dyn CloudApi>> {
    #[allow(unused_mut)]
    let mut v: Vec<Arc<dyn CloudApi>> = Vec::new();
    #[cfg(feature = "aws")]
    v.push(Arc::new(aws::Aws::new()));
    #[cfg(feature = "modal")]
    v.push(Arc::new(modal::ModalApi::from_env()));
    #[cfg(feature = "gcp")]
    v.push(Arc::new(gcp::Gcp::new()));
    v
}

/// The clouds of `home` over `apis`, joining sandboxes to `relay`: the
/// providers to register and the manager behind `cua cloud`.
pub fn clouds(
    home: &Path,
    apis: Vec<Arc<dyn CloudApi>>,
    relay: RelayAccess,
) -> (Vec<Arc<dyn cua_sandbox_core::Provider>>, Arc<Clouds>) {
    let store = Arc::new(Store::new(home));
    let providers = apis
        .iter()
        .map(|a| {
            Arc::new(CloudSandboxes::new(a.clone(), store.clone(), relay.clone()))
                as Arc<dyn cua_sandbox_core::Provider>
        })
        .collect();
    (
        providers,
        Arc::new(Clouds::new(home, apis, store).with_relay(relay)),
    )
}

/// Registers this build's clouds on `builder` (a cloud provider replaces a
/// contrib one of the same name), with the state of `home` and the relay
/// of the environment and the `cua auth login` session.
pub fn install(
    builder: cua_sandbox_core::SandboxesBuilder,
    home: &Path,
) -> cua_sandbox_core::SandboxesBuilder {
    install_with(builder, home, builtin_apis(), RelayAccess::from_env())
}

/// [`install`] with explicit clouds and relay (tests, fixtures).
pub fn install_with(
    mut builder: cua_sandbox_core::SandboxesBuilder,
    home: &Path,
    apis: Vec<Arc<dyn CloudApi>>,
    relay: RelayAccess,
) -> cua_sandbox_core::SandboxesBuilder {
    // Clouds with a choice of engine validate `--runtime` in the placement
    // model like any location.
    for a in &apis {
        let runtimes = a.runtimes();
        if !runtimes.is_empty() {
            let _ = cua_sandbox_core::placement::register_provider(
                cua_sandbox_core::placement::Capabilities {
                    name: a.name().into(),
                    description: format!("your {} account", a.title()),
                    kinds: vec![cua_sandbox_core::placement::KindSupport {
                        kind: cua_sandbox_core::placement::Kind::Container,
                        runtimes,
                    }],
                },
            );
        }
    }
    let (providers, manager) = clouds(home, apis, relay);
    for p in providers {
        builder = builder.provider(p);
    }
    builder.clouds(manager)
}
