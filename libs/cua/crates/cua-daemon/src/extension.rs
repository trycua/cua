//! Extension points for what the Cua Spaces build of the daemon adds: the
//! Keyvault broker, the Cua Volume and teleport (their implementations are
//! source-available, FSL-1.1-MIT, in `cua-spaces-ext`).
//!
//! A [`DaemonExtension`] runs at two moments of [`crate::Runtime::new`]:
//! [`DaemonExtension::configure`] before the Spaces runtime is built (to
//! register [`cua_spaces::extension::SpacesExtension`]s and seams on the
//! builder), and [`DaemonExtension::attach`] once it is (to start what needs
//! it, such as a broker). The attached part lives as long as the runtime and
//! may serve the teleport seam and daemon background tasks.
//!
//! Extensions come from [`crate::RuntimeConfig::extensions`] and from the
//! process-wide [`register`] (a host that embeds the SDK registers once;
//! every runtime it builds then carries them). This crate never names an
//! implementation, so an MIT build has none and the tools they serve report
//! that they ship with Cua Spaces.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cua_fleet::FleetClient;
use cua_spaces::SpacesBuilder;

/// What an extension sees of the runtime being built.
pub struct ExtensionContext<'a> {
    /// The cua home the Spaces state lives in (`RuntimeConfig::spaces_home`,
    /// else the default cua home).
    pub home: &'a Path,
    /// Whether `home` was set explicitly (tests and fixtures) rather than
    /// defaulted.
    pub home_explicit: bool,
    /// The Fleet client, when configured.
    pub fleet: Option<&'a FleetClient>,
    /// A fixture home standing in for the host's app data
    /// (`RuntimeConfig::teleport_home`): nothing may read the real machine.
    pub teleport_home: Option<&'a Path>,
}

/// A capability added to the daemon runtime. See the module docs.
pub trait DaemonExtension: Send + Sync + 'static {
    /// A short name for logs.
    fn name(&self) -> &str;

    /// Before the Spaces runtime is built: register Spaces extensions and
    /// seams on `builder`.
    fn configure(&self, builder: SpacesBuilder, cx: &ExtensionContext<'_>) -> SpacesBuilder {
        let _ = cx;
        builder
    }

    /// Once the Spaces runtime is built: start what needs it. The returned
    /// part is kept for the runtime's lifetime.
    fn attach(
        &self,
        spaces: &cua_spaces::Spaces,
        cx: &ExtensionContext<'_>,
    ) -> Option<Arc<dyn AttachedExtension>> {
        let _ = (spaces, cx);
        None
    }
}

/// The live part of a [`DaemonExtension`], held by the runtime.
pub trait AttachedExtension: Send + Sync + 'static {
    /// The Keyvault seam the Spaces `teleport_app` tool delivers through,
    /// when this extension hosts one.
    fn session_broker(&self) -> Option<Arc<dyn cua_spaces::teleport_broker::SessionBroker>> {
        None
    }

    /// Background tasks of `cua daemon` (not of an embedded runtime):
    /// supervisors, extra sockets beside `cua.sock` in `socket_dir`.
    #[cfg(feature = "server")]
    fn daemon_tasks(
        &self,
        runtime: &crate::Runtime,
        socket_dir: Option<&Path>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let _ = (runtime, socket_dir);
        Vec::new()
    }

    /// Runs when `cua daemon` stops, before its tasks are aborted (bounded
    /// by the daemon): what must not outlive the daemon (a mount it serves,
    /// writes not yet stored) ends here.
    #[cfg(feature = "server")]
    fn shutdown<'a>(
        &'a self,
        runtime: &'a crate::Runtime,
    ) -> cua_spaces::extension::BoxFuture<'a, ()> {
        let _ = runtime;
        Box::pin(async {})
    }

    /// For [`crate::Runtime::attached`] (downcasting to the concrete type).
    fn as_any(&self) -> &dyn Any;
}

/// How a daemon reports each extension in `GetInfoResponse.features`
/// (`extension:<name>`).
pub const FEATURE_PREFIX: &str = "extension:";

static REGISTERED: Mutex<Vec<Arc<dyn DaemonExtension>>> = Mutex::new(Vec::new());

/// Registers `extension` for every runtime this process builds from now on
/// (in addition to [`crate::RuntimeConfig::extensions`]). Registering a
/// second extension with the same [`DaemonExtension::name`] keeps the first.
pub fn register(extension: Arc<dyn DaemonExtension>) {
    let mut all = REGISTERED.lock().expect("daemon extensions");
    if !all.iter().any(|e| e.name() == extension.name()) {
        all.push(extension);
    }
}

/// The process-wide extensions ([`register`]).
pub fn registered() -> Vec<Arc<dyn DaemonExtension>> {
    REGISTERED.lock().expect("daemon extensions").clone()
}

pub(crate) fn home_of(spaces_home: Option<&PathBuf>) -> (PathBuf, bool) {
    match spaces_home {
        Some(h) => (h.clone(), true),
        None => (cua_home::cua_home(), false),
    }
}
