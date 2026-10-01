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
    pub(crate) volume_auto: bool,
}

impl Default for DriveRuntime {
    fn default() -> Self {
        DriveRuntime {
            keys: Arc::new(cua_volume::service::EnvKeys),
            service: tokio::sync::OnceCell::new(),
            volumes: tokio::sync::Mutex::new(BTreeMap::new()),
            unavailable: std::sync::Mutex::new(BTreeMap::new()),
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
            if m.attachment.is_live() {
                return Ok(Some(m.info(&id)));
            }
            if let Some(stale) = volumes.remove(&id) {
                stale.detach().await;
            }
        }
        let s = self.space(&id).await?;
        let Some(backend) = s.volume_backend() else {
            let why = s
                .feature(VOLUME_FEATURE)
                .map(|f| f.limitation.clone())
                .unwrap_or_else(|| "its cua-spacesd predates the volume".into());
            tracing::info!(space = %id, "no Cua Volume in this Space: {why}");
            ext.runtime
                .note_unavailable(&id, Some(format!("no Cua Volume in this Space: {why}")));
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
            cua_volume::Context::space(&id),
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
        let attachment = s.attach_volume(servers.dialer(), None).await?;
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
            .set_context(cua_volume::Context::agent(agent, Some(&id)))
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
                .set_context(cua_volume::Context::space(&id))
                .await
                .drive()?;
            m.holder = None;
        }
        Ok(())
    }
}

impl DriveRuntime {
    /// Unmounts `id`'s volume; whether there was one.
    pub(crate) async fn detach(&self, id: &str) -> bool {
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

pub(crate) fn spawn_attach(spaces: &Spaces, id: &str) {
    if !(id.starts_with("local:") || id.starts_with("cloud:")) {
        return;
    }
    let spaces = spaces.clone();
    let key = id.to_string();
    tokio::spawn(async move {
        // Bounded: the Space is ready either way; a volume that cannot
        // mount says why (`volume_sync_status`, `cua volume status`).
        let why = match tokio::time::timeout(ATTACH_TIMEOUT, spaces.volume_attach(&key)).await {
            Ok(Ok(_)) => return,
            Ok(Err(e)) => format!("Cua Volume not mounted: {e}"),
            Err(_) => format!(
                "Cua Volume not mounted: no answer within {} s",
                ATTACH_TIMEOUT.as_secs()
            ),
        };
        tracing::warn!(space = %key, "{why}");
        if let Ok(ext) = spaces.drive_extension() {
            ext.runtime.note_unavailable(&key, Some(why));
        }
    });
}
