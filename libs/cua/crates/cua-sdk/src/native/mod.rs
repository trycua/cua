//! Native (non-wasm) implementation.

mod agent_setup;
mod agents;
mod auth;
#[cfg(feature = "spaces")]
mod cloud;
mod config;
mod cua;
mod fleet;
#[cfg(feature = "host")]
mod host;
mod image;
#[cfg(feature = "local")]
mod local;
mod mcp;
#[cfg(feature = "media")]
mod media;
mod overlay;
#[cfg(feature = "spaces")]
mod persistent;
mod sandbox;
#[cfg(feature = "spaces")]
mod spaces;
mod spacesd;
pub(crate) mod telemetry;
#[cfg(feature = "spaces")]
mod teleport_types;

pub use agent_setup::*;
pub use agents::*;
pub use auth::*;
#[cfg(feature = "spaces")]
pub use cloud::*;
pub use config::*;
pub use cua::*;
pub use fleet::*;
#[cfg(feature = "host")]
pub use host::*;
pub use image::*;
#[cfg(feature = "local")]
pub use local::*;
pub use mcp::*;
#[cfg(feature = "media")]
pub use media::*;
pub use overlay::*;
#[cfg(feature = "spaces")]
pub use persistent::*;
pub use sandbox::*;
pub use spaces::*;
pub use spacesd::*;
pub use telemetry::*;
#[cfg(feature = "spaces")]
pub use teleport_types::*;

use crate::CuaError;
use std::{future::Future, sync::OnceLock};

/// The Tokio runtime every exported async call runs on. Foreign executors
/// only poll the join handle, so callers never need a runtime of their own
/// and background tasks (forwards, media pumps) outlive the call.
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 4))
            .unwrap_or(2);
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name("cua-sdk")
            .enable_all()
            .build()
            .expect("build the cua-sdk Tokio runtime")
    })
}

/// Runs `fut` on the SDK runtime and awaits it from any executor.
pub(crate) async fn run<T, F>(fut: F) -> crate::Result<T>
where
    T: Send + 'static,
    F: Future<Output = crate::Result<T>> + Send + 'static,
{
    runtime()
        .spawn(fut)
        .await
        .map_err(|e| CuaError::Internal(format!("SDK task failed: {e}")))?
}

impl From<cua_daemon::Error> for CuaError {
    fn from(e: cua_daemon::Error) -> Self {
        use cua_daemon::Error as E;
        let m = e.message().to_string();
        match e {
            E::InvalidArgument(_) => CuaError::InvalidArgument(m),
            E::InvalidPlacement { .. } => CuaError::InvalidPlacement(m),
            E::NotFound(_) => CuaError::NotFound(m),
            E::AmbiguousSandbox { .. } => CuaError::AmbiguousSandbox(m),
            E::ProviderNotConfigured(_) => CuaError::ProviderNotConfigured(m),
            E::Unsupported(_) => CuaError::Unsupported(m),
            E::SpacesdNotAvailable(_) => CuaError::SpacesdNotAvailable(m),
            E::Timeout(_) => CuaError::Timeout(m),
            E::Fleet(_) => CuaError::Fleet(m),
            E::FleetAdmissionDenied(_) => CuaError::FleetAdmissionDenied(m),
            E::CloudCreditExhausted(_) => CuaError::CloudCreditExhausted(m),
            E::Cloud(c) => CuaError::Cloud(c),
            E::Runtime(_) => CuaError::Runtime(m),
            E::Env(_) => CuaError::Env(m),
            E::Http(_) => CuaError::Http(m),
            E::Unauthenticated(_) => CuaError::Unauthenticated(m),
            E::Transport(_) => CuaError::Transport(m),
            E::DaemonNotRunning(_) => CuaError::DaemonNotRunning(m),
            E::CapabilityMissing(_) => CuaError::CapabilityMissing(m),
            E::HostCapabilityMissing(_) => CuaError::HostCapabilityMissing(m),
            E::TeleportRefused(_) => CuaError::TeleportRefused(m),
            E::PoolSpecMismatch(_) => CuaError::PoolSpecMismatch(m),
            E::ClaimSecretsNotDelivered(_) => CuaError::ClaimSecretsNotDelivered(m),
            E::InsufficientDisk(_) => CuaError::InsufficientDisk(m),
            E::Cancelled(_) => CuaError::Cancelled(m),
            E::Internal(_) => CuaError::Internal(m),
        }
    }
}

impl From<cua_spacesd_client::Error> for CuaError {
    fn from(e: cua_spacesd_client::Error) -> Self {
        use cua_spacesd_client::Error as E;
        let m = e.to_string();
        match e {
            E::InvalidEndpoint(_) => CuaError::InvalidArgument(m),
            E::Transport(_) => CuaError::Transport(m),
            E::SpacesdNotAvailable { .. } => CuaError::SpacesdNotAvailable(m),
            E::Unauthenticated(_) => CuaError::Unauthenticated(m),
            E::FeatureUnsupported { .. } => CuaError::Unsupported(m),
            E::PermissionDenied(_) => CuaError::PermissionDenied(m),
            E::ProcessNotFound(_) | E::PathNotFound(_) | E::SessionNotFound(_) => {
                CuaError::NotFound(m)
            }
            E::Timeout(_) | E::DesktopNotReady(_) => CuaError::Timeout(m),
            E::Io(_) => CuaError::Internal(m),
            _ => CuaError::Env(m),
        }
    }
}

impl From<cua_fleet::Error> for CuaError {
    fn from(e: cua_fleet::Error) -> Self {
        cua_daemon::Error::from(e).into()
    }
}

impl From<cua_sandbox_core::Error> for CuaError {
    fn from(e: cua_sandbox_core::Error) -> Self {
        cua_daemon::Error::from(e).into()
    }
}

impl From<std::io::Error> for CuaError {
    fn from(e: std::io::Error) -> Self {
        CuaError::Internal(e.to_string())
    }
}

impl From<tonic::Status> for CuaError {
    fn from(s: tonic::Status) -> Self {
        cua_spacesd_client::Error::from(s).into()
    }
}

pub(crate) fn ms(v: Option<u32>, default_ms: u32) -> std::time::Duration {
    std::time::Duration::from_millis(u64::from(v.unwrap_or(default_ms)))
}

pub(crate) fn millis(v: impl Into<u64>) -> std::time::Duration {
    std::time::Duration::from_millis(v.into())
}

pub(crate) fn secs(v: impl Into<u64>) -> std::time::Duration {
    std::time::Duration::from_secs(v.into())
}

/// Test helper: tests that change the process environment take turns.
#[cfg(test)]
pub(crate) mod test_env {
    /// A private `CUA_HOME` and no `CUA_DEFAULT_*` for one test; tests that
    /// touch the process environment take turns.
    pub(crate) struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        home: tempfile::TempDir,
        saved: Vec<(&'static str, Option<String>)>,
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const VARS: [&str; 4] = [
        "CUA_HOME",
        "CUA_DEFAULT_ON",
        "CUA_DEFAULT_KIND",
        "CUA_DEFAULT_RUNTIME",
    ];

    impl EnvGuard {
        pub(crate) fn isolated() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let home = tempfile::tempdir().unwrap();
            let saved = VARS.iter().map(|v| (*v, std::env::var(v).ok())).collect();
            // SAFETY: tests that change the environment hold ENV_LOCK.
            unsafe {
                for v in VARS {
                    std::env::remove_var(v);
                }
                std::env::set_var("CUA_HOME", home.path());
            }
            Self {
                _lock: lock,
                home,
                saved,
            }
        }

        pub(crate) fn set(&self, k: &str, v: &str) {
            let _ = &self.home;
            // SAFETY: see `isolated`.
            unsafe { std::env::set_var(k, v) }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: see `isolated`.
            unsafe {
                for (k, v) in &self.saved {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }
}
