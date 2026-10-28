// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Volume runtime behind [`crate::DriveExtension`]: the drive's
//! services (change feed, block cache, this machine's mount, storage
//! switching) and the volume mounted in each Space's guest.
//!
//! A Space's guest mounts its own view (`public/` read-only, its
//! `spaces/<this>/` read-write), served from this host by
//! [`cua_volume::guest::GuestServers`] over the Space's `/volume` socket
//! ([`cua_spaces::volume`]); the access rules, versioning, audit and secret
//! scan stay here, and the guest never holds storage keys. The view is
//! mounted when a local or cloud Space connects
//! ([`cua_spaces::extension::SpacesExtension::space_connected`]) and ends
//! when the Space goes away. While a persistent agent runs in the Space the
//! same mount shows the agent's view (its home read-write), without
//! remounting.
//!
//! [`SpacesDrive`] reaches all of it from a [`Spaces`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cua_spaces::volume::{VOLUME_FEATURE, VolumeAttachment};
use cua_spaces::{Error, Result, Spaces};
use cua_volume::guest::GuestServers;
use cua_volume::service::DriveService;
use cua_volume::vfs::Vfs;

use crate::DriveResult as _;

/// How long a guest gets to mount.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(60);

/// A Space's mounted volume.
pub(crate) struct Mounted {
    attachment: VolumeAttachment,
    servers: Arc<GuestServers>,
    /// The persistent agent whose view the volume shows while it runs.
    holder: Option<String>,
    /// The named volumes the Space mounted (its view grants them).
    access: Vec<cua_volume::MountAccess>,
    /// The mounts they came from.
    named: Vec<cua_spaces::named_volumes::VolumeMount>,
}

impl Mounted {
    fn vfs(&self) -> &Arc<Vfs> {
        self.servers.vfs()
    }

    fn info(&self, space: &str) -> VolumeInfo {
        VolumeInfo {
            space: space.to_string(),
            mount_path: self.attachment.mount_path().to_string(),
            backend: self.attachment.backend().to_string(),
            principal: self.vfs().context().principal.id(),
        }
    }

    async fn detach(self) {
        let _ = self.attachment.detach().await;
        match Arc::try_unwrap(self.servers) {
            Ok(s) => {
                let _ = s.stop().await;
            }
            Err(s) => {
                let _ = s.vfs().flush_all().await;
            }
        }
    }
}

/// Where a Space's volume is mounted, and as whom.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct VolumeInfo {
    pub space: String,
    /// The mount point in the guest (`/volume`, `~/Cua Volume`).
    pub mount_path: String,
    /// `nfs` or `fs`.
    pub backend: String,
    /// `space:<folder>` or `agent:<name>`.
    pub principal: String,
}

/// The drive's runtime state, one per [`crate::DriveExtension`].
pub(crate) struct DriveRuntime {
    keys: Arc<dyn cua_volume::service::KeyStore>,
    service: tokio::sync::OnceCell<Arc<DriveService>>,
    volumes: tokio::sync::Mutex<BTreeMap<String, Mounted>>,
    /// Why a Space that connected has no volume (the attach failed, or its
    /// image has no volume), until it mounts or the Space goes.
    unavailable: std::sync::Mutex<BTreeMap<String, String>>,
    /// Per Space, the task that re-mounts the volume when the guest daemon
    /// restarts (see [`spawn_attach`]).
    watchers: std::sync::Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    pub(crate) volume_auto: bool,
}

impl Default for DriveRuntime {
    fn default() -> Self {
        DriveRuntime {
            keys: Arc::new(cua_volume::service::EnvKeys),
            service: tokio::sync::OnceCell::new(),
            volumes: tokio::sync::Mutex::new(BTreeMap::new()),
            unavailable: std::sync::Mutex::new(BTreeMap::new()),
            watchers: std::sync::Mutex::new(BTreeMap::new()),
            volume_auto: true,
        }
    }
}

impl DriveRuntime {
    fn note_unavailable(&self, space: &str, why: Option<String>) {
        let mut u = self.unavailable.lock().expect("volume notes");
        match why {
            Some(w) => {
                u.insert(space.to_string(), w);
            }
            None => {
                u.remove(space);
            }
        }
    }

    /// Spaces without a volume and why (`space`, `error`), sorted.
    pub(crate) fn unavailable(&self) -> Vec<(String, String)> {
        let u = self.unavailable.lock().expect("volume notes");
        u.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    pub(crate) fn set_keys(&mut self, keys: Arc<dyn cua_volume::service::KeyStore>) {
        self.keys = keys;
    }
}

/// What the mounts of a Space grant it in the Cua Volume.
pub fn mount_access(
    mounts: &[cua_spaces::named_volumes::VolumeMount],
) -> Vec<cua_volume::MountAccess> {
    mounts
        .iter()
        .map(|m| cua_volume::MountAccess {
            prefix: m.folder(),
            mode: if m.read_only {
                cua_volume::Mode::Read
            } else {
                cua_volume::Mode::ReadWrite
            },
        })
        .collect()
}

/// The Cua Volume runtime of a [`Spaces`] with a [`crate::DriveExtension`].
#[allow(async_fn_in_trait)]
pub trait SpacesDrive {
    /// The drive extension, or `host_capability_missing`.
    fn drive_extension(&self) -> Result<&crate::DriveExtension>;

    /// The Cua Volume (agent homes, `public/`, per-Space folders).
    ///
    /// # Panics
    ///
    /// When the runtime has no [`crate::DriveExtension`].
    fn drive(&self) -> &cua_volume::Drive {
        self.drive_extension()
            .expect("the Cua Volume extension (cua_spaces_ext::register)")
            .drive()
    }

    /// The drive's services: change feed, block cache, mount, storage.
    /// Started on first use.
    async fn drive_service(&self) -> Result<Arc<DriveService>>;

    /// The change feed, when the services run (never starts them).
    fn drive_feed(&self) -> Option<Arc<cua_volume::feed::Feed>> {
        self.drive_extension()
            .ok()?
            .runtime
            .service
            .get()
            .map(|s| s.feed().clone())
    }

    /// Starts the services now (a daemon at startup).
    async fn start_drive_service(&self) -> Result<()> {
        self.drive_service().await.map(|_| ())
    }

    /// Unmounts every Space's volume, then stops the services: uploads
    /// still pending land and this machine's mount goes away.
    async fn stop_drive_service(&self);

    /// Mounts Cua Volume in `space` as the Space's own view, unless it is
    /// mounted already. `None` when the guest has no mount backend: the
    /// Space works without the volume.
    async fn volume_attach(&self, space: &str) -> Result<Option<VolumeInfo>>;

    /// Unmounts the volume in `space` (writes still pending land first).
    async fn volume_detach(&self, space: &str) -> Result<bool>;

    /// The mounted volume of `space`, if any.
    async fn volume(&self, space: &str) -> Option<VolumeInfo>;

    /// Every mounted volume.
    async fn volumes(&self) -> Vec<VolumeInfo>;

    /// Shows persistent agent `agent`'s view on `space`'s volume while it
    /// runs there (mounting it if needed). `None` when the Space has no
    /// volume or another agent holds it: the caller copies the home in and
    /// out instead.
    async fn volume_for_agent(&self, space: &str, agent: &str) -> Result<Option<VolumeInfo>>;

    /// Lands `agent`'s writes on `space`'s volume if it holds it; `false`
    /// when it does not.
    async fn volume_flush_agent(&self, space: &str, agent: &str) -> Result<bool>;

    /// Gives `space`'s volume back to the Space's own view when `agent`
    /// holds it.
    async fn volume_release_agent(&self, space: &str, agent: &str) -> Result<()>;
}

impl SpacesDrive for Spaces {
    fn drive_extension(&self) -> Result<&crate::DriveExtension> {
        self.extension::<crate::DriveExtension>()
            .ok_or_else(|| Error::needs_cua_spaces("Cua Volume"))
    }

    async fn drive_service(&self) -> Result<Arc<DriveService>> {
        let ext = self.drive_extension()?;
        let rt = &ext.runtime;
        rt.service
            .get_or_try_init(|| {
                // Boxed: the mount's start-up future is deep, and inlining
                // it into every caller's state machine overflows layout.
                let fut: std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<Arc<DriveService>>> + Send + '_>,
                > = Box::pin(async {
                    let svc =
                        DriveService::new(ext.drive().clone(), self.home_dir(), rt.keys.clone())
                            .drive()?;
                    svc.start().await;
                    Ok(svc)
                });
                fut
            })
            .await
            .cloned()
    }

    async fn stop_drive_service(&self) {
        let Ok(ext) = self.drive_extension() else {
            return;
        };
        ext.runtime.stop_watchers();
        // This machine's mount first: it is quick, and a daemon that is
        // ended before the Spaces' volumes are done (a stop that waits
        // 30 s, then signals) must not leave it behind with no server, the
        // macOS "Server connections interrupted" dialog.
        if let Some(svc) = ext.runtime.service.get() {
            Box::pin(svc.release_mount()).await;
        }
        let all: Vec<Mounted> = std::mem::take(&mut *ext.runtime.volumes.lock().await)
            .into_values()
            .collect();
        for m in all {
            m.detach().await;
        }
        if let Some(svc) = ext.runtime.service.get() {
            Box::pin(svc.shutdown()).await;
        }
    }

    async fn volume_attach(&self, space: &str) -> Result<Option<VolumeInfo>> {
        let ext = self.drive_extension()?;
        let id = self.resolve(space)?.to_string();
        let mut volumes = ext.runtime.volumes.lock().await;
        if let Some(m) = volumes.get(&id) {
            // Live, and with the mounts the Space has now (a Space that
            // connected first and got its mounts after is mounted again).
            if m.attachment.is_live() && m.named == self.volume_mounts(&id)? {
                return Ok(Some(m.info(&id)));
            }
            if let Some(stale) = volumes.remove(&id) {
                stale.detach().await;
            }
        }
        let s = self.space(&id).await?;
        let named = self.volume_mounts(&id)?;
        let access = mount_access(&named);
        let Some(backend) = s.volume_backend() else {
            let answered_before = self
                .registry()
                .get(&id)
                .ok()
                .flatten()
                .is_some_and(|r| !r.spacesd_version.is_empty());
            if not_up_yet(s.has_spacesd(), answered_before) {
                // Connected while its cua-spacesd was still starting (a
                // fresh or restarted container): the connection has no
                // capabilities, not a guest without the volume. Connect
                // afresh next time, and fail so the supervisor retries.
                let _ = self.forget_connection(&id).await;
                // No route to the VM is not "still starting": macOS Local
                // Network privacy blocks this process, and waiting does not
                // help until the named app is allowed (CUA-1807).
                let why = match s.spacesd_unreachable_cause() {
                    Some(cause) => format!("its cua-spacesd can't be reached: {cause}"),
                    None => "its cua-spacesd is not answering yet (the Space is still starting); \
                             Cua Volume mounts once it does"
                        .into(),
                };
                ext.runtime.note_unavailable(&id, Some(why.clone()));
                return Err(Error::extension("precondition_failed", why));
            }
            let why = s
                .feature(VOLUME_FEATURE)
                .map(|f| f.limitation.clone())
                .unwrap_or_else(|| "its cua-spacesd predates the volume".into());
            tracing::info!(space = %id, "no Cua Volume in this Space: {why}");
            ext.runtime
                .note_unavailable(&id, Some(format!("no Cua Volume in this Space: {why}")));
            // A Space that asked for named volumes cannot work without them.
            if !named.is_empty() {
                return Err(Error::CapabilityMissing {
                    space: id,
                    feature: VOLUME_FEATURE.into(),
                    limitation: format!("named volumes need the Cua Volume mount: {why}"),
                });
            }
            return Ok(None);
        };
        let drive = ext.drive();
        let svc = self.drive_service().await.ok();
        let state = drive
            .state_dir()
            .join("volumes")
            .join(cua_volume::path::space_folder_name(&id));
        let vfs = Vfs::new(
            drive,
            cua_volume::Context::space(&id).with_mounts(access.clone()),
            svc.as_ref().map(|s| s.cache().clone()),
            svc.as_ref().map(|s| s.feed().clone()),
            &state,
        )
        .drive()?;
        let servers = Arc::new(GuestServers::start(vfs).await.drive()?);
        if servers.target(&backend).is_none() {
            return Err(Error::CapabilityMissing {
                space: id,
                feature: VOLUME_FEATURE.into(),
                limitation: format!("this host cannot serve a {backend} guest mount"),
            });
        }
        let attachment = s
            .attach_volume_with(
                servers.dialer(),
                None,
                cua_spaces::named_volumes::binds(&named),
            )
            .await?;
        if let Err(e) = attachment.wait_mounted(MOUNT_TIMEOUT).await {
            let _ = attachment.detach().await;
            if let Ok(s) = Arc::try_unwrap(servers) {
                let _ = s.stop().await;
            }
            return Err(e);
        }
        let m = Mounted {
            attachment,
            servers,
            holder: None,
            access,
            named,
        };
        let info = m.info(&id);
        tracing::info!(space = %id, path = %info.mount_path, backend = %info.backend, "Cua Volume mounted");
        ext.runtime.note_unavailable(&id, None);
        volumes.insert(id, m);
        Ok(Some(info))
    }

    async fn volume_detach(&self, space: &str) -> Result<bool> {
        let ext = self.drive_extension()?;
        let id = self.resolve(space)?.to_string();
        Ok(ext.runtime.detach(&id).await)
    }

    async fn volume(&self, space: &str) -> Option<VolumeInfo> {
        let ext = self.drive_extension().ok()?;
        let id = self.resolve(space).ok()?.to_string();
        let v = ext.runtime.volumes.lock().await;
        v.get(&id)
            .filter(|m| m.attachment.is_live())
            .map(|m| m.info(&id))
    }

    async fn volumes(&self) -> Vec<VolumeInfo> {
        let Ok(ext) = self.drive_extension() else {
            return vec![];
        };
        ext.runtime
            .volumes
            .lock()
            .await
            .iter()
            .filter(|(_, m)| m.attachment.is_live())
            .map(|(id, m)| m.info(id))
            .collect()
    }

    async fn volume_for_agent(&self, space: &str, agent: &str) -> Result<Option<VolumeInfo>> {
        if self.volume_attach(space).await?.is_none() {
            return Ok(None);
        }
        let ext = self.drive_extension()?;
        let id = self.resolve(space)?.to_string();
        let mut volumes = ext.runtime.volumes.lock().await;
        let Some(m) = volumes.get_mut(&id) else {
            return Ok(None);
        };
        if m.holder.as_deref().is_some_and(|h| h != agent) {
            return Ok(None);
        }
        m.vfs()
            .set_context(cua_volume::Context::agent(agent, Some(&id)).with_mounts(m.access.clone()))
            .await
            .drive()?;
        m.holder = Some(agent.to_string());
        Ok(Some(m.info(&id)))
    }

    async fn volume_flush_agent(&self, space: &str, agent: &str) -> Result<bool> {
        let ext = self.drive_extension()?;
        let id = self.resolve(space)?.to_string();
        let vfs = {
            let v = ext.runtime.volumes.lock().await;
            match v.get(&id) {
                Some(m) if m.holder.as_deref() == Some(agent) => m.vfs().clone(),
                _ => return Ok(false),
            }
        };
        vfs.flush_all().await.drive()?;
        Ok(true)
    }

    async fn volume_release_agent(&self, space: &str, agent: &str) -> Result<()> {
        let ext = self.drive_extension()?;
        let id = self.resolve(space)?.to_string();
        let mut volumes = ext.runtime.volumes.lock().await;
        if let Some(m) = volumes.get_mut(&id)
            && m.holder.as_deref() == Some(agent)
        {
            m.vfs()
                .set_context(cua_volume::Context::space(&id).with_mounts(m.access.clone()))
                .await
                .drive()?;
            m.holder = None;
        }
        Ok(())
    }
}

impl DriveRuntime {
    fn stop_watcher(&self, id: &str) {
        if let Some(t) = self.watchers.lock().expect("volume watchers").remove(id) {
            t.abort();
        }
    }

    pub(crate) fn stop_watchers(&self) {
        let all = std::mem::take(&mut *self.watchers.lock().expect("volume watchers"));
        for t in all.into_values() {
            t.abort();
        }
    }

    /// Unmounts `id`'s volume; whether there was one.
    pub(crate) async fn detach(&self, id: &str) -> bool {
        self.stop_watcher(id);
        self.note_unavailable(id, None);
        let m = self.volumes.lock().await.remove(id);
        match m {
            Some(m) => {
                m.detach().await;
                true
            }
            None => false,
        }
    }
}

/// Mounts the volume of a Space that just connected, in the background (a
/// guest without the backend is left alone). Local and cloud Spaces only:
/// a machine added by address, or a host Space, is someone's own computer.
/// How long a Space's volume may take to mount when it connects (the
/// guest's own mount wait is [`MOUNT_TIMEOUT`]).
pub const ATTACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Whether a Space that just connected gets the volume: local and cloud
/// Spaces always, any other when it has named volumes to serve.
pub(crate) fn attaches_on_connect(id: &str, has_mounts: bool) -> bool {
    id.starts_with("local:") || id.starts_with("cloud:") || has_mounts
}

pub(crate) fn spawn_attach(spaces: &Spaces, id: &str) {
    // Elsewhere, the Space is someone's own computer, unless it asked for
    // named volumes: then this machine, which holds the Cua Volume, serves
    // them over the Space's own connection (a Space on a pool host).
    if !attaches_on_connect(id, spaces.volume_mounts(id).is_ok_and(|m| !m.is_empty())) {
        return;
    }
    // A cluster job's Space runs one command for someone else's queue: it
    // gets no volume.
    if cua_spaces::host_jobs::is_job_space(id) {
        return;
    }
    let Ok(ext) = spaces.drive_extension() else {
        return;
    };
    let key = id.to_string();
    let task = tokio::spawn(keep_attached(spaces.clone(), key.clone()));
    // A fresh connection replaces the old watcher.
    if let Some(old) = ext
        .runtime
        .watchers
        .lock()
        .expect("volume watchers")
        .insert(key, task)
    {
        old.abort();
    }
}

/// Whether a Space without the volume feature on this connection is only
/// still starting: no cua-spacesd answered when it connected, though one did
/// before. A guest whose cua-spacesd answers without the feature predates
/// the volume; one that never ran cua-spacesd has nothing to mount.
fn not_up_yet(has_spacesd: bool, answered_before: bool) -> bool {
    !has_spacesd && answered_before
}

/// Mounts the Space's volume and keeps it mounted: when the guest daemon
/// restarts (an update, a crash, a reboot) its mount and its half of the
/// socket are gone, so the volume is mounted again, with a backoff while the
/// daemon is still coming up. Ends when the Space is dropped or its volume
/// detached (both abort this task) or when the guest has no mount backend.
async fn keep_attached(spaces: Spaces, key: String) {
    use cua_spaces::reattach::{Attempt, Policy, supervise};
    let health = {
        let (spaces, key) = (spaces.clone(), key.clone());
        move || {
            let (spaces, key) = (spaces.clone(), key.clone());
            async move { volume_health(&spaces, &key).await }
        }
    };
    let reattach = {
        let (spaces, key) = (spaces.clone(), key.clone());
        move || {
            let (spaces, key) = (spaces.clone(), key.clone());
            async move {
                // Bounded: the Space is ready either way; a volume that
                // cannot mount says why (`volume_sync_status`,
                // `cua volume status`).
                let r = tokio::time::timeout(ATTACH_TIMEOUT, remount(&spaces, &key)).await;
                let why = match r {
                    Ok(Ok(Some(_))) => return Ok(Attempt::Attached),
                    Ok(Ok(None)) => return Ok(Attempt::Unsupported),
                    Ok(Err(e)) => format!("Cua Volume not mounted: {e}"),
                    Err(_) => format!(
                        "Cua Volume not mounted: no answer within {} s",
                        ATTACH_TIMEOUT.as_secs()
                    ),
                };
                if let Ok(ext) = spaces.drive_extension() {
                    ext.runtime.note_unavailable(&key, Some(why.clone()));
                }
                Err(why)
            }
        }
    };
    supervise(Policy::default(), health, reattach).await;
}

/// Whether the Space's volume is mounted and served: this side's socket is
/// open and the guest still reports its mount.
async fn volume_health(spaces: &Spaces, key: &str) -> cua_spaces::reattach::Health {
    use cua_spaces::reattach::Health;
    let Ok(ext) = spaces.drive_extension() else {
        return Health::Gone;
    };
    let live = ext
        .runtime
        .volumes
        .lock()
        .await
        .get(key)
        .map(|m| m.attachment.is_live());
    match live {
        None | Some(false) => {
            if stopped_on_its_own(spaces, key).await {
                // Its container stopped without a `stop` (out of memory, a
                // crash): retrying every minute only fills the log. Starting
                // it again connects it, and that mounts the volume afresh.
                tracing::warn!(
                    space = %key,
                    "the Space stopped without being asked to; its Cua Volume is mounted again when it starts"
                );
                ext.runtime
                    .note_unavailable(key, Some("the Space is not running".into()));
                return Health::Gone;
            }
            Health::Down
        }
        Some(true) => match spaces.space(key).await {
            // A restarted daemon answers `detached`: it knows nothing of the
            // socket this side still holds.
            Ok(s) => match s.volume_status().await {
                Ok(st) if st.state == "detached" || st.state == "error" => Health::Down,
                _ => Health::Healthy,
            },
            Err(_) => Health::Healthy,
        },
    }
}

/// Whether a local Space's sandbox is definitely not running (stopped, or its
/// container gone). An engine that cannot answer counts as running, so the
/// supervisor keeps retrying as before.
async fn stopped_on_its_own(spaces: &Spaces, key: &str) -> bool {
    let Some(name) = key.strip_prefix("local:") else {
        return false;
    };
    match spaces.sandboxes().get(name).await {
        Ok(info) => is_down(&info.status),
        Err(_) => false,
    }
}

fn is_down(status: &cua_sandbox_core::Status) -> bool {
    use cua_sandbox_core::Status;
    match status {
        Status::Stopped => true,
        Status::Unknown(word) => word == cua_sandbox_core::MISSING,
        _ => false,
    }
}

/// Mounts again: drops what is left of the old attachment (keeping a
/// persistent agent's view on it), then attaches.
async fn remount(spaces: &Spaces, key: &str) -> Result<Option<VolumeInfo>> {
    let ext = spaces.drive_extension()?;
    let holder = {
        let mut volumes = ext.runtime.volumes.lock().await;
        match volumes.remove(key) {
            Some(old) => {
                let holder = old.holder.clone();
                drop(volumes);
                old.detach().await;
                holder
            }
            None => None,
        }
    };
    let info = spaces.volume_attach(key).await?;
    if let (Some(agent), Some(_)) = (holder, &info) {
        let _ = spaces.volume_for_agent(key, &agent).await;
    }
    if info.is_some() {
        tracing::info!(space = %key, "Cua Volume mounted again");
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::attaches_on_connect;

    #[test]
    fn only_spaces_with_mounts_attach_off_this_machine() {
        assert!(attaches_on_connect("local:a", false));
        assert!(attaches_on_connect("cloud:a", false));
        assert!(!attaches_on_connect("relay:m1", false));
        assert!(!attaches_on_connect("direct:10.0.0.5:3211", false));
        assert!(attaches_on_connect("relay:m1", true));
        assert!(attaches_on_connect("direct:10.0.0.5:3211", true));
    }
}

#[cfg(test)]
mod not_up_yet_tests {
    use super::not_up_yet;

    #[test]
    fn a_daemon_that_answered_before_is_still_starting() {
        assert!(not_up_yet(false, true));
        // It answered on this connection: the feature list is real.
        assert!(!not_up_yet(true, true));
        assert!(!not_up_yet(true, false));
        // An image that never ran cua-spacesd.
        assert!(!not_up_yet(false, false));
    }
}

#[cfg(test)]
mod volume_gone_tests {
    use super::is_down;
    use cua_sandbox_core::{MISSING, Status};

    #[test]
    fn only_a_stopped_or_missing_sandbox_ends_the_volume_watch() {
        assert!(is_down(&Status::Stopped));
        assert!(is_down(&Status::Unknown(MISSING.into())));
        assert!(!is_down(&Status::Running));
        assert!(!is_down(&Status::Suspended));
        assert!(!is_down(&Status::Provisioning));
        assert!(!is_down(&Status::Unknown("restarting".into())));
    }
}
