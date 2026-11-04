//! Where sandboxes come from. [`CuaBackend`] is the real one: Cua Spaces
//! through the cua SDK (the running `cua daemon`, else an embedded runtime).
//! Every sandbox is an image plus a kind (`vm` by default), never a host
//! process. Data-plane calls (processes, files) go to the sandbox's
//! cua-spacesd through the client this module hands out.

use async_trait::async_trait;
use cua_sdk::{
    Cua, CuaConfig, CuaError, PortForward, Sandbox, SandboxCreateOptions, SpaceCreateOptions,
};
use cua_spacesd_client::SpacesdClient;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A backend failure, classified for the provider's error shapes.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BackendError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Timeout(String),
    /// The account is at its live-session (or remote-hours) limit: the
    /// message is the plan-limit sentence with its upgrade path, verbatim.
    /// Providers answer their rate-limit error (HTTP 429).
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    Other(String),
}

impl From<CuaError> for BackendError {
    fn from(e: CuaError) -> Self {
        let msg = e.to_string();
        // The relay's plan limit ("You're using N of N live sessions on
        // <plan>. ...") reaches us as text inside whatever error carried it.
        if cua_home::plan_limit::PlanLimit::parse(&msg).is_some() {
            return BackendError::RateLimited(plan_limit_sentence(&msg));
        }
        match e {
            CuaError::NotFound(_) => BackendError::NotFound(msg),
            CuaError::InvalidArgument(_) | CuaError::InvalidPlacement(_) => {
                BackendError::Invalid(msg)
            }
            // A create with network none / egress that the host or daemon
            // cannot enforce is refused by core: surface it as the provider's
            // 400 with core's reason, never an open sandbox.
            CuaError::Unsupported(_)
            | CuaError::CapabilityMissing(_)
            | CuaError::HostCapabilityMissing(_) => BackendError::Unsupported(msg),
            CuaError::Timeout(_) => BackendError::Timeout(msg),
            _ => BackendError::Other(msg),
        }
    }
}

/// The plan-limit sentence out of an error text that may wrap it.
fn plan_limit_sentence(text: &str) -> String {
    cua_home::plan_limit::PlanLimit::parse(text)
        .map(|l| l.sentence())
        .unwrap_or_else(|| text.to_string())
}

pub type Result<T> = std::result::Result<T, BackendError>;

/// Which Cua identity a call runs as (from the provider API key).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Identity {
    /// Cua daemon address; `None`: the ambient one.
    pub daemon: Option<String>,
    /// Cua daemon token.
    pub token: Option<String>,
}

/// What a new sandbox starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A fresh Space from an image (`None`: the configured default image).
    Image(Option<String>),
    /// A fork of a checkpoint (copy-on-write where the runtime supports it).
    Checkpoint(String),
}

/// A sandbox to make.
#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub name: String,
    pub source: Source,
    /// Guest environment for the Space (image creates).
    pub env: HashMap<String, String>,
    /// `false`: no outbound network (`network = "none"`).
    pub internet: bool,
    /// Per-sandbox placement and size, over the gateway defaults: any
    /// location Cua has (`local`, `host:<machine>`, a pool, a BYOC cloud
    /// such as `aws` or a contrib provider) and any kind (`vm`,
    /// `container`, `microvm`; macOS and Windows images too).
    pub place: Placement,
}

/// Overrides of [`SpaceDefaults`] for one sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    pub on: Option<String>,
    pub kind: Option<String>,
    pub cpus: Option<u32>,
    pub memory_mb: Option<u64>,
    /// Keep the guest's desktop (`cua.desktop` metadata or label). Off by
    /// default: provider sandboxes are shells, files and ports, so they
    /// boot without display hardware (several times faster, and many more
    /// fit on a machine).
    pub desktop: bool,
}

/// What [`Backend::create`] made.
#[derive(Debug, Clone)]
pub struct Created {
    /// The sandbox's Cua name (the handle every later call takes).
    pub name: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

/// A new size for a sandbox: `None` keeps the current value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shape {
    pub cpus: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_gb: Option<u32>,
}

/// A checkpoint and the sandbox it was taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointInfo {
    pub name: String,
    /// The source sandbox's Cua name.
    pub source: String,
}

/// Live state of a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Running,
    Suspended,
    Gone,
}

/// What a create may ask for beyond [`CreateSpec`] (Daytona's disk size,
/// GPU and image layers). `Default` is the plain create.
#[derive(Debug, Clone, Default)]
pub struct CreateExtras {
    /// Disk size in GiB (local VMs).
    pub disk_gb: Option<u32>,
    /// A GPU option of the runtime (`auto`, `virgl`, `nvidia`, ...).
    pub gpu: Option<String>,
    /// Image layers built on top of the image (a Dockerfile's steps).
    pub build: Option<cua_sdk::ImageBuild>,
    /// The sandbox's command (a Dockerfile's ENTRYPOINT/CMD).
    pub command: Option<Vec<String>>,
    /// Take the sandbox from this Cua pool (a warm pool's ready instances).
    pub pool: Option<String>,
}

impl CreateExtras {
    pub fn is_default(&self) -> bool {
        self.disk_gb.is_none()
            && self.gpu.is_none()
            && self.build.is_none()
            && self.command.is_none()
            && self.pool.is_none()
    }
}

/// One entry of a Cua Volume folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEntry {
    /// Key (folders end in `/`).
    pub path: String,
    pub name: String,
    pub folder: bool,
    pub size: u64,
    pub modified_ms: u64,
}

fn no_volume() -> BackendError {
    BackendError::Unsupported(
        "Cua Volume is not available through this backend (it needs the Cua daemon Cua Spaces runs)".into(),
    )
}

#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// The immediate children of a Cua Volume folder (`""` is the root).
    async fn volume_ls(&self, _who: &Identity, _path: &str) -> Result<Vec<VolumeEntry>> {
        Err(no_volume())
    }
    /// A Cua Volume file's content.
    async fn volume_read(&self, _who: &Identity, _path: &str) -> Result<Vec<u8>> {
        Err(no_volume())
    }
    /// Writes a Cua Volume file (a new version).
    async fn volume_write(&self, _who: &Identity, _path: &str, _data: Vec<u8>) -> Result<()> {
        Err(no_volume())
    }
    /// Deletes a Cua Volume file (history stays).
    async fn volume_delete(&self, _who: &Identity, _path: &str) -> Result<()> {
        Err(no_volume())
    }

    async fn create(&self, who: &Identity, spec: CreateSpec) -> Result<Created>;
    /// [`Backend::create`] with extras. Backends that cannot honour an
    /// extra refuse it; none is silently dropped.
    async fn create_with(
        &self,
        who: &Identity,
        spec: CreateSpec,
        extras: CreateExtras,
    ) -> Result<Created> {
        if !extras.is_default() {
            return Err(BackendError::Unsupported(
                "this backend cannot create with a disk size, GPU or image layers".into(),
            ));
        }
        self.create(who, spec).await
    }
    async fn delete(&self, who: &Identity, name: &str) -> Result<bool>;
    /// Whether the sandbox really has no outbound network: checked for a
    /// sandbox created without internet, because a host or daemon older than
    /// the request may ignore `network = none` and create an open one. `true`
    /// when it cannot be checked (the core's refusal is then the control).
    async fn network_blocked(&self, _who: &Identity, _name: &str) -> Result<bool> {
        Ok(true)
    }
    /// Stops a create that is still running for `name` (a host being
    /// booted for it included); `true` when one was running. The create
    /// itself then fails. Backends without one do nothing.
    async fn cancel_create(&self, _who: &Identity, _name: &str) -> Result<bool> {
        Ok(false)
    }
    /// Whether a sandbox of this placement forks from a checkpoint in about
    /// a second (a VM with memory restore), so a booted base is worth
    /// keeping to fork for each create.
    fn forks_fast(&self, _place: &Placement) -> bool {
        false
    }
    /// Asks the Cua side to end (or suspend) sandbox `name` `seconds` from
    /// now even if this gateway is dead by then (the Cua daemon enforces a
    /// Space lifetime). `Ok(true)`: recorded there. `Ok(false)`: this
    /// backend has no Cua-side lifetime, so the gateway's own reaper is the
    /// only thing that ends the sandbox (and only while the gateway runs).
    async fn set_lifetime(
        &self,
        _who: &Identity,
        _name: &str,
        _seconds: u64,
        _on_expire: &str,
    ) -> Result<bool> {
        Ok(false)
    }
    async fn liveness(&self, who: &Identity, name: &str) -> Result<Liveness>;
    /// The sandbox's cua-spacesd.
    async fn spacesd(&self, who: &Identity, name: &str) -> Result<SpacesdClient>;
    /// A local address that reaches guest `port` (kept open until the
    /// sandbox is deleted).
    async fn forward(&self, who: &Identity, name: &str, port: u16) -> Result<String>;
    async fn suspend(&self, who: &Identity, name: &str) -> Result<()>;
    async fn resume(&self, who: &Identity, name: &str) -> Result<()>;
    /// Reboots the sandbox (disk kept, processes gone).
    async fn restart(&self, who: &Identity, name: &str) -> Result<()>;
    /// Gives the sandbox a new shape (cold restart with the new vCPUs,
    /// memory and disk). Backends without a core resize say so; nothing is
    /// faked.
    async fn resize(&self, _who: &Identity, _name: &str, _shape: Shape) -> Result<()> {
        Err(BackendError::Unsupported(
            "this backend cannot give a sandbox another shape".into(),
        ))
    }
    /// Where the sandbox runs, as a `cua.on` placement word (`local`,
    /// `host:<machine>`), to put another sandbox beside it.
    async fn placement_of(&self, _who: &Identity, _name: &str) -> Result<Option<String>> {
        Ok(None)
    }
    /// Keeps `min` sandboxes of `image` ready (a Cua pool), by `name`.
    async fn pool_create(
        &self,
        _who: &Identity,
        _name: &str,
        _image: &str,
        _min: u32,
        _place: &Placement,
    ) -> Result<()> {
        Err(BackendError::Unsupported(
            "this backend has no pools".into(),
        ))
    }
    /// How many instances of pool `name` are ready (`None`: no such pool).
    async fn pool_ready(&self, _who: &Identity, _name: &str) -> Result<Option<u32>> {
        Ok(None)
    }
    async fn pool_delete(&self, _who: &Identity, _name: &str) -> Result<bool> {
        Ok(false)
    }
    /// Checks an image reference can be pulled (Daytona pulls at snapshot
    /// creation, so a bad name fails there).
    async fn check_image(&self, _who: &Identity, _image: &str) -> Result<()> {
        Ok(())
    }
    async fn checkpoint(&self, who: &Identity, name: &str, checkpoint: &str) -> Result<()>;
    async fn checkpoints(&self, who: &Identity) -> Result<Vec<CheckpointInfo>>;
    async fn delete_checkpoint(&self, who: &Identity, checkpoint: &str) -> Result<bool>;
}

/// Placement and size for new Spaces.
#[derive(Debug, Clone, Default)]
pub struct SpaceDefaults {
    /// `--on`. Empty (the default): the core default placement, as for every
    /// Cua create (this machine, or its overflow when a cloud is linked);
    /// `auto` is the least-loaded machine of the account pool.
    pub on: String,
    /// `--kind` (default: [`default_kind`], `vm` where this machine can run
    /// one, else `container`).
    pub kind: String,
    /// Image for `template = base` / unmapped templates.
    pub image: String,
    pub cpus: Option<u32>,
    pub memory_mb: Option<u64>,
    /// Freshly built binaries injected into every new sandbox (`cua compat
    /// serve --overlay`, as `cua sandbox create --overlay`): for trying a
    /// guest agent that no image carries yet.
    pub overlays: Vec<cua_sdk::Overlay>,
}

/// The kind a gateway's sandboxes get when nothing says otherwise: a VM
/// (memory snapshots, pause and fork all work) where this machine can run
/// one with hardware acceleration (a Mac, or Linux with `/dev/kvm`), else a
/// container (a VM without acceleration is too slow to be a default).
pub fn default_kind() -> &'static str {
    kind_for(cua_sandbox_core::placement::HostFacts::current())
}

/// The kind for a sandbox placed `on` when none was asked for: the host's
/// own default ([`default_kind`]) here, and a container anywhere else (a
/// VM there means a host with KVM, which a cloud pool buys as metal at
/// several dollars an hour for one small sandbox: only when asked).
pub fn kind_on(configured: &str, on: &str) -> String {
    if !configured.is_empty() {
        configured.to_string()
    } else if matches!(on, "" | "local") {
        default_kind().to_string()
    } else {
        "container".to_string()
    }
}

fn kind_for(h: cua_sandbox_core::placement::HostFacts) -> &'static str {
    if h.kvm || cfg!(target_os = "macos") {
        "vm"
    } else {
        "container"
    }
}

/// Sandboxes as Cua Spaces through the cua SDK.
///
/// - **Create** from an image: `Spaces.create` (placement `on` = any of your
///   machines, the pool, or a cloud; any kind). With no outbound network
///   (`network = "none"`, local QEMU only) it is `Sandboxes.create`.
/// - **Fork**: a checkpoint this machine holds, `Sandboxes.fork`; else
///   (or with an `on`) `Spaces.create` of image `checkpoint:<name>`, which
///   starts it on the machine of yours that holds it (or where `on` says,
///   after a verified copy).
/// - **Checkpoint / suspend / resume / restart**: the `Sandboxes` calls by
///   the Space's name (a checkpoint of a Space another machine of yours
///   provides is taken there, by the core).
/// - **Processes, files**: the Space's cua-spacesd.
/// - **Ports**: a cua-spacesd tunnel forward, which reaches a Space on any
///   machine the SDK reaches (this one, the LAN, the relay).
pub struct CuaBackend {
    defaults: SpaceDefaults,
    clients: Mutex<HashMap<Identity, Arc<Cua>>>,
    handles: Mutex<HashMap<(Identity, String), Arc<Sandbox>>>,
    /// Space ids by Space name, for those made with `Spaces.create`.
    spaces: Mutex<HashMap<String, String>>,
    forwards: Mutex<HashMap<(String, u16), Forward>>,
}

/// A port forward, held open while it is in the map (the handle is never
/// read; dropping it closes the forward).
enum Forward {
    Tunnel(
        #[allow(dead_code)] Arc<cua_spacesd_client::TcpForward>,
        String,
    ),
    Sandbox(#[allow(dead_code)] Arc<PortForward>, String),
}

impl Forward {
    fn addr(&self) -> String {
        match self {
            Forward::Tunnel(_, a) | Forward::Sandbox(_, a) => a.clone(),
        }
    }
}

impl CuaBackend {
    pub fn new(defaults: SpaceDefaults) -> Self {
        Self {
            defaults,
            clients: Mutex::default(),
            handles: Mutex::default(),
            spaces: Mutex::default(),
            forwards: Mutex::default(),
        }
    }

    async fn client(&self, who: &Identity) -> Result<Arc<Cua>> {
        let mut clients = self.clients.lock().await;
        if let Some(c) = clients.get(who) {
            return Ok(c.clone());
        }
        let c = if who.daemon.is_some() || who.token.is_some() {
            Cua::connect(who.daemon.clone(), who.token.clone())?
        } else {
            Cua::auto(CuaConfig::default())?
        };
        clients.insert(who.clone(), c.clone());
        Ok(c)
    }

    async fn handle(&self, who: &Identity, name: &str) -> Result<Arc<Sandbox>> {
        let key = (who.clone(), name.to_string());
        if let Some(h) = self.handles.lock().await.get(&key) {
            return Ok(h.clone());
        }
        let sb = self
            .client(who)
            .await?
            .sandboxes()
            .connect(name.to_string())
            .await?;
        self.handles.lock().await.insert(key, sb.clone());
        Ok(sb)
    }

    /// The Space id of a sandbox this backend made with `Spaces.create`
    /// (also after a gateway restart: resolved by name).
    async fn space_id(&self, who: &Identity, name: &str) -> Option<String> {
        if let Some(id) = self.spaces.lock().await.get(name).cloned() {
            return Some(id);
        }
        let c = self.client(who).await.ok()?;
        let info = c.spaces().resolve(name.to_string()).await.ok()?;
        self.spaces
            .lock()
            .await
            .insert(name.to_string(), info.id.clone());
        Some(info.id)
    }

    async fn forget(&self, who: &Identity, name: &str) {
        self.handles
            .lock()
            .await
            .remove(&(who.clone(), name.to_string()));
        self.forwards.lock().await.retain(|(n, _), _| n != name);
    }
}

/// Where a `vm:<path>` image can go: the path is a file on this machine, so
/// `auto` means here, and any other machine is refused with the reason
/// (instead of failing on a host that lacks the file).
pub(crate) fn place_local_disk(image: Option<&str>, on: Option<String>) -> Result<Option<String>> {
    let Some(path) = image.and_then(|i| i.strip_prefix("vm:")) else {
        return Ok(on);
    };
    // Omitted is the core default, which is this machine unless a cloud is
    // linked with a budget; a disk file cannot move, so it is this machine.
    let Some(on) = on else {
        return Ok(Some("local".into()));
    };
    match on.as_str() {
        "local" => Ok(Some(on)),
        "auto" if std::path::Path::new(path).exists() => Ok(Some("local".into())),
        "auto" => Err(BackendError::Invalid(format!(
            "image vm:{path} is not a file on this machine"
        ))),
        _ => Err(BackendError::Invalid(format!(
            "image vm:{path} is a disk file on the gateway machine, so it can only run on this machine (cua.on = local); other machines need an image they can pull"
        ))),
    }
}

/// Whether core reports the egress policy as enforced by the host, outside
/// the guest (the control for `allow_internet_access=false`).
pub(crate) fn enforced_by_host(e: Option<cua_sdk::EgressEnforcement>) -> bool {
    e == Some(cua_sdk::EgressEnforcement::Host)
}

/// Whether a placement refusal means "every machine is busy right now".
pub(crate) fn is_no_capacity(m: &str) -> bool {
    m.contains("a machine that can take this Space")
        || m.contains("not enough CPU free")
        // A fork of a checkpoint that no machine has room for now.
        || m.contains("no machine can take it")
}

impl CuaBackend {
    async fn create_once(
        &self,
        who: &Identity,
        spec: CreateSpec,
        extras: CreateExtras,
    ) -> Result<Created> {
        let c = self.client(who).await?;
        let p = &spec.place;
        // `None`: the core default placement (the user's `default.on`, else
        // this machine; the same resolution `cua spaces create` and the SDK
        // use), not a word of this layer's.
        let on: Option<String> =
            p.on.clone()
                .or_else(|| Some(self.defaults.on.clone()).filter(|o| !o.is_empty()));
        // A request that names its own image takes that image's kind
        // (`auto`) unless a `cua.kind` label says otherwise: a container
        // image such as `python:3.12` has no VM variant.
        let own_image = matches!(&spec.source, Source::Image(Some(i)) if !i.is_empty());
        let kind = p.kind.clone().unwrap_or_else(|| {
            if own_image {
                "auto".into()
            } else {
                kind_on(&self.defaults.kind, on.as_deref().unwrap_or(""))
            }
        });
        let cpus = p.cpus.or(self.defaults.cpus);
        let memory_mb = p.memory_mb.or(self.defaults.memory_mb);
        let mut disk_applied = true;
        let name = match &spec.source {
            Source::Checkpoint(cp) => {
                // A checkpoint this machine holds, forked here: the sandbox
                // API (copy-on-write RAM, no registry round trip).
                let here = matches!(on.as_deref(), None | Some("local"));
                let local = if here {
                    match c.sandboxes().fork(cp.clone(), spec.name.clone()).await {
                        Ok(sb) => Some(sb),
                        // Not here: another machine of yours holds it (a
                        // copy comes here when `on` says local).
                        Err(CuaError::NotFound(_)) => None,
                        Err(e) => return Err(e.into()),
                    }
                } else {
                    None
                };
                match local {
                    Some(sb) => {
                        let name = sb.name();
                        self.handles
                            .lock()
                            .await
                            .insert((who.clone(), name.clone()), sb);
                        name
                    }
                    None => {
                        // The core starts it where `on` says: unset, where
                        // the checkpoint is; `host:<machine>` or `auto`, a
                        // machine that lacks it gets a verified copy first.
                        // A fork keeps the checkpoint's guest, so no
                        // environment; its kind is the checkpoint's.
                        let created = c
                            .spaces()
                            .create(SpaceCreateOptions {
                                image: Some(format!("checkpoint:{cp}")),
                                on: on.clone(),
                                kind: p.kind.clone(),
                                name: Some(spec.name.clone()),
                                cpus,
                                memory_mb,
                                wait: Some(true),
                                ..Default::default()
                            })
                            .await?;
                        let info = created
                            .space
                            .ok_or_else(|| BackendError::Other("the fork did not start".into()))?;
                        self.spaces
                            .lock()
                            .await
                            .insert(info.name.clone(), info.id.clone());
                        info.name
                    }
                }
            }
            Source::Image(image) => {
                let image = image
                    .clone()
                    .filter(|i| !i.is_empty())
                    .or_else(|| Some(self.defaults.image.clone()).filter(|i| !i.is_empty()));
                let on = place_local_disk(image.as_deref(), on)?;
                if spec.internet
                    && extras.build.is_none()
                    && extras.pool.is_none()
                    && self.defaults.overlays.is_empty()
                {
                    let make = |disk_gb: Option<u32>| SpaceCreateOptions {
                        image: image.clone(),
                        on: on.clone(),
                        kind: Some(kind.clone()),
                        name: Some(spec.name.clone()),
                        cpus,
                        memory_mb,
                        disk_gb,
                        gpu: extras.gpu.clone(),
                        command: extras.command.clone(),
                        env: guest_env(&spec.env, p.desktop),
                        // Provider sandboxes are snapshotted and forked by
                        // the hundred: a forkable VM's checkpoint shares
                        // its RAM with every fork. Other kinds ignore it.
                        forkable: Some(true),
                        wait: Some(true),
                        ..Default::default()
                    };
                    let created = match c.spaces().create(make(extras.disk_gb)).await {
                        // A container shares its engine's disk: there is no
                        // size to give it. Daytona's `disk` then is what the
                        // sandbox reports (the guest's filesystem, read at
                        // create), not a cap; a VM image gets the size.
                        Err(e)
                            if extras.disk_gb.is_some() && e.to_string().contains("disk size") =>
                        {
                            tracing::info!(
                                name = spec.name,
                                "disk size not applied to a container Space: {e}"
                            );
                            disk_applied = false;
                            c.spaces().create(make(None)).await?
                        }
                        r => r?,
                    };
                    let info = created
                        .space
                        .ok_or_else(|| BackendError::Other("the Space did not start".into()))?;
                    self.spaces
                        .lock()
                        .await
                        .insert(info.name.clone(), info.id.clone());
                    info.name
                } else {
                    // No outbound network (local QEMU VMs, `network = "none"`)
                    // or image layers: the Sandboxes API.
                    if extras.disk_gb.is_some() && extras.build.is_some() {
                        return Err(BackendError::Unsupported(
                            "a disk size and image layers cannot be combined".into(),
                        ));
                    }
                    let image = image
                        .unwrap_or_else(|| cua_image::canonical::canonical_image_or_linux("linux"));
                    let mut o = SandboxCreateOptions::auto(image);
                    o.on = on;
                    o.kind = Some(kind);
                    o.name = Some(spec.name.clone());
                    o.cpus = cpus;
                    o.memory_mb = memory_mb;
                    o.env = guest_env(&spec.env, p.desktop);
                    o.gpu = extras.gpu.clone();
                    o.command = extras.command.clone();
                    // Checkpoints share RAM with their forks (see above).
                    o.forkable = Some(true);
                    o.pool = extras.pool.clone();
                    o.overlays = self.defaults.overlays.clone();
                    o.build = extras.build.clone();
                    if !spec.internet {
                        o.network = Some("none".into());
                    }
                    let sb = c.sandboxes().create(o).await?;
                    let name = sb.name();
                    self.handles
                        .lock()
                        .await
                        .insert((who.clone(), name.clone()), sb);
                    name
                }
            }
        };
        Ok(Created {
            name,
            cpus: cpus.unwrap_or(2),
            memory_mb: memory_mb.unwrap_or(2048),
            disk_mb: extras
                .disk_gb
                .filter(|_| disk_applied)
                .map_or(0, |g| u64::from(g) * 1024),
        })
    }
}

/// The guest environment of a new sandbox: the caller's, plus no desktop
/// unless `desktop` (an explicit `CUA_ENV_NO_DESKTOP` always wins).
fn guest_env(env: &HashMap<String, String>, desktop: bool) -> HashMap<String, String> {
    let mut env = env.clone();
    if !desktop {
        env.entry("CUA_ENV_NO_DESKTOP".into())
            .or_insert_with(|| "true".into());
    }
    env
}

#[cfg(test)]
mod place_tests {
    use super::*;

    #[test]
    fn disk_images_stay_on_this_machine() {
        let here = std::env::current_exe().unwrap();
        let img = format!("vm:{}", here.display());
        assert_eq!(
            place_local_disk(Some(&img), Some("auto".into())).unwrap(),
            Some("local".into())
        );
        assert_eq!(
            place_local_disk(Some(&img), Some("local".into())).unwrap(),
            Some("local".into())
        );
        assert_eq!(
            place_local_disk(Some(&img), None).unwrap(),
            Some("local".into())
        );
        assert!(place_local_disk(Some(&img), Some("host:kvm-a".into())).is_err());
        assert!(place_local_disk(Some("vm:/nope/disk.img"), Some("auto".into())).is_err());
        assert_eq!(
            place_local_disk(Some("ghcr.io/x/y"), Some("host:kvm-a".into())).unwrap(),
            Some("host:kvm-a".into())
        );
        assert_eq!(
            place_local_disk(None, Some("auto".into())).unwrap(),
            Some("auto".into())
        );
        assert_eq!(place_local_disk(None, None).unwrap(), None);
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;

    #[test]
    fn provider_sandboxes_boot_without_a_desktop_unless_asked() {
        let none = HashMap::new();
        assert_eq!(guest_env(&none, false)["CUA_ENV_NO_DESKTOP"], "true");
        assert!(!guest_env(&none, true).contains_key("CUA_ENV_NO_DESKTOP"));
        let mut own = HashMap::new();
        own.insert("CUA_ENV_NO_DESKTOP".to_string(), "false".to_string());
        assert_eq!(guest_env(&own, false)["CUA_ENV_NO_DESKTOP"], "false");
    }
}

#[async_trait]
impl Backend for CuaBackend {
    fn forks_fast(&self, place: &Placement) -> bool {
        let on = place.on.as_deref().unwrap_or(&self.defaults.on);
        let kind = place
            .kind
            .clone()
            .unwrap_or_else(|| kind_on(&self.defaults.kind, on));
        let kind = kind.as_str();
        // A checkpoint lives on the machine that took it: this one only.
        // Containers too: a gVisor checkpoint restores the process memory,
        // and a fork starts in about a second where a cold container is
        // seconds of CPU each (64 at once exhaust a small host).
        matches!(kind, "vm" | "microvm" | "container") && matches!(on, "" | "local")
    }

    async fn volume_ls(&self, who: &Identity, path: &str) -> Result<Vec<VolumeEntry>> {
        let c = self.client(who).await?;
        let listing = c.spaces().volume_ls(Some(path.to_string()), None).await?;
        Ok(listing
            .entries
            .into_iter()
            .map(|e| VolumeEntry {
                path: e.path,
                name: e.name,
                folder: e.folder,
                size: e.size,
                modified_ms: e.modified_ms,
            })
            .collect())
    }

    async fn volume_read(&self, who: &Identity, path: &str) -> Result<Vec<u8>> {
        let c = self.client(who).await?;
        Ok(c.spaces()
            .volume_read(path.to_string(), None, None)
            .await?
            .content)
    }

    async fn volume_write(&self, who: &Identity, path: &str, data: Vec<u8>) -> Result<()> {
        let c = self.client(who).await?;
        c.spaces()
            .volume_write(path.to_string(), data, None, false, None)
            .await?;
        Ok(())
    }

    async fn volume_delete(&self, who: &Identity, path: &str) -> Result<()> {
        let c = self.client(who).await?;
        c.spaces()
            .volume_delete(path.to_string(), None, None)
            .await?;
        Ok(())
    }

    async fn create(&self, who: &Identity, spec: CreateSpec) -> Result<Created> {
        self.create_with(who, spec, CreateExtras::default()).await
    }

    async fn create_with(
        &self,
        who: &Identity,
        spec: CreateSpec,
        extras: CreateExtras,
    ) -> Result<Created> {
        // A provider SDK never sets CPUs and expects a create to queue when
        // the machines are busy, not to fail: `on=auto` refuses while every
        // machine is loaded ("a machine that can take this Space"). Wait for
        // capacity (bounded) instead of surfacing that as "not found".
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        let mut delay = std::time::Duration::from_secs(2);
        loop {
            match self.create_once(who, spec.clone(), extras.clone()).await {
                Err(BackendError::NotFound(m))
                | Err(BackendError::Unsupported(m))
                | Err(BackendError::Other(m))
                    if is_no_capacity(&m) && std::time::Instant::now() < deadline =>
                {
                    tracing::info!("compat: waiting for capacity: {m}");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(20));
                }
                r => return r,
            }
        }
    }

    async fn set_lifetime(
        &self,
        who: &Identity,
        name: &str,
        seconds: u64,
        on_expire: &str,
    ) -> Result<bool> {
        // The Cua daemon (on the machine that holds the Space) enforces the
        // deadline whether or not this gateway is running.
        if let Some(id) = self.space_id(who, name).await {
            let c = self.client(who).await?;
            c.spaces()
                .set_lifetime(id, seconds, Some(on_expire.to_string()))
                .await?;
        } else {
            let secs = u32::try_from(seconds).unwrap_or(u32::MAX);
            self.handle(who, name)
                .await?
                .set_lifetime(secs, Some(on_expire.to_string()))
                .await?;
        }
        Ok(true)
    }

    async fn network_blocked(&self, who: &Identity, name: &str) -> Result<bool> {
        // Core says where the policy is enforced: by the host, outside the
        // guest. That is the control; the probe below is defence in depth
        // for sandboxes core does not report on.
        if let Some(id) = self.space_id(who, name).await
            && let Ok(info) = self.client(who).await?.spaces().resolve(id).await
            && enforced_by_host(info.egress_enforced)
        {
            return Ok(true);
        }
        let c = self.spacesd(who, name).await?;
        let probe = cua_spacesd_client::Command::new("sh").args([
            "-c",
            "command -v bash >/dev/null 2>&1 || { echo unknown; exit 0; }; \
             if timeout 3 bash -c 'exec 3<>/dev/tcp/1.1.1.1/53' 2>/dev/null; then echo open; else echo closed; fi",
        ]);
        match c.run(probe).await {
            Ok(o) => Ok(!String::from_utf8_lossy(&o.stdout).contains("open")),
            Err(_) => Ok(true),
        }
    }

    async fn cancel_create(&self, who: &Identity, name: &str) -> Result<bool> {
        let c = self.client(who).await?;
        let spaces = c.spaces().cancel_create(name.to_string()).await;
        if let Ok(o) = &spaces
            && o.state != "not_creating"
        {
            return Ok(true);
        }
        Ok(c.sandboxes()
            .cancel_create(name.to_string())
            .await?
            .is_some())
    }

    async fn delete(&self, who: &Identity, name: &str) -> Result<bool> {
        let c = self.client(who).await?;
        self.forget(who, name).await;
        let id = self.space_id(who, name).await;
        self.spaces.lock().await.remove(name);
        let r = match id {
            Some(id) => c.spaces().delete(id).await.map(|_| ()),
            None => c.sandboxes().delete(name.to_string()).await,
        };
        match r {
            Ok(()) => Ok(true),
            Err(CuaError::NotFound(_)) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    async fn liveness(&self, who: &Identity, name: &str) -> Result<Liveness> {
        let c = self.client(who).await?;
        if let Some(id) = self.space_id(who, name).await {
            return match c.spaces().resolve(id).await {
                Ok(_) => Ok(Liveness::Running),
                Err(CuaError::NotFound(_)) => Ok(Liveness::Gone),
                Err(e) => Err(e.into()),
            };
        }
        match c.sandboxes().get(name.to_string()).await {
            Ok(info) => Ok(match info.status {
                // A suspended QEMU VM on Linux reports `stopped` (its state
                // is on disk, `resume` starts it): resumable is paused.
                cua_sdk::SandboxStatus::Suspended | cua_sdk::SandboxStatus::Stopped => {
                    Liveness::Suspended
                }
                _ => Liveness::Running,
            }),
            Err(CuaError::NotFound(_)) => Ok(Liveness::Gone),
            Err(e) => Err(e.into()),
        }
    }

    async fn spacesd(&self, who: &Identity, name: &str) -> Result<SpacesdClient> {
        if let Some(id) = self.space_id(who, name).await {
            let c = self.client(who).await?;
            let space = c.spaces().space(id).await?;
            return Ok(space.spacesd()?.inner().clone());
        }
        let sb = self.handle(who, name).await?;
        Ok(sb.spacesd(None).await?.inner().clone())
    }

    async fn forward(&self, who: &Identity, name: &str, port: u16) -> Result<String> {
        if let Some(f) = self.forwards.lock().await.get(&(name.to_string(), port)) {
            return Ok(f.addr());
        }
        let env = self.spacesd(who, name).await?;
        let fwd = match env
            .forward_tcp(cua_spacesd_client::ForwardOptions::new(port))
            .await
        {
            Ok(t) => {
                let addr = t.local_addr().to_string();
                Forward::Tunnel(Arc::new(t), addr)
            }
            Err(cua_spacesd_client::Error::FeatureUnsupported { .. }) => {
                // An older cua-spacesd: the sandbox's own forward.
                let sb = self.handle(who, name).await?;
                let f = sb.forward(port).await?;
                let addr = f
                    .local_addr()
                    .ok_or_else(|| BackendError::Other("the forward has no address".into()))?;
                Forward::Sandbox(f, addr)
            }
            Err(e) => return Err(BackendError::Other(e.to_string())),
        };
        let mut fwds = self.forwards.lock().await;
        let entry = fwds.entry((name.to_string(), port)).or_insert(fwd);
        Ok(entry.addr())
    }

    async fn suspend(&self, who: &Identity, name: &str) -> Result<()> {
        Ok(self.handle(who, name).await?.suspend().await?)
    }

    async fn resume(&self, who: &Identity, name: &str) -> Result<()> {
        Ok(self.handle(who, name).await?.resume().await?)
    }

    async fn restart(&self, who: &Identity, name: &str) -> Result<()> {
        Ok(self.handle(who, name).await?.restart().await?)
    }

    async fn resize(&self, who: &Identity, name: &str, shape: Shape) -> Result<()> {
        self.handle(who, name)
            .await?
            .resize(shape.cpus, shape.memory_mb, shape.disk_gb)
            .await?;
        // The VM booted again on new host ports. A Space keeps the connection
        // it opened before; turning it off and on drops that connection, and
        // the next one finds the new address.
        if let Some(id) = self.space_id(who, name).await {
            let spaces = self.client(who).await?.spaces();
            spaces.stop(id.clone()).await?;
            spaces.start(id).await?;
        }
        // The guest answers once its driver has read its token, a moment
        // after the VM reports running.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let last = match self.spacesd(who, name).await {
                Ok(c) => match c.run(cua_spacesd_client::Command::new("/bin/true")).await {
                    Ok(_) => return Ok(()),
                    Err(e) => e.to_string(),
                },
                Err(e) => e.to_string(),
            };
            if std::time::Instant::now() > deadline {
                return Err(BackendError::Other(format!(
                    "the resized VM did not answer within 90 s: {last}"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    async fn pool_create(
        &self,
        who: &Identity,
        name: &str,
        image: &str,
        min: u32,
        place: &Placement,
    ) -> Result<()> {
        let c = self.client(who).await?;
        c.sandboxes()
            .create_pool(
                image.to_string(),
                cua_sdk::PoolCreateOptions {
                    name: Some(name.to_string()),
                    kind: place.kind.clone(),
                    cpus: place.cpus,
                    memory_mb: place.memory_mb,
                    min,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

    async fn pool_ready(&self, who: &Identity, name: &str) -> Result<Option<u32>> {
        let c = self.client(who).await?;
        match c.sandboxes().get_pool(name.to_string()).await {
            Ok(p) => Ok(Some(p.ready)),
            Err(CuaError::NotFound(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn pool_delete(&self, who: &Identity, name: &str) -> Result<bool> {
        let c = self.client(who).await?;
        Ok(c.sandboxes().delete_pool(name.to_string()).await?)
    }

    async fn check_image(&self, _who: &Identity, image: &str) -> Result<()> {
        let image = image.to_string();
        tokio::task::spawn_blocking(move || {
            cua_sdk::resolve_image(image, "auto".into(), None).map(|_| ())
        })
        .await
        .map_err(|e| BackendError::Other(e.to_string()))?
        .map_err(BackendError::from)
    }

    async fn placement_of(&self, who: &Identity, name: &str) -> Result<Option<String>> {
        let Some(id) = self.space_id(who, name).await else {
            return Ok(None);
        };
        let info = self.client(who).await?.spaces().resolve(id).await?;
        Ok(if !info.host.is_empty() {
            Some(format!("host:{}", info.host))
        } else if info.provider == "local" {
            Some("local".into())
        } else {
            None
        })
    }

    async fn checkpoint(&self, who: &Identity, name: &str, checkpoint: &str) -> Result<()> {
        let c = self.client(who).await?;
        c.sandboxes()
            .checkpoint(name.to_string(), checkpoint.to_string(), false)
            .await?;
        Ok(())
    }

    async fn checkpoints(&self, who: &Identity) -> Result<Vec<CheckpointInfo>> {
        let c = self.client(who).await?;
        Ok(c.sandboxes()
            .checkpoints()
            .await?
            .into_iter()
            .map(|s| CheckpointInfo {
                name: s.name,
                source: s.source,
            })
            .collect())
    }

    async fn delete_checkpoint(&self, who: &Identity, checkpoint: &str) -> Result<bool> {
        let c = self.client(who).await?;
        match c
            .sandboxes()
            .delete_checkpoint(checkpoint.to_string())
            .await
        {
            Ok(()) => Ok(true),
            Err(CuaError::NotFound(_)) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// A backend over in-process mock cua-spacesd servers (feature `testing`):
/// processes are simulated, nothing runs on the host.
#[cfg(any(test, feature = "testing"))]
pub mod mock {
    use super::*;
    use cua_spacesd_client::testing::{MockAuth, MockServer};

    struct Sb {
        server: MockServer,
        suspended: bool,
    }

    /// A mock checkpoint: its source sandbox and the files it holds.
    type MockCheckpoint = (String, Vec<(String, Vec<u8>)>);

    /// Mock Spaces. `ports` maps a guest port to a host address the test
    /// serves (the stand-in for a guest server behind a forward).
    #[derive(Default)]
    pub struct MockBackend {
        sandboxes: Mutex<HashMap<String, Sb>>,
        checkpoints: Mutex<HashMap<String, MockCheckpoint>>,
        pub ports: Mutex<HashMap<u16, String>>,
        /// Every create, for assertions.
        pub created: Mutex<Vec<CreateSpec>>,
        /// How long a create takes (milliseconds), to model a busy host.
        pub create_delay_ms: std::sync::atomic::AtomicU64,
        /// Whether forks are fast ([`Backend::forks_fast`]): turns on the
        /// booted base that creates fork from.
        pub fast_forks: std::sync::atomic::AtomicBool,
        /// The mock's sandboxes have outbound network although they were
        /// created without (a host that ignores `network = none`).
        pub leaky_network: std::sync::atomic::AtomicBool,
        /// A refusal the next create returns (a core capability error).
        pub create_fails: Mutex<Option<BackendError>>,
        /// Every cancelled create, for assertions.
        pub cancelled: Mutex<Vec<String>>,
        /// Every delete, for assertions.
        pub deleted: Mutex<Vec<String>>,
        /// Every resize the mock accepted.
        pub resized: Mutex<Vec<(String, Shape)>>,
        /// The extras of every create, in the same order.
        pub extras: Mutex<Vec<CreateExtras>>,
        /// Pools made: name -> (image, min).
        pub pools: Mutex<HashMap<String, (String, u32)>>,
        /// Cua-side lifetimes: name -> (deadline, action), enforced by
        /// [`MockBackend::reap_expired`] the way the daemon would.
        pub lifetimes: Mutex<HashMap<String, (std::time::Instant, String)>>,
        /// A stand-in for Cua Volume: key -> content.
        pub volume: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    }

    impl MockBackend {
        pub fn new() -> Self {
            Self::default()
        }

        /// The daemon's reaper tick: ends every Space whose lifetime passed.
        pub async fn reap_expired(&self) -> usize {
            let due: Vec<(String, String)> = {
                let l = self.lifetimes.lock().await;
                l.iter()
                    .filter(|(_, (t, _))| *t <= std::time::Instant::now())
                    .map(|(n, (_, a))| (n.clone(), a.clone()))
                    .collect()
            };
            for (n, action) in &due {
                self.lifetimes.lock().await.remove(n);
                if action == "suspend" {
                    if let Some(sb) = self.sandboxes.lock().await.get_mut(n) {
                        sb.suspended = true;
                    }
                } else {
                    self.sandboxes.lock().await.remove(n);
                }
            }
            due.len()
        }

        /// The names of the Spaces that exist (the leak-check inventory).
        pub async fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = self.sandboxes.lock().await.keys().cloned().collect();
            v.sort();
            v
        }

        /// The state of the mock Space `name` (what it observed).
        pub async fn state(
            &self,
            name: &str,
        ) -> Option<Arc<cua_spacesd_client::testing::MockState>> {
            self.sandboxes
                .lock()
                .await
                .get(name)
                .map(|s| s.server.state.clone())
        }

        /// Adds a checkpoint this gateway did not take (one baked on
        /// another machine of the pool, as the core lists it), taken of
        /// `source`, holding no files.
        pub async fn add_checkpoint(&self, name: &str, source: &str) {
            self.checkpoints
                .lock()
                .await
                .insert(name.to_string(), (source.to_string(), vec![]));
        }

        /// The mock server behind a sandbox (to inspect its files).
        pub async fn file(&self, name: &str, path: &str) -> Option<Vec<u8>> {
            self.sandboxes
                .lock()
                .await
                .get(name)
                .and_then(|s| s.server.state.file(path))
        }
    }

    #[async_trait]
    impl Backend for MockBackend {
        async fn network_blocked(&self, _who: &Identity, _name: &str) -> Result<bool> {
            Ok(!self
                .leaky_network
                .load(std::sync::atomic::Ordering::Relaxed))
        }
        async fn cancel_create(&self, _who: &Identity, name: &str) -> Result<bool> {
            self.cancelled.lock().await.push(name.to_string());
            Ok(true)
        }
        fn forks_fast(&self, _place: &Placement) -> bool {
            self.fast_forks.load(std::sync::atomic::Ordering::Relaxed)
        }

        async fn volume_ls(&self, _who: &Identity, path: &str) -> Result<Vec<VolumeEntry>> {
            let prefix = if path.is_empty() || path.ends_with('/') {
                path.to_string()
            } else {
                format!("{path}/")
            };
            let vol = self.volume.lock().await;
            let mut out: std::collections::BTreeMap<String, VolumeEntry> = Default::default();
            for (k, v) in vol.iter() {
                let Some(rest) = k.strip_prefix(&prefix) else {
                    continue;
                };
                match rest.split_once('/') {
                    Some((dir, _)) => {
                        out.entry(dir.to_string()).or_insert_with(|| VolumeEntry {
                            path: format!("{prefix}{dir}/"),
                            name: dir.to_string(),
                            folder: true,
                            size: 0,
                            modified_ms: 0,
                        });
                    }
                    None => {
                        out.insert(
                            rest.to_string(),
                            VolumeEntry {
                                path: k.clone(),
                                name: rest.to_string(),
                                folder: false,
                                size: v.len() as u64,
                                modified_ms: 0,
                            },
                        );
                    }
                }
            }
            Ok(out.into_values().collect())
        }

        async fn volume_read(&self, _who: &Identity, path: &str) -> Result<Vec<u8>> {
            self.volume
                .lock()
                .await
                .get(path)
                .cloned()
                .ok_or_else(|| BackendError::NotFound(format!("{path} not found")))
        }

        async fn volume_write(&self, _who: &Identity, path: &str, data: Vec<u8>) -> Result<()> {
            self.volume.lock().await.insert(path.to_string(), data);
            Ok(())
        }

        async fn volume_delete(&self, _who: &Identity, path: &str) -> Result<()> {
            self.volume
                .lock()
                .await
                .remove(path)
                .map(|_| ())
                .ok_or_else(|| BackendError::NotFound(format!("{path} not found")))
        }

        async fn create(&self, who: &Identity, spec: CreateSpec) -> Result<Created> {
            self.create_with(who, spec, CreateExtras::default()).await
        }

        async fn create_with(
            &self,
            _who: &Identity,
            spec: CreateSpec,
            extras: CreateExtras,
        ) -> Result<Created> {
            if let Some(e) = self.create_fails.lock().await.clone() {
                return Err(e);
            }
            self.created.lock().await.push(spec.clone());
            self.extras.lock().await.push(extras.clone());
            let delay = self
                .create_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            let server = MockServer::start(MockAuth::default()).await;
            if let Source::Checkpoint(cp) = &spec.source {
                let files = self
                    .checkpoints
                    .lock()
                    .await
                    .get(cp)
                    .map(|(_, files)| files.clone())
                    .ok_or_else(|| BackendError::NotFound(format!("checkpoint {cp} not found")))?;
                for (p, d) in files {
                    server.state.put_file(&p, d);
                }
            }
            self.sandboxes.lock().await.insert(
                spec.name.clone(),
                Sb {
                    server,
                    suspended: false,
                },
            );
            Ok(Created {
                name: spec.name,
                cpus: spec.place.cpus.unwrap_or(2),
                memory_mb: spec.place.memory_mb.unwrap_or(2048),
                disk_mb: extras.disk_gb.map_or(0, |g| u64::from(g) * 1024),
            })
        }

        async fn delete(&self, _who: &Identity, name: &str) -> Result<bool> {
            self.deleted.lock().await.push(name.to_string());
            Ok(self.sandboxes.lock().await.remove(name).is_some())
        }

        async fn pool_create(
            &self,
            _who: &Identity,
            name: &str,
            image: &str,
            min: u32,
            _place: &Placement,
        ) -> Result<()> {
            self.pools
                .lock()
                .await
                .insert(name.to_string(), (image.to_string(), min));
            Ok(())
        }

        async fn pool_ready(&self, _who: &Identity, name: &str) -> Result<Option<u32>> {
            Ok(self.pools.lock().await.get(name).map(|(_, m)| *m))
        }

        async fn pool_delete(&self, _who: &Identity, name: &str) -> Result<bool> {
            Ok(self.pools.lock().await.remove(name).is_some())
        }

        async fn set_lifetime(
            &self,
            _who: &Identity,
            name: &str,
            seconds: u64,
            on_expire: &str,
        ) -> Result<bool> {
            if !self.sandboxes.lock().await.contains_key(name) {
                return Err(BackendError::NotFound(format!("sandbox {name} not found")));
            }
            self.lifetimes.lock().await.insert(
                name.to_string(),
                (
                    std::time::Instant::now() + std::time::Duration::from_secs(seconds),
                    on_expire.to_string(),
                ),
            );
            Ok(true)
        }

        async fn liveness(&self, _who: &Identity, name: &str) -> Result<Liveness> {
            Ok(match self.sandboxes.lock().await.get(name) {
                None => Liveness::Gone,
                Some(s) if s.suspended => Liveness::Suspended,
                Some(_) => Liveness::Running,
            })
        }

        async fn spacesd(&self, _who: &Identity, name: &str) -> Result<SpacesdClient> {
            let url = {
                let sbs = self.sandboxes.lock().await;
                let sb = sbs
                    .get(name)
                    .ok_or_else(|| BackendError::NotFound(format!("sandbox {name} not found")))?;
                if sb.suspended {
                    return Err(BackendError::Other(format!("sandbox {name} is suspended")));
                }
                sb.server.url()
            };
            SpacesdClient::connect_url(&url, None)
                .await
                .map_err(|e| BackendError::Other(e.to_string()))
        }

        async fn forward(&self, _who: &Identity, name: &str, port: u16) -> Result<String> {
            if !self.sandboxes.lock().await.contains_key(name) {
                return Err(BackendError::NotFound(format!("sandbox {name} not found")));
            }
            self.ports
                .lock()
                .await
                .get(&port)
                .cloned()
                .ok_or_else(|| BackendError::Other(format!("nothing listens on port {port}")))
        }

        async fn suspend(&self, _who: &Identity, name: &str) -> Result<()> {
            let mut sbs = self.sandboxes.lock().await;
            let sb = sbs
                .get_mut(name)
                .ok_or_else(|| BackendError::NotFound(format!("sandbox {name} not found")))?;
            sb.suspended = true;
            Ok(())
        }

        async fn resume(&self, _who: &Identity, name: &str) -> Result<()> {
            let mut sbs = self.sandboxes.lock().await;
            let sb = sbs
                .get_mut(name)
                .ok_or_else(|| BackendError::NotFound(format!("sandbox {name} not found")))?;
            sb.suspended = false;
            Ok(())
        }

        async fn resize(&self, _who: &Identity, name: &str, shape: Shape) -> Result<()> {
            if !self.sandboxes.lock().await.contains_key(name) {
                return Err(BackendError::NotFound(format!("sandbox {name} not found")));
            }
            self.resized.lock().await.push((name.to_string(), shape));
            Ok(())
        }

        async fn restart(&self, _who: &Identity, name: &str) -> Result<()> {
            if self.sandboxes.lock().await.contains_key(name) {
                Ok(())
            } else {
                Err(BackendError::NotFound(format!("sandbox {name} not found")))
            }
        }

        async fn checkpoint(&self, _who: &Identity, name: &str, checkpoint: &str) -> Result<()> {
            let files = {
                let sbs = self.sandboxes.lock().await;
                let sb = sbs
                    .get(name)
                    .ok_or_else(|| BackendError::NotFound(format!("sandbox {name} not found")))?;
                sb.server.state.files()
            };
            self.checkpoints
                .lock()
                .await
                .insert(checkpoint.to_string(), (name.to_string(), files));
            Ok(())
        }

        async fn checkpoints(&self, _who: &Identity) -> Result<Vec<CheckpointInfo>> {
            Ok(self
                .checkpoints
                .lock()
                .await
                .iter()
                .map(|(name, (source, _))| CheckpointInfo {
                    name: name.clone(),
                    source: source.clone(),
                })
                .collect())
        }

        async fn delete_checkpoint(&self, _who: &Identity, checkpoint: &str) -> Result<bool> {
            Ok(self.checkpoints.lock().await.remove(checkpoint).is_some())
        }
    }
}

#[cfg(test)]
mod default_tests {
    use super::*;
    use cua_sandbox_core::placement::HostFacts;

    #[test]
    fn local_containers_and_vms_fork_from_a_booted_base_elsewhere_does_not() {
        let b = CuaBackend::new(SpaceDefaults::default());
        let p = |on: Option<&str>, kind: Option<&str>| Placement {
            on: on.map(String::from),
            kind: kind.map(String::from),
            ..Default::default()
        };
        assert!(b.forks_fast(&p(None, Some("container"))));
        assert!(b.forks_fast(&p(Some("local"), Some("vm"))));
        assert!(!b.forks_fast(&p(Some("aws"), Some("container"))));
        assert!(!b.forks_fast(&p(Some("host:kvm-a"), Some("vm"))));
    }

    #[test]
    fn only_host_enforcement_counts_as_the_control() {
        assert!(enforced_by_host(Some(cua_sdk::EgressEnforcement::Host)));
        // Not reported (an older host or daemon): the probe decides.
        assert!(!enforced_by_host(None));
    }

    #[test]
    fn a_plan_limit_error_is_rate_limited_with_its_sentence_verbatim() {
        let wrapped = CuaError::Internal(
            "rpc failed: You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro. (resource exhausted)"
                .into(),
        );
        match BackendError::from(wrapped) {
            BackendError::RateLimited(m) => assert_eq!(
                m,
                "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro."
            ),
            other => panic!("{other:?}"),
        }
        // Anything else is not rate limiting.
        assert!(!matches!(
            BackendError::from(CuaError::Internal("boom".into())),
            BackendError::RateLimited(_)
        ));
    }

    #[test]
    fn core_capability_refusals_are_unsupported_not_internal() {
        for e in [
            CuaError::CapabilityMissing("network none: this host cannot enforce it".into()),
            CuaError::HostCapabilityMissing("outdated daemon".into()),
        ] {
            assert!(matches!(
                BackendError::from(e),
                BackendError::Unsupported(_)
            ));
        }
    }

    #[test]
    fn kind_follows_what_the_host_can_accelerate() {
        let none = HostFacts::default();
        let kvm = HostFacts {
            kvm: true,
            ..Default::default()
        };
        assert_eq!(kind_for(kvm), "vm");
        // Without KVM a Linux host runs containers; a Mac always has HVF.
        assert_eq!(
            kind_for(none),
            if cfg!(target_os = "macos") {
                "vm"
            } else {
                "container"
            }
        );
        let d = SpaceDefaults::default();
        assert_eq!(
            d.on, "",
            "omitted is the core default, not a word of this layer"
        );
        assert_eq!(d.kind, "", "the kind is chosen per placement");
        assert_eq!(kind_on(&d.kind, ""), default_kind());
        assert_eq!(kind_on(&d.kind, "local"), default_kind());
        // Anywhere else a container, not a KVM host bought for one sandbox.
        assert_eq!(kind_on(&d.kind, "aws"), "container");
        assert_eq!(kind_on(&d.kind, "host:kvm-a"), "container");
        assert_eq!(kind_on("vm", "aws"), "vm", "asked for is honoured");
    }
}
