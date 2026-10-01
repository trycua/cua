//! Spaces on your machines: a machine set up with `cua host setup
//! --provide-spaces` creates Spaces for the owner's other devices, through
//! the relay. "Spin up 2 macOS Spaces on my spare Mac mini" is
//! `create_space(on="host:spare mac mini", image="macos")`, twice.
//!
//! Two halves:
//!
//! - **Client** ([`Spaces::create`] with [`On::Host`], [`Spaces::delete`]
//!   of a Space a host provides, [`Spaces::host_spaces`]): resolves the
//!   machine by id or name ([`cua_host::match_machine`]; an ambiguous name
//!   returns the choices), registers a relay machine for the new Space as
//!   the signed-in account from this enrolled device (with `host` set, so
//!   lists group it under the host), and asks the host's
//!   `HostSpacesService` to create the Space and attach it to that machine.
//!   The Space is then `relay:<machine>`, like any machine of the account,
//!   and per-agent computer access applies to it as to any other machine.
//! - **Host** ([`HostSpacesServer`], served by the host's cua daemon behind
//!   its cua-spacesd): checks the host settings and the caller, keeps the
//!   capacity limits (two macOS VMs per Mac, by Apple's license; the host's
//!   Space limit), creates the Space with this machine's runtimes (Lume for
//!   macOS, Docker for Linux), attaches it to the relay
//!   (`SystemService.AttachRelay`, the second half of
//!   [`Spaces::relay_register`]), and records every create, delete and
//!   refusal in `~/.cua/host/spaces-audit.jsonl`.
//!
//! The host never holds an account credential: the client registers each
//! Space's relay machine and hands the host only that machine's token.
//!
//! **Without the relay.** A machine set up with `cua host setup --direct
//! <ip:port> --provide-spaces` (reached by its Tailscale or LAN address)
//! serves the same `HostSpacesService` on its direct listener to the holder
//! of its env token. A laptop adds it with `cua spaces add <addr> --host
//! --token <token>` (a [`crate::registry::DirectHost`]), and
//! [`Spaces::resolve_host`] falls back to those hosts when the relay does
//! not know the name. A Space created there is not attached to the relay:
//! the host forwards a port on its own address to the Space's cua-spacesd,
//! and the client adds it as `direct:<host address>:<port>` with the
//! Space's token. The host takes those connections (and host calls) only
//! from loopback, Tailscale and private LAN addresses unless set up with
//! `--allow-any-address`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_host::provided::{self, ProvidedSpace};
use cua_host::{DirectHosting, Host, MachineMatch, RelayClient};
use cua_proto::env::v1 as pb;
use cua_sandbox_core::placement::{Kind, On, Runtime};
use serde::Deserialize;

use crate::registry::DirectHost;
use crate::share::RELAY_ATTACH_FEATURE;
use crate::{Error, Result, SpaceCreate, SpaceId, SpaceInfo, Spaces};

/// The spacesd feature a host reports while it provides Spaces.
pub const HOST_SPACES_FEATURE: &str = "host_spaces";

/// Metadata carrying the relay-verified caller from the host's driver to
/// its daemon (set by cua-spacesd; absent for a call made on the host
/// itself).
pub const CALLER_METADATA: &str = "x-cua-host-caller";

/// How long a client waits for a new Space to come online on the relay.
const ONLINE_WAIT: Duration = Duration::from_secs(60);

/// Who asked the host for a Space.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct HostCaller {
    /// Account id, or `local` for a call made on the host itself.
    pub account: String,
    /// Verified email, when the relay shares it.
    #[serde(default)]
    pub email: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// `owner` or `shared` (an editor of the host).
    #[serde(default)]
    pub role: String,
    /// `relay` or `token`.
    #[serde(default)]
    pub via: String,
}

impl HostCaller {
    /// A caller on this machine (its own app or CLI): the owner.
    pub fn local() -> Self {
        Self {
            account: "local".into(),
            role: "owner".into(),
            via: "token".into(),
            ..Default::default()
        }
    }

    /// The caller in [`CALLER_METADATA`]; `None` (a direct call to this
    /// machine's daemon) is [`HostCaller::local`].
    pub fn from_metadata(value: Option<&str>) -> Result<Self> {
        match value {
            None => Ok(Self::local()),
            Some(v) => serde_json::from_str(v)
                .map_err(|e| Error::invalid(format!("{CALLER_METADATA}: {e}"))),
        }
    }

    /// `Name <email> (account)` for the audit and people.
    pub fn label(&self) -> String {
        if self.account == "local" {
            return "local".into();
        }
        let mut s = String::new();
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            s.push_str(n);
            s.push(' ');
        }
        if let Some(e) = self.email.as_deref().filter(|e| !e.is_empty()) {
            s.push_str(&format!("<{e}> "));
        }
        s.push_str(&format!("({})", self.account));
        s
    }

    /// The host's owner (or someone on the host itself).
    pub fn is_owner(&self) -> bool {
        self.role == "owner"
    }

    /// Owner or editor: may create Spaces on the host.
    pub fn may_create(&self) -> bool {
        matches!(self.role.as_str(), "owner" | "shared")
    }
}

/// The relay URL a Space on this host dials: the relay the client named,
/// except that a relay on this machine's loopback (a self-hosted or test
/// relay) is reached from a guest at the host's address as the guest sees
/// it (`host.docker.internal` from Docker Desktop and Colima containers,
/// the Apple Virtualization NAT gateway from a Lume VM).
pub fn guest_relay_url(relay_url: &str, kind: &str) -> String {
    let Ok(mut url) = url::Url::parse(relay_url) else {
        return relay_url.to_string();
    };
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if !loopback {
        return relay_url.to_string();
    }
    let host = if kind == "vm" && cfg!(target_os = "macos") {
        "192.168.64.1"
    } else {
        "host.docker.internal"
    };
    if url.set_host(Some(host)).is_err() {
        return relay_url.to_string();
    }
    url.to_string().trim_end_matches('/').to_string()
}

/// `image` as the host runs it: an alias (`macos`, `macos:26`, `linux`)
/// resolved to its reference, else the reference as given; and the guest
/// OS when known.
fn resolve_image(image: &str) -> (String, String) {
    let word = image.trim();
    if word.is_empty() {
        return (cua_fleet::canonical_image("linux"), "linux".into());
    }
    if let Some((reference, os)) =
        cua_image::canonical::alias_with(word, &|n| std::env::var(n).ok())
    {
        return (reference, os.as_str().to_string());
    }
    let os = if provided::is_macos_image(word) {
        "macos"
    } else {
        ""
    };
    (word.to_string(), os.into())
}

fn to_pb(s: &ProvidedSpace) -> pb::HostSpace {
    pb::HostSpace {
        relay_machine: s.relay_machine.clone(),
        local_space: s.local_space.clone(),
        name: s.name.clone(),
        image: s.image.clone(),
        os: s.os.clone(),
        kind: s.kind.clone(),
        runtime: s.runtime.clone(),
        created_by: s.created_by.clone(),
        created_at_ms: s.created_at_ms as i64,
        direct_port: u32::from(s.direct_port),
        // Only the create response carries a direct Space's token.
        direct_token: String::new(),
    }
}

#[derive(Default)]
struct Reserved {
    spaces: u32,
    macos: u32,
}

/// Holds a capacity slot until the Space is recorded (or the create fails).
struct Slot<'a> {
    reserved: &'a Mutex<Reserved>,
    macos: bool,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut r = self.reserved.lock().expect("reserved");
        r.spaces = r.spaces.saturating_sub(1);
        if self.macos {
            r.macos = r.macos.saturating_sub(1);
        }
    }
}

/// The host half: what this machine's cua daemon does for
/// `HostSpacesService` calls.
pub struct HostSpacesServer {
    spaces: Spaces,
    reserved: Mutex<Reserved>,
    /// The ports this host forwards to the Spaces it provides directly,
    /// by the Space's `local:` id.
    forwards: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl Drop for HostSpacesServer {
    fn drop(&mut self) {
        if let Ok(forwards) = self.forwards.get_mut() {
            for task in forwards.values() {
                task.abort();
            }
        }
    }
}

impl HostSpacesServer {
    /// Serves Spaces from `spaces` (the daemon's runtime; its home is the
    /// cua home whose `host/` holds the settings).
    pub fn new(spaces: Spaces) -> Arc<Self> {
        Arc::new(Self {
            spaces,
            reserved: Mutex::default(),
            forwards: Mutex::default(),
        })
    }

    /// Opens the forwarded port of every Space this host provides directly
    /// that is not open yet (after the daemon started again). The daemon
    /// calls it when it starts; `GetHostSpaces` and turning a Space on do
    /// too. Nothing for a host that is not in direct mode.
    pub async fn restore_direct_forwards(&self) {
        let Ok(policy) = self.policy() else {
            return;
        };
        let Some(direct) = policy.direct.as_ref().filter(|_| policy.provide_spaces) else {
            return;
        };
        let Ok(all) = provided::load_provided(&self.host_dir()) else {
            return;
        };
        for record in all.iter().filter(|s| s.is_direct()) {
            if self.forward_open(&record.local_space) {
                continue;
            }
            if let Err(e) = self
                .open_forward(direct, &record.local_space, record.direct_port)
                .await
            {
                tracing::warn!(
                    space = %record.local_space,
                    port = record.direct_port,
                    "the forwarded port of a direct Space did not open again: {e}"
                );
            }
        }
    }

    fn forward_open(&self, local_space: &str) -> bool {
        self.forwards
            .lock()
            .expect("forwards")
            .get(local_space)
            .is_some_and(|t| !t.is_finished())
    }

    fn close_forward(&self, local_space: &str) {
        if let Some(task) = self.forwards.lock().expect("forwards").remove(local_space) {
            task.abort();
        }
    }

    /// Forwards `port` (0: any free one) on the host's direct address to
    /// the cua-spacesd of its local Space `local_space`, and returns the
    /// port. Each connection looks the Space up again (a VM's address can
    /// change when it boots again); one from outside loopback, Tailscale
    /// and private LAN addresses is dropped unless the host allows any.
    async fn open_forward(
        &self,
        direct: &DirectHosting,
        local_space: &str,
        port: u16,
    ) -> Result<u16> {
        let listen: std::net::SocketAddr = direct.listen.parse().map_err(|_| {
            Error::invalid(format!(
                "this host's direct listen address {:?} is not ip:port; run `cua host setup --direct` again",
                direct.listen
            ))
        })?;
        let SpaceId::Local { name } = SpaceId::parse(local_space)? else {
            return Err(Error::invalid(format!(
                "{local_space} is not a local Space"
            )));
        };
        let listener =
            tokio::net::TcpListener::bind(std::net::SocketAddr::new(listen.ip(), port)).await?;
        let bound = listener.local_addr()?.port();
        let task = tokio::spawn(forward_loop(
            listener,
            self.spaces.sandboxes().clone(),
            name,
            direct.allow_any_address,
        ));
        if let Some(old) = self
            .forwards
            .lock()
            .expect("forwards")
            .insert(local_space.to_string(), task)
        {
            old.abort();
        }
        Ok(bound)
    }

    /// The cua-spacesd token of this host's local Space `local_space`. A
    /// Space without one is never forwarded.
    async fn space_token(&self, local_space: &str) -> Result<String> {
        if let Some(t) = self
            .spaces
            .registry()
            .credential(local_space)?
            .and_then(|c| c.token)
            .filter(|t| !t.is_empty())
        {
            return Ok(t);
        }
        let SpaceId::Local { name } = SpaceId::parse(local_space)? else {
            return Err(Error::invalid(format!(
                "{local_space} is not a local Space"
            )));
        };
        let sandbox = self.spaces.sandboxes().connect(&name).await?;
        if sandbox.env_token().is_none() {
            // A bootstrap-mode spacesd: installs and records one.
            let _ = sandbox.spacesd().await;
        }
        sandbox
            .env_token()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{local_space} has no cua-spacesd token, so it is not reachable directly"
                ))
            })
    }

    fn host(&self) -> Host {
        Host::new(self.spaces.home_dir())
    }

    fn host_dir(&self) -> std::path::PathBuf {
        self.host().paths().dir.clone()
    }

    fn audit(&self, action: &str, who: &HostCaller, space: &str, detail: &str) {
        if let Err(e) = provided::audit(&self.host_dir(), action, &who.label(), space, detail) {
            tracing::warn!("host spaces audit: {e}");
        }
    }

    /// Refuses with `err`, recording why.
    fn refuse(&self, who: &HostCaller, space: &str, err: Error) -> Error {
        self.audit("refused", who, space, &err.to_string());
        err
    }

    fn policy(&self) -> Result<cua_host::HostPolicy> {
        self.host().policy()?.ok_or_else(|| {
            Error::host(
                "host spaces",
                "this machine is not set up to provide Spaces (run `cua host setup --provide-spaces` on it)",
            )
        })
    }

    /// macOS VMs running on this Mac now: the ones it provides, and the
    /// owner's own local macOS Spaces (Apple's limit is per Mac).
    fn macos_in_use(&self, provided: &[ProvidedSpace]) -> u32 {
        let mut ids: std::collections::BTreeSet<String> = provided
            .iter()
            .filter(|s| s.is_macos())
            .map(|s| s.local_space.clone())
            .collect();
        if let Ok(records) = self.spaces.registry().list() {
            for r in records {
                let local = matches!(SpaceId::parse(&r.id), Ok(SpaceId::Local { .. }));
                if local && (r.os == "macos" || provided::is_macos_image(&r.image)) {
                    ids.insert(r.id.clone());
                }
            }
        }
        ids.len() as u32
    }

    fn capacity(
        &self,
        policy: &cua_host::HostPolicy,
        provided: &[ProvidedSpace],
    ) -> Vec<pb::HostSpacesCapacity> {
        let settings = policy.spaces_settings();
        let r = self.reserved.lock().expect("reserved");
        let mut out = vec![pb::HostSpacesCapacity {
            resource: "spaces".into(),
            used: provided.len() as u32 + r.spaces,
            limit: settings.max_spaces,
            reason: "this host's limit (`cua host config --max-spaces`)".into(),
        }];
        if cfg!(target_os = "macos") {
            out.push(pb::HostSpacesCapacity {
                resource: "macos_vms".into(),
                used: self.macos_in_use(provided) + r.macos,
                limit: settings.max_macos_vms,
                reason: provided::MACOS_LIMIT_REASON.into(),
            });
        }
        out
    }

    /// Reserves a slot for one more Space, or says which limit is full.
    fn reserve(
        &self,
        policy: &cua_host::HostPolicy,
        provided: &[ProvidedSpace],
        macos: bool,
        host_name: &str,
    ) -> Result<Slot<'_>> {
        let settings = policy.spaces_settings();
        let macos_used = if macos {
            self.macos_in_use(provided)
        } else {
            0
        };
        let mut r = self.reserved.lock().expect("reserved");
        let used = provided.len() as u32 + r.spaces;
        if settings.max_spaces > 0 && used >= settings.max_spaces {
            return Err(Error::LimitExceeded(format!(
                "{host_name} already provides {used} Spaces, its limit (on it: `cua host config \
                 --max-spaces N`); delete one first"
            )));
        }
        if macos {
            let in_use = macos_used + r.macos;
            if in_use >= settings.max_macos_vms {
                return Err(Error::LimitExceeded(format!(
                    "{host_name} already runs {in_use} macOS VM{} and allows {}: {}. Delete one \
                     (delete_space) or create a Linux Space instead",
                    if in_use == 1 { "" } else { "s" },
                    settings.max_macos_vms,
                    provided::MACOS_LIMIT_REASON
                )));
            }
            r.macos += 1;
        }
        r.spaces += 1;
        Ok(Slot {
            reserved: &self.reserved,
            macos,
        })
    }

    fn host_name(&self) -> String {
        self.host()
            .config()
            .ok()
            .flatten()
            .map(|c| c.name)
            .unwrap_or_else(cua_host::device_name)
    }

    /// `GetHostSpaces`: settings, capacity, the caller's Spaces (all of
    /// them for the owner) and, for the owner, the recent audit.
    pub async fn get(&self, caller: &HostCaller) -> Result<pb::GetHostSpacesResponse> {
        let policy = self.policy()?;
        // A direct host's driver asks when it starts: the forwarded ports
        // open again.
        self.restore_direct_forwards().await;
        let all = provided::load_provided(&self.host_dir())?;
        let settings = policy.spaces_settings();
        let mut images = vec!["linux".to_string()];
        if cfg!(target_os = "macos") {
            images.push("macos".into());
        }
        Ok(pb::GetHostSpacesResponse {
            name: self.host_name(),
            os: std::env::consts::OS.into(),
            settings: Some(pb::HostSpacesSettings {
                share_desktop: settings.share_desktop,
                provide_spaces: settings.provide_spaces,
                max_spaces: settings.max_spaces,
                max_macos_vms: if cfg!(target_os = "macos") {
                    settings.max_macos_vms
                } else {
                    0
                },
            }),
            capacity: self.capacity(&policy, &all),
            spaces: all
                .iter()
                .filter(|s| caller.is_owner() || s.created_by_account == caller.account)
                .map(to_pb)
                .collect(),
            audit: if caller.is_owner() {
                provided::read_audit(&self.host_dir(), 50)
                    .recent
                    .into_iter()
                    .map(|e| pb::HostSpacesAuditEvent {
                        ts_ms: e.at_ms as i64,
                        action: e.action,
                        who: e.who,
                        space: e.space,
                        detail: e.detail,
                    })
                    .collect()
            } else {
                vec![]
            },
            images,
        })
    }

    /// `CreateHostSpace`: creates one Space here and attaches it to the
    /// relay machine the caller registered for it.
    pub async fn create(
        &self,
        caller: &HostCaller,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace> {
        // Host Spaces use: the kind and guest family only, never the name,
        // image, caller or machine.
        let kind = req.kind.clone();
        let image = req.image.to_ascii_lowercase();
        // Empty is the canonical Linux image.
        let guest = ["macos", "windows", "android", "linux"]
            .into_iter()
            .find(|os| image.contains(os))
            .unwrap_or(if image.is_empty() { "linux" } else { "unknown" });
        let r = self.create_provided(caller, req).await;
        cua_telemetry::capture(cua_telemetry::events::host_space_provided(
            &kind,
            guest,
            if r.is_ok() {
                cua_telemetry::Outcome::Ok
            } else {
                cua_telemetry::Outcome::Error
            },
            r.is_err().then_some("other"),
        ));
        r
    }

    async fn create_provided(
        &self,
        caller: &HostCaller,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace> {
        let attach = req.attach.clone().unwrap_or_default();
        let direct = req.direct;
        let label = if direct || attach.machine_id.is_empty() {
            req.name.clone()
        } else {
            attach.machine_id.clone()
        };
        let policy = self.policy().map_err(|e| self.refuse(caller, &label, e))?;
        if !policy.provide_spaces {
            return Err(self.refuse(
                caller,
                &label,
                Error::host(
                    "host spaces",
                    "this machine does not provide Spaces (on it: `cua host config --provide-spaces on`)",
                ),
            ));
        }
        if !caller.may_create() {
            return Err(self.refuse(
                caller,
                &label,
                Error::Relay(cua_host::Error::PermissionDenied(
                    "only the owner and the accounts this machine is shared with (as editors) can create Spaces on it"
                        .into(),
                )),
            ));
        }
        let direct_hosting = match (direct, policy.direct.clone()) {
            (true, Some(d)) => Some(d),
            (true, None) => {
                return Err(self.refuse(
                    caller,
                    &label,
                    Error::host(
                        "host spaces",
                        "this machine provides Spaces through the relay, not directly (on it: \
                         `cua host setup --direct <ip:port> --provide-spaces`)",
                    ),
                ));
            }
            (false, _) => None,
        };
        if direct && !attach.machine_id.is_empty() {
            return Err(self.refuse(
                caller,
                &label,
                Error::invalid("a direct Space is not attached to the relay; leave attach empty"),
            ));
        }
        if !direct
            && (attach.machine_id.is_empty()
                || attach.machine_token.is_empty()
                || attach.relay_url.is_empty())
        {
            return Err(self.refuse(
                caller,
                &label,
                Error::invalid(
                    "attach needs the relay URL, machine id and machine token the caller registered",
                ),
            ));
        }
        let (image, os) = resolve_image(&req.image);
        let macos = os == "macos";
        let host_name = self.host_name();
        if macos && !cfg!(target_os = "macos") {
            return Err(self.refuse(
                caller,
                &label,
                Error::host(
                    "macOS VMs",
                    format!(
                        "{host_name} runs {}; macOS Spaces need a Mac host (Apple Virtualization with Lume)",
                        std::env::consts::OS
                    ),
                ),
            ));
        }
        let place = |e: cua_sandbox_core::placement::PlacementError| {
            Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(e))
        };
        let kind = Kind::parse(&req.kind).map_err(place)?;
        let runtime = Runtime::parse(&req.runtime).map_err(place)?;
        let provided_now = provided::load_provided(&self.host_dir())?;
        let slot = self
            .reserve(&policy, &provided_now, macos, &host_name)
            .map_err(|e| self.refuse(caller, &label, e))?;
        let created = self
            .spaces
            .create(SpaceCreate {
                image: Some(image.clone()),
                on: Some(On::Local),
                kind,
                runtime: runtime.clone(),
                name: Some(req.name.clone()).filter(|n| !n.trim().is_empty()),
                cpus: Some(req.cpus).filter(|c| *c > 0),
                memory_mb: Some(req.memory_mb).filter(|m| *m > 0),
                disk_gb: Some(req.disk_gb).filter(|d| *d > 0),
                wait: Some(true),
                // A provided Space joins the relay through its own driver
                // (a direct one is reached through the host's forward).
                spacesd: Some(true),
                // The caller cancels it by the machine it registered.
                create_id: (!direct).then(|| attach.machine_id.clone()),
                ..Default::default()
            })
            .await;
        let info = match created {
            Ok(c) => match c.ready() {
                Ok(info) => info,
                Err(p) => {
                    return Err(self.refuse(
                        caller,
                        &label,
                        Error::invalid(format!("{} is still starting", p.id)),
                    ));
                }
            },
            Err(e) => {
                self.audit("failed", caller, &label, &format!("create {image}: {e}"));
                return Err(e);
            }
        };
        // Without the relay: a port on the host's direct address forwarded
        // to the new Space's cua-spacesd, and its token for the caller.
        if let Some(d) = direct_hosting {
            let opened = async {
                let token = self.space_token(&info.id).await?;
                let port = self.open_forward(&d, &info.id, 0).await?;
                Ok::<_, Error>((token, port))
            }
            .await;
            let (token, port) = match opened {
                Ok(v) => v,
                Err(e) => {
                    self.close_forward(&info.id);
                    let _ = self.spaces.delete(&info.id).await;
                    self.audit(
                        "failed",
                        caller,
                        &label,
                        &format!("forward a port to {}: {e}", info.id),
                    );
                    return Err(e);
                }
            };
            let record = ProvidedSpace {
                relay_machine: String::new(),
                local_space: info.id.clone(),
                name: info.name.clone(),
                image: image.clone(),
                os: if info.os.is_empty() {
                    os
                } else {
                    info.os.clone()
                },
                kind: info.kind.clone(),
                runtime: runtime.as_str().to_string(),
                created_by: caller.label(),
                created_by_account: caller.account.clone(),
                created_at_ms: now_ms(),
                machine_token: String::new(),
                relay_url: String::new(),
                direct_port: port,
            };
            let mut all = provided::load_provided(&self.host_dir())?;
            all.push(record.clone());
            provided::save_provided(&self.host_dir(), &all)?;
            drop(slot);
            self.audit(
                "create",
                caller,
                &record.local_space,
                &format!("{image}, {}, direct on port {port}", record.kind),
            );
            let mut out = to_pb(&record);
            out.direct_token = token;
            return Ok(out);
        }
        // Attach the new Space's own driver to the relay machine the caller
        // registered (the second half of `relay_register`).
        let attached = async {
            let s = self.spaces.space(&info.id).await?;
            s.require(RELAY_ATTACH_FEATURE)?;
            s.spacesd()?
                .system()
                .attach_relay(pb::AttachRelayRequest {
                    relay_url: guest_relay_url(&attach.relay_url, &info.kind),
                    ..attach.clone()
                })
                .await
                .map_err(|e| Error::Env(cua_spacesd_client::Error::from(e)))?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(e) = attached {
            let _ = self.spaces.delete(&info.id).await;
            self.audit(
                "failed",
                caller,
                &label,
                &format!("attach {} to the relay: {e}", info.id),
            );
            return Err(e);
        }
        let record = ProvidedSpace {
            relay_machine: attach.machine_id.clone(),
            local_space: info.id.clone(),
            name: info.name.clone(),
            image: image.clone(),
            os: if info.os.is_empty() {
                os
            } else {
                info.os.clone()
            },
            kind: info.kind.clone(),
            runtime: runtime.as_str().to_string(),
            created_by: caller.label(),
            created_by_account: caller.account.clone(),
            created_at_ms: now_ms(),
            machine_token: attach.machine_token.clone(),
            relay_url: attach.relay_url.clone(),
            direct_port: 0,
        };
        let mut all = provided::load_provided(&self.host_dir())?;
        all.push(record.clone());
        provided::save_provided(&self.host_dir(), &all)?;
        drop(slot);
        self.audit(
            "create",
            caller,
            &record.relay_machine,
            &format!("{} ({image}, {})", record.local_space, record.kind),
        );
        Ok(to_pb(&record))
    }

    /// `CancelHostSpace`: stops a create a caller started here (by the
    /// relay machine it registered), removing what it made; a Space that
    /// already finished is deleted as `DeleteHostSpace` would. Idempotent.
    pub async fn cancel(&self, caller: &HostCaller, space: &str) -> Result<String> {
        let machine = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        if !caller.may_create() {
            return Err(self.refuse(
                caller,
                &machine,
                Error::Relay(cua_host::Error::PermissionDenied(
                    "only the owner and the accounts this machine is shared with (as editors) can cancel creates on it"
                        .into(),
                )),
            ));
        }
        let outcome = self.spaces.cancel_create(&machine).await?;
        let provided = provided::load_provided(&self.host_dir())?
            .into_iter()
            .any(|s| s.relay_machine == machine);
        if provided {
            // It finished before the cancel reached it.
            return self.delete(caller, &machine).await;
        }
        self.audit("cancel", caller, &machine, &outcome.message);
        Ok(outcome.message)
    }

    /// `DeleteCloudSpace`: deletes a Space in the owner's own cloud that
    /// this machine created (by the relay machine it joined as): its cloud
    /// resources through this machine's ownership records, and its relay
    /// machine. Only the owner; audited.
    pub async fn delete_cloud(&self, caller: &HostCaller, space: &str) -> Result<String> {
        let machine = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        // Only the owner: the cloud account is theirs, and it costs money.
        if !caller.is_owner() {
            return Err(self.refuse(
                caller,
                &machine,
                Error::Relay(cua_host::Error::PermissionDenied(
                    "only the owner can delete a Space in their own cloud".into(),
                )),
            ));
        }
        let id = format!("relay:{machine}");
        if self.spaces.cloud_sandbox_of(&id)?.is_none() {
            return Err(self.refuse(
                caller,
                &machine,
                Error::NotFound(format!(
                    "{id} is not a Space in your cloud that this machine created"
                )),
            ));
        }
        match self.spaces.delete(&id).await {
            Ok(message) => {
                self.audit("delete-cloud", caller, &machine, &message);
                Ok(message)
            }
            Err(e) => {
                self.audit("failed", caller, &machine, &format!("delete-cloud: {e}"));
                Err(e)
            }
        }
    }

    /// `DeleteHostSpace`: deletes a Space this host provides (its sandbox
    /// and its relay machine). The owner deletes any; another account only
    /// the ones it created.
    pub async fn delete(&self, caller: &HostCaller, space: &str) -> Result<String> {
        let space = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        let mut all = provided::load_provided(&self.host_dir())?;
        let Some(i) = all.iter().position(|s| {
            !space.is_empty() && (s.relay_machine == space || s.local_space == space)
        }) else {
            return Err(self.refuse(
                caller,
                &space,
                Error::NotFound(format!("{space} is not a Space this host provides")),
            ));
        };
        let record = all[i].clone();
        if !caller.is_owner() && record.created_by_account != caller.account {
            return Err(self.refuse(
                caller,
                &space,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{space} was created by someone else; only they or the host's owner can delete it"
                ))),
            ));
        }
        // Recorded as this caller's delete first, so the local delete below
        // (which forgets any provided Space it removes) finds nothing left.
        all.remove(i);
        provided::save_provided(&self.host_dir(), &all)?;
        self.close_forward(&record.local_space);
        match self.spaces.delete(&record.local_space).await {
            Ok(_) | Err(Error::NotFound(_)) => {}
            Err(e) => {
                // Still running: keep providing it.
                if let Ok(mut now) = provided::load_provided(&self.host_dir()) {
                    now.push(record.clone());
                    let _ = provided::save_provided(&self.host_dir(), &now);
                }
                self.audit("failed", caller, &space, &format!("delete: {e}"));
                return Err(e);
            }
        }
        // The Space's machine token removes its relay machine (best effort:
        // the client removes it with the account too).
        if !record.machine_token.is_empty()
            && !record.relay_url.is_empty()
            && let Ok(client) = RelayClient::new(&record.relay_url)
        {
            let _ = client
                .delete(&record.machine_token, &record.relay_machine)
                .await;
        }
        if record.is_direct() {
            self.audit(
                "delete",
                caller,
                &record.local_space,
                &format!("direct on port {}", record.direct_port),
            );
            return Ok(format!(
                "Deleted {} on {} (it was forwarded on port {}).",
                record.local_space,
                self.host_name(),
                record.direct_port
            ));
        }
        self.audit("delete", caller, &record.relay_machine, &record.local_space);
        Ok(format!(
            "Deleted relay:{} ({} on {}).",
            record.relay_machine,
            record.local_space,
            self.host_name()
        ))
    }

    /// `SetHostSpacePower`: turns a Space this host provides off or on.
    /// Off stops it with its disk kept, so it frees the host's memory (a
    /// QEMU VM, which cannot boot again from its state, is suspended
    /// instead); on boots it again and attaches it to its relay machine
    /// again, with the machine token the host keeps. The owner powers any;
    /// another account only the ones it created.
    pub async fn set_power(
        &self,
        caller: &HostCaller,
        space: &str,
        on: bool,
    ) -> Result<pb::SetHostSpacePowerResponse> {
        let space = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        let Some(record) = provided::load_provided(&self.host_dir())?
            .into_iter()
            .find(|s| !space.is_empty() && (s.relay_machine == space || s.local_space == space))
        else {
            return Err(self.refuse(
                caller,
                &space,
                Error::NotFound(format!("{space} is not a Space this host provides")),
            ));
        };
        if !caller.is_owner() && record.created_by_account != caller.account {
            return Err(self.refuse(
                caller,
                &space,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{space} was created by someone else; only they or the host's owner can turn it off or on"
                ))),
            ));
        }
        let id = SpaceId::parse(&record.local_space)?;
        let SpaceId::Local { name } = &id else {
            return Err(Error::invalid(format!(
                "{} is not a local Space",
                record.local_space
            )));
        };
        let sandboxes = self.spaces.sandboxes();
        let Some(own) = sandboxes.power_control(name) else {
            // The core's refusal names the runtime that cannot.
            let e = sandboxes
                .power_off(name)
                .await
                .err()
                .map(Error::from)
                .unwrap_or_else(|| Error::invalid("no power control"));
            return Err(self.refuse(caller, &space, e));
        };
        let control = match sandboxes.get(name).await.map(|i| i.runtime_type) {
            Ok(runtime) if runtime == "qemu" => own,
            _ => cua_sandbox_core::PowerControl::Stop,
        };
        self.spaces.drop_connection(&id).await;
        let label = record.label();
        // What the audit names it by: its relay machine, or for a direct
        // Space its id on this host.
        let subject = if record.is_direct() {
            record.local_space.clone()
        } else {
            record.relay_machine.clone()
        };
        let state = if on {
            let started = async {
                sandboxes.power_on(name).await?;
                if record.is_direct() {
                    // Reached through this host's forward again (each
                    // connection finds the Space's new address).
                    self.ensure_forward(&record).await
                } else {
                    self.reattach(&record).await
                }
            }
            .await;
            if let Err(e) = started {
                self.audit("failed", caller, &subject, &format!("start: {e}"));
                return Err(e);
            }
            cua_sandbox_core::PowerState::Running
        } else {
            match sandboxes.power_off_as(name, control).await {
                Ok(state) => state,
                Err(e) => {
                    let e = Error::from(e);
                    self.audit("failed", caller, &subject, &format!("stop: {e}"));
                    return Err(e);
                }
            }
        };
        let report = crate::SpacePower::new(&label, control, state);
        self.audit(
            if on { "start" } else { "stop" },
            caller,
            &subject,
            &record.local_space,
        );
        Ok(pb::SetHostSpacePowerResponse {
            state: report.state,
            power: report.power,
            message: format!(
                "{} ({} on {})",
                report.message.trim_end_matches('.'),
                record.local_space,
                self.host_name()
            ),
        })
    }

    /// The forwarded port of a direct Space this host provides, opened
    /// again on its recorded port when it is not open.
    async fn ensure_forward(&self, record: &ProvidedSpace) -> Result<()> {
        if self.forward_open(&record.local_space) {
            return Ok(());
        }
        let direct = self.policy()?.direct.ok_or_else(|| {
            Error::host(
                "host spaces",
                format!(
                    "{} is a direct Space, but this host is no longer set up with --direct; delete it",
                    record.local_space
                ),
            )
        })?;
        self.open_forward(&direct, &record.local_space, record.direct_port)
            .await
            .map(|_| ())
    }

    /// Attaches a provided Space that booted again to its relay machine:
    /// its driver forgot the attachment when it stopped. The relay's keys
    /// and the machine's owner are read again (the keys may have rotated).
    async fn reattach(&self, record: &ProvidedSpace) -> Result<()> {
        if record.machine_token.is_empty() || record.relay_url.is_empty() {
            return Err(Error::invalid(format!(
                "relay:{} has no machine token on this host, so it cannot rejoin the relay; delete it and create a new one",
                record.relay_machine
            )));
        }
        let client = RelayClient::new(&record.relay_url)?;
        let jwks = client.jwks().await?;
        let machine = client
            .machine(&record.machine_token, &record.relay_machine)
            .await?;
        let s = self.spaces.space(&record.local_space).await?;
        s.require(RELAY_ATTACH_FEATURE)?;
        s.spacesd()?
            .system()
            .attach_relay(pb::AttachRelayRequest {
                relay_url: guest_relay_url(&record.relay_url, &record.kind),
                machine_token: record.machine_token.clone(),
                machine_id: record.relay_machine.clone(),
                relay_jwks_json: jwks.to_string(),
                owner: machine.owner.id.clone(),
                owner_email: machine.owner.email.clone().unwrap_or_default(),
            })
            .await
            .map_err(|e| Error::Env(cua_spacesd_client::Error::from(e)))?;
        Ok(())
    }
}

/// Accepts connections on a direct Space's forwarded port and splices each
/// to the Space's cua-spacesd, looked up per connection.
async fn forward_loop(
    listener: tokio::net::TcpListener,
    sandboxes: cua_sandbox_core::Sandboxes,
    name: String,
    allow_any_address: bool,
) {
    loop {
        let (mut inbound, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::debug!(error = %e, space = %name, "direct Space forward: accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        if !allow_any_address && !cua_host::direct::is_private_address(peer.ip()) {
            tracing::warn!(
                %peer,
                space = %name,
                "refused a connection to a direct Space from outside loopback, Tailscale and the LAN"
            );
            continue;
        }
        let sandboxes = sandboxes.clone();
        let name = name.clone();
        tokio::spawn(async move {
            let target = match spacesd_target(&sandboxes, &name).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::debug!(error = %e, space = %name, "direct Space forward: no target");
                    return;
                }
            };
            if let Ok(mut outbound) = tokio::net::TcpStream::connect(&target).await {
                let _ = inbound.set_nodelay(true);
                let _ = outbound.set_nodelay(true);
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            }
        });
    }
}

/// Where this host reaches the cua-spacesd of its local Space `name` now.
async fn spacesd_target(sandboxes: &cua_sandbox_core::Sandboxes, name: &str) -> Result<String> {
    let sandbox = sandboxes.connect(name).await?;
    let port = sandbox
        .services()
        .get("env")
        .copied()
        .filter(|p| *p != 0)
        .unwrap_or(cua_proto::SPACESD_DEFAULT_PORT);
    match sandbox.port(port)? {
        cua_sandbox_core::PortTarget::Addr { host, port } => Ok(crate::id::authority(&host, port)),
        cua_sandbox_core::PortTarget::Url(url) => Err(Error::invalid(format!(
            "{name}'s cua-spacesd is at {url}, not a TCP address this host can forward"
        ))),
    }
}

/// After `space` (a `local:` id) was deleted on this machine: when it was a
/// Space this machine provided, its relay machine is removed (with the
/// Space's own machine token) and it leaves the host's list, audited as a
/// local delete. Best effort; nothing when it was not provided.
/// Asks host `host` (a relay machine id) to cancel the Space it is
/// creating for relay machine `machine` ([`HostSpacesServer::cancel`]).
pub(crate) async fn cancel_on_host(spaces: &Spaces, host: &str, machine: &str) -> Result<String> {
    let relay = spaces.relay()?;
    let token = relay.tokens.access_token().await?;
    let client = relay.client().await?;
    let row = client.machine(&token, host).await?;
    let space = spaces.host_space(&row).await?;
    space
        .spacesd()?
        .host_spaces()
        .cancel_host_space(pb::CancelHostSpaceRequest {
            space: machine.to_string(),
        })
        .await
        .map(|r| r.into_inner().message)
        .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
}

pub(crate) async fn forget_provided(spaces: &Spaces, space: &str) {
    let dir = Host::new(spaces.home_dir()).paths().dir.clone();
    let Ok(mut all) = provided::load_provided(&dir) else {
        return;
    };
    let Some(i) = all.iter().position(|s| s.local_space == space) else {
        return;
    };
    let record = all.remove(i);
    if !record.machine_token.is_empty()
        && let Ok(client) = RelayClient::new(&record.relay_url)
    {
        let _ = client
            .delete(&record.machine_token, &record.relay_machine)
            .await;
    }
    if let Err(e) = provided::save_provided(&dir, &all) {
        tracing::warn!("provided spaces: {e}");
    }
    let _ = provided::audit(&dir, "delete", "local", &record.relay_machine, space);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A `tonic::Status` carrying a `cua.env.v1.ErrorInfo` for `e`, so the
/// client decodes the same kind (the daemon's `HostSpacesService`).
pub fn to_status(e: &Error) -> tonic::Status {
    use prost::Message as _;
    let (code, reason) = match e.tag() {
        "invalid_argument" | "invalid_placement" => {
            (tonic::Code::InvalidArgument, pb::ErrorReason::Unspecified)
        }
        "not_found" => (tonic::Code::NotFound, pb::ErrorReason::Unspecified),
        "permission_denied" => (
            tonic::Code::PermissionDenied,
            pb::ErrorReason::PermissionDenied,
        ),
        "limit_exceeded" => (
            tonic::Code::ResourceExhausted,
            pb::ErrorReason::LimitExceeded,
        ),
        "host_capability_missing" | "capability_missing" => (
            tonic::Code::FailedPrecondition,
            pb::ErrorReason::FeatureUnsupported,
        ),
        "unauthenticated" => (
            tonic::Code::Unauthenticated,
            pb::ErrorReason::Unauthenticated,
        ),
        "timeout" => (tonic::Code::DeadlineExceeded, pb::ErrorReason::Unspecified),
        _ => (tonic::Code::Internal, pb::ErrorReason::Internal),
    };
    let message = e.to_string();
    let info = pb::ErrorInfo {
        reason: reason as i32,
        message: message.clone(),
        feature: String::new(),
        metadata: [("kind".to_string(), e.tag().to_string())].into(),
    };
    let rpc = cua_spacesd_client::error::RpcStatus {
        code: code as i32,
        message: message.clone(),
        details: vec![cua_proto::wkt::Any {
            type_url: "type.googleapis.com/cua.env.v1.ErrorInfo".into(),
            value: info.encode_to_vec().into(),
        }],
    };
    tonic::Status::with_details(code, message, rpc.encode_to_vec().into())
}

/// A host's error, as this crate's kind (the `kind` the host put in the
/// error's metadata, else the spacesd error).
pub(crate) fn from_host(e: cua_spacesd_client::Error) -> Error {
    use cua_spacesd_client::Error as E;
    let kind = match &e {
        E::LimitExceeded(d)
        | E::PermissionDenied(d)
        | E::Internal(d)
        | E::Rpc(d)
        | E::NotInitialized(d) => d.metadata.get("kind").cloned(),
        E::FeatureUnsupported { details, .. } => details.metadata.get("kind").cloned(),
        _ => None,
    };
    let msg = match &e {
        E::LimitExceeded(d) | E::PermissionDenied(d) | E::Internal(d) | E::Rpc(d) => {
            d.message.clone()
        }
        E::FeatureUnsupported { details, .. } => details.message.clone(),
        other => other.to_string(),
    };
    match (kind.as_deref(), &e) {
        (Some("limit_exceeded"), _) | (_, E::LimitExceeded(_)) => Error::LimitExceeded(msg),
        (Some("permission_denied"), _) | (_, E::PermissionDenied(_)) => {
            Error::Relay(cua_host::Error::PermissionDenied(msg))
        }
        (Some("host_capability_missing"), _) | (_, E::FeatureUnsupported { .. }) => {
            Error::host("host spaces", msg)
        }
        (Some("not_found"), _) => Error::NotFound(msg),
        (Some("invalid_argument"), _) => Error::invalid(msg),
        _ => Error::Env(e),
    }
}

/// A machine that provides Spaces, as [`Spaces::resolve_host`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // resolved once per call, never stored in bulk
pub enum ResolvedHost {
    /// One of the account's machines on the relay.
    Relay(crate::relay::RelayMachine),
    /// A machine added with `cua spaces add <addr> --host`, reached at its
    /// direct (Tailscale or LAN) address without the relay.
    Direct(DirectHost),
}

impl ResolvedHost {
    /// `relay` or `direct`: how the host is reached.
    pub fn via(&self) -> &'static str {
        match self {
            Self::Relay(_) => "relay",
            Self::Direct(_) => "direct",
        }
    }

    /// Its display name (a relay machine without one: its id).
    pub fn name(&self) -> String {
        match self {
            Self::Relay(m) if m.name.is_empty() => m.id.clone(),
            Self::Relay(m) => m.name.clone(),
            Self::Direct(h) => h.name.clone(),
        }
    }
}

/// How a direct host takes part in matching: its name is its id.
fn direct_as_machine(h: &DirectHost) -> cua_host::Machine {
    cua_host::Machine {
        id: h.name.clone(),
        name: h.name.clone(),
        online: true,
        ..Default::default()
    }
}

fn ambiguous_host(query: &str, choices: &[cua_host::Machine]) -> Error {
    Error::AmbiguousHost {
        query: query.to_string(),
        choices: choices.iter().map(|m| format!("host:{}", m.id)).collect(),
        message: format!(
            "{query:?} matches several of your machines: {}; say which (on=\"host:<id>\")",
            provided::describe_choices(choices)
        ),
    }
}

/// How long [`Spaces::hosts`] waits for each machine to answer.
const HOST_PROBE: Duration = Duration::from_secs(4);

/// One limit of a host and how much of it is used (`HostSpacesCapacity`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HostLimit {
    /// `spaces` (every Space it provides) or `macos_vms`.
    pub resource: String,
    /// In use now.
    pub used: u32,
    /// The limit (0: none).
    pub limit: u32,
    /// Why the limit exists, for people.
    pub reason: String,
}

/// One of your machines that provides Spaces, as [`Spaces::hosts`] lists
/// it: what `on="host:<id>"` creates on.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HostOffer {
    /// What `on="host:<id>"` takes: the relay machine id, or the name a
    /// direct host was added as.
    pub id: String,
    /// Its name.
    pub name: String,
    /// `relay` or `direct`.
    pub via: String,
    /// It answered.
    pub online: bool,
    /// Its operating system (`macos`, `linux`, `windows`), when it answered.
    pub os: String,
    /// Its limits, when it answered.
    pub limits: Vec<HostLimit>,
}

/// What a host said about itself.
async fn host_answer(space: &crate::Space) -> Result<pb::GetHostSpacesResponse> {
    space
        .spacesd()?
        .host_spaces()
        .get_host_spaces(pb::GetHostSpacesRequest {})
        .await
        .map(tonic::Response::into_inner)
        .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
}

/// A machine as [`Spaces::hosts`] lists it: one that answered and provides
/// Spaces; one that does not provide them is left out; one that did not
/// answer is listed offline when `keep_offline` (it provided a Space
/// before, or was added as a host).
fn host_offer(
    id: String,
    name: String,
    via: &str,
    answer: Option<Result<pb::GetHostSpacesResponse>>,
    keep_offline: bool,
) -> Option<HostOffer> {
    match answer {
        Some(Ok(r)) if r.settings.as_ref().is_some_and(|s| !s.provide_spaces) => None,
        Some(Ok(r)) => Some(HostOffer {
            id,
            name,
            via: via.into(),
            online: true,
            os: r.os,
            limits: r
                .capacity
                .into_iter()
                .map(|c| HostLimit {
                    resource: c.resource,
                    used: c.used,
                    limit: c.limit,
                    reason: c.reason,
                })
                .collect(),
        }),
        // It answered without providing Spaces (or no longer does).
        Some(Err(e)) if e.tag() == "host_capability_missing" => None,
        _ => keep_offline.then(|| HostOffer {
            id,
            name,
            via: via.into(),
            online: false,
            os: String::new(),
            limits: vec![],
        }),
    }
}

impl Spaces {
    /// Your machines that provide Spaces, for a "Run on" menu: the signed-in
    /// account's machines on the relay (owned, or shared with you as an
    /// editor; not Spaces a host provides, nor ones in your cloud), then the
    /// hosts added by their direct address. Each is asked for its limits
    /// (at most [`HOST_PROBE`] each, all at once). One that answers without
    /// providing Spaces is left out; one that does not answer is listed
    /// offline when it provided one of your Spaces before (relay) or was
    /// added as a host (direct). Without a relay account, or when the relay
    /// cannot be asked, only the direct hosts are listed.
    pub async fn hosts(&self) -> Result<Vec<HostOffer>> {
        let relay: Vec<crate::relay::RelayMachine> = if self.relay_account().is_some() {
            self.relay_machines().await.unwrap_or_default()
        } else {
            vec![]
        };
        // Hosts that provided one of your Spaces.
        let known: std::collections::HashSet<String> =
            relay.iter().filter_map(|m| m.host.clone()).collect();
        let known = &known;
        // This machine is "This Mac" in the menu, not one of the hosts.
        let this_machine = Host::new(self.home_dir())
            .config()
            .ok()
            .flatten()
            .and_then(|c| c.machine_id);
        let relay_offers = futures_util::future::join_all(
            relay
                .iter()
                .filter(|m| {
                    m.host.is_none()
                        && m.role != "viewer"
                        && !m.meta.contains_key(cua_sandbox_core::byoc::meta::PROVIDER)
                        && this_machine.as_deref() != Some(m.id.as_str())
                })
                .map(|m| async move {
                    let name = if m.name.is_empty() {
                        m.id.clone()
                    } else {
                        m.name.clone()
                    };
                    let answer = if m.online {
                        tokio::time::timeout(HOST_PROBE, async {
                            let s = self.host_space(m).await?;
                            host_answer(&s).await
                        })
                        .await
                        .ok()
                    } else {
                        None
                    };
                    host_offer(m.id.clone(), name, "relay", answer, known.contains(&m.id))
                }),
        )
        .await;
        let mut out: Vec<HostOffer> = relay_offers.into_iter().flatten().collect();
        // The same machine set up both ways is listed once (through the relay).
        let direct: Vec<DirectHost> = self
            .registry()
            .direct_hosts()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !out.iter().any(|o| o.name.eq_ignore_ascii_case(&h.name)))
            .collect();
        let direct_offers = futures_util::future::join_all(direct.iter().map(|h| async move {
            let answer = tokio::time::timeout(HOST_PROBE, async {
                let s = self.direct_host_space(h).await?;
                host_answer(&s).await
            })
            .await
            .ok();
            host_offer(h.name.clone(), h.name.clone(), "direct", answer, true)
        }))
        .await;
        out.extend(direct_offers.into_iter().flatten());
        Ok(out)
    }
}

/// The host part of a `host:port` authority, without IPv6 brackets.
fn authority_host(authority: &str) -> &str {
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    host.trim_start_matches('[').trim_end_matches(']')
}

impl Spaces {
    /// Resolves `query` (a machine id or name, "spare mac mini") to one of
    /// the account's machines on the relay, else (the relay does not know
    /// it, or there is no relay account) to a machine added with `cua
    /// spaces add <addr> --host`. Spaces other hosts provide are never
    /// hosts themselves. Several equally good matches fail with
    /// [`Error::AmbiguousHost`] listing them, for direct hosts as for relay
    /// ones.
    pub async fn resolve_host(&self, query: &str) -> Result<ResolvedHost> {
        let mut relay_error = None;
        let machines: Vec<crate::relay::RelayMachine> = if self.relay_account().is_some() {
            match self.relay_machines().await {
                Ok(all) => all.into_iter().filter(|m| m.host.is_none()).collect(),
                Err(e) => {
                    relay_error = Some(e);
                    vec![]
                }
            }
        } else {
            vec![]
        };
        match cua_host::match_machine(query, &machines) {
            MachineMatch::One(m) => return Ok(ResolvedHost::Relay(*m)),
            MachineMatch::Ambiguous(choices) => return Err(ambiguous_host(query, &choices)),
            MachineMatch::None => {}
        }
        // The machines added by their direct address.
        let direct = self.registry().direct_hosts()?;
        let as_machines: Vec<cua_host::Machine> = direct.iter().map(direct_as_machine).collect();
        match cua_host::match_machine(query, &as_machines) {
            MachineMatch::One(m) => {
                if let Some(h) = direct.iter().find(|h| h.name == m.id) {
                    return Ok(ResolvedHost::Direct(h.clone()));
                }
            }
            MachineMatch::Ambiguous(choices) => return Err(ambiguous_host(query, &choices)),
            MachineMatch::None => {}
        }
        // Nothing here and the relay could not be asked: say why.
        if let Some(e) = relay_error
            && direct.is_empty()
        {
            return Err(e);
        }
        if machines.is_empty() && direct.is_empty() && self.relay_account().is_none() {
            return Err(Error::NotFound(format!(
                "a machine matching {query:?}: you have no machines (sign in to cua.ai to use \
                 the ones on the relay, or add one by its Tailscale or LAN address with `cua \
                 spaces add <addr> --host --token <token>`)"
            )));
        }
        let mut all = machines;
        all.extend(as_machines);
        Err(Error::NotFound(format!(
            "a machine matching {query:?} among your machines ({}); set one up with \
             `cua host setup --provide-spaces` on it, or add one by its Tailscale or LAN address \
             with `cua spaces add <addr> --host --token <token>`",
            if all.is_empty() {
                "none on the relay".to_string()
            } else {
                provided::describe_choices(&all)
            }
        )))
    }

    /// Adds a machine that provides Spaces over its direct listener (`cua
    /// host setup --direct <ip:port> --provide-spaces` on it, which prints
    /// this step): registers it as the Space `direct:<addr>` with its env
    /// token (stored like any direct Space's), and lists it as a direct
    /// host, so `on="host:<name>"` finds it. `name` defaults to the name
    /// the host reports.
    pub async fn add_direct_host(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<String>,
    ) -> Result<SpaceInfo> {
        let info = self.add(url, token, name.clone()).await?;
        if !matches!(SpaceId::parse(&info.id)?, SpaceId::Direct { .. }) {
            return Err(Error::invalid(format!(
                "{} is not reached by a direct address; a host to add with --host is \
                 `<tailscale or LAN address>:<port>`",
                info.id
            )));
        }
        let space = self.space(&info.id).await?;
        // Its own name for itself, unless one was given.
        let host_name = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => n.trim().to_string(),
            None => match space
                .spacesd()?
                .host_spaces()
                .get_host_spaces(pb::GetHostSpacesRequest {})
                .await
            {
                Ok(r) if !r.get_ref().name.is_empty() => r.into_inner().name,
                _ => info.name.clone(),
            },
        };
        if !space
            .capabilities()
            .features
            .iter()
            .any(|f| f.name == HOST_SPACES_FEATURE && f.supported)
        {
            tracing::warn!(
                host = %host_name,
                "{} does not provide Spaces yet (on it: `cua host config --provide-spaces on`)",
                info.id
            );
        }
        if let Some(other) = self
            .registry()
            .direct_hosts()?
            .into_iter()
            .find(|h| h.space != info.id && h.name.eq_ignore_ascii_case(&host_name))
        {
            return Err(Error::invalid(format!(
                "another host is already called {host_name:?} ({}); pass --name",
                other.space
            )));
        }
        self.registry().upsert_direct_host(&host_name, &info.id)?;
        Ok(info)
    }

    /// What a host provides: its settings, capacity and your Spaces on it
    /// (all of them, with its audit, when you own it).
    pub async fn host_spaces(&self, host: &str) -> Result<pb::GetHostSpacesResponse> {
        let space = match self.resolve_host(host).await? {
            ResolvedHost::Relay(m) => self.host_space(&m).await?,
            ResolvedHost::Direct(h) => self.direct_host_space(&h).await?,
        };
        space
            .spacesd()?
            .host_spaces()
            .get_host_spaces(pb::GetHostSpacesRequest {})
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
    }

    async fn host_space(&self, m: &crate::relay::RelayMachine) -> Result<crate::Space> {
        if !m.online {
            return Err(Error::host(
                "host spaces",
                format!(
                    "{} is offline (its cua-spacesd is not connected to the relay)",
                    if m.name.is_empty() { &m.id } else { &m.name }
                ),
            ));
        }
        if m.role == "viewer" {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                "{} is shared with you to watch only; ask its owner to share it as an editor",
                m.name
            ))));
        }
        let id = format!("relay:{}", m.id);
        let s = self.space(&id).await?;
        if !s
            .capabilities()
            .features
            .iter()
            .any(|f| f.name == HOST_SPACES_FEATURE && f.supported)
        {
            // Capabilities may be cached from before the host changed its
            // settings: check once more.
            self.forget_connection(&id).await?;
            let s = self.space(&id).await?;
            s.require(HOST_SPACES_FEATURE).map_err(|_| {
                Error::host(
                    "host spaces",
                    format!(
                        "{} does not provide Spaces (on it: `cua host config --provide-spaces on`)",
                        if m.name.is_empty() { &m.id } else { &m.name }
                    ),
                )
            })?;
            return Ok(s);
        }
        Ok(s)
    }

    /// A direct host's own Space, checked to provide Spaces.
    async fn direct_host_space(&self, host: &DirectHost) -> Result<crate::Space> {
        let provides = |s: &crate::Space| {
            s.capabilities()
                .features
                .iter()
                .any(|f| f.name == HOST_SPACES_FEATURE && f.supported)
        };
        let s = self.space(&host.space).await?;
        if provides(&s) {
            return Ok(s);
        }
        // Capabilities may be cached from before the host changed its
        // settings: check once more.
        self.forget_connection(&host.space).await?;
        let s = self.space(&host.space).await?;
        s.require(HOST_SPACES_FEATURE).map_err(|_| {
            Error::host(
                "host spaces",
                format!(
                    "{} does not provide Spaces (on it: `cua host config --provide-spaces on`)",
                    host.name
                ),
            )
        })?;
        Ok(s)
    }

    /// Creates a Space on the machine `query` names (see the module docs).
    pub(crate) async fn create_on_host(&self, query: &str, opts: SpaceCreate) -> Result<SpaceInfo> {
        let host = match self.resolve_host(query).await? {
            ResolvedHost::Relay(m) => m,
            ResolvedHost::Direct(h) => return self.create_on_direct_host(h, opts).await,
        };
        let space = self.host_space(&host).await?;
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let machine = format!("space-{:016x}", rand::random::<u64>());
        let host_label = if host.name.is_empty() {
            host.id.clone()
        } else {
            host.name.clone()
        };
        let name = opts
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| machine.clone());
        let reg = client
            .register(
                &token,
                &cua_host::relay::RegisterRequest {
                    id: machine.clone(),
                    name: name.clone(),
                    allow: vec![],
                    host: Some(host.id.clone()),
                    meta: Default::default(),
                },
            )
            .await?;
        // What a cancel removes: the relay machine, then the Space the
        // host is creating for it.
        crate::creating::record(crate::creating::Made::RelayMachine {
            id: machine.clone(),
        });
        crate::creating::record(crate::creating::Made::HostSpace {
            host: host.id.clone(),
            machine: machine.clone(),
        });
        let request = pb::CreateHostSpaceRequest {
            image: opts.image.clone().unwrap_or_default(),
            kind: match opts.kind {
                Kind::Auto => String::new(),
                k => k.as_str().into(),
            },
            runtime: match &opts.runtime {
                Runtime::Auto => String::new(),
                r => r.as_str().into(),
            },
            name: opts.name.clone().unwrap_or_default(),
            cpus: opts.cpus.unwrap_or(0),
            memory_mb: opts.memory_mb.unwrap_or(0),
            disk_gb: opts.disk_gb.unwrap_or(0),
            direct: false,
            attach: Some(pb::AttachRelayRequest {
                relay_url: relay.url.clone(),
                machine_token: reg.machine_token.clone(),
                machine_id: machine.clone(),
                relay_jwks_json: reg.jwks.to_string(),
                owner: reg.machine.owner.id.clone(),
                owner_email: reg.machine.owner.email.clone().unwrap_or_default(),
            }),
        };
        let created = async {
            space
                .spacesd()?
                .host_spaces()
                .create_host_space(request)
                .await
                .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
        }
        .await;
        if let Err(e) = created {
            // Nothing runs for it: take the machine off the relay again.
            let _ = client.delete(&token, &machine).await;
            return Err(e);
        }
        // Its driver dials out on its own: wait (bounded) until the relay
        // lists it online.
        let deadline = tokio::time::Instant::now() + ONLINE_WAIT;
        let mut row = None;
        while tokio::time::Instant::now() < deadline {
            if let Ok(m) = client.machine(&token, &machine).await
                && m.online
            {
                row = Some(m);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let _ = self.relay_machines().await;
        let m = match row {
            Some(m) => m,
            None => client.machine(&token, &machine).await?,
        };
        let mut info = crate::spaces::relay_info(&m);
        info.host_name = host_label;
        if !m.online {
            tracing::warn!(machine, "the new Space has not connected to the relay yet");
        }
        Ok(info)
    }

    /// Creates a Space on a direct host: the host creates it and forwards a
    /// port on its address to it, and this device adds it as
    /// `direct:<host address>:<port>` with the Space's token, grouped under
    /// the host. Nothing is registered with the relay.
    async fn create_on_direct_host(
        &self,
        host: DirectHost,
        opts: SpaceCreate,
    ) -> Result<SpaceInfo> {
        let space = self.direct_host_space(&host).await?;
        let request = pb::CreateHostSpaceRequest {
            image: opts.image.clone().unwrap_or_default(),
            kind: match opts.kind {
                Kind::Auto => String::new(),
                k => k.as_str().into(),
            },
            runtime: match &opts.runtime {
                Runtime::Auto => String::new(),
                r => r.as_str().into(),
            },
            name: opts.name.clone().unwrap_or_default(),
            cpus: opts.cpus.unwrap_or(0),
            memory_mb: opts.memory_mb.unwrap_or(0),
            disk_gb: opts.disk_gb.unwrap_or(0),
            attach: None,
            direct: true,
        };
        let made = space
            .spacesd()?
            .host_spaces()
            .create_host_space(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?
            .space
            .ok_or_else(|| {
                Error::host(
                    "host spaces",
                    format!("{} answered without the Space it created", host.name),
                )
            })?;
        let SpaceId::Direct { authority } = SpaceId::parse(&host.space)? else {
            return Err(Error::invalid(format!(
                "{} is not a direct Space",
                host.space
            )));
        };
        let port = u16::try_from(made.direct_port).unwrap_or(0);
        if port == 0 || made.direct_token.is_empty() {
            let _ = self.delete_on_direct_host(&host, &made.local_space).await;
            return Err(Error::host(
                "host spaces",
                format!(
                    "{} created {} but did not forward it (its cua is older than this one; update it)",
                    host.name, made.local_space
                ),
            ));
        }
        let id = format!(
            "direct:{}",
            crate::id::authority(authority_host(&authority), port)
        );
        let name = opts
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| Some(made.name.clone()).filter(|n| !n.is_empty()));
        let info = match self.add(&id, Some(made.direct_token.clone()), name).await {
            Ok(info) => info,
            Err(e) => {
                // Nobody could use it: the host deletes it again.
                let _ = self.delete_on_direct_host(&host, &made.local_space).await;
                return Err(e);
            }
        };
        // Which host made it (deleted and turned off there), and its id
        // there; lists group it under the host.
        self.registry().update_direct_hosts(|hosts| {
            if let Some(h) = hosts.iter_mut().find(|h| h.space == host.space) {
                h.spaces.insert(info.id.clone(), made.local_space.clone());
            }
        })?;
        if let Some(mut record) = self.registry().get(&info.id)? {
            let credential = self.registry().credential(&info.id)?.unwrap_or_default();
            record.host = host.space.clone();
            record.host_name = host.name.clone();
            if record.image.is_empty() {
                record.image = made.image.clone();
            }
            if record.kind.is_empty() {
                record.kind = made.kind.clone();
            }
            self.registry().upsert(record, credential)?;
        }
        self.info(&SpaceId::parse(&info.id)?)
    }

    /// Asks direct host `host` to delete its Space `on_host` (`local:<name>`
    /// there).
    async fn delete_on_direct_host(&self, host: &DirectHost, on_host: &str) -> Result<String> {
        let space = self.direct_host_space(host).await?;
        space
            .spacesd()?
            .host_spaces()
            .delete_host_space(pb::DeleteHostSpaceRequest {
                space: on_host.to_string(),
            })
            .await
            .map(|r| r.into_inner().message)
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
    }

    /// Deletes Space `id`, which direct host `host` created (`on_host`
    /// there): the host deletes it, and it leaves the host's list here.
    pub(crate) async fn delete_on_direct_host_space(
        &self,
        id: &SpaceId,
        host: &DirectHost,
        on_host: &str,
    ) -> Result<String> {
        let message = self.delete_on_direct_host(host, on_host).await?;
        let _ = self.registry().forget_direct(&id.to_string());
        Ok(message)
    }

    /// Turns Space `id`, which direct host `host` created (`on_host`
    /// there), off or on, on that host.
    pub(crate) async fn power_on_direct_host(
        &self,
        id: &SpaceId,
        host: &DirectHost,
        on_host: &str,
        on: bool,
    ) -> Result<crate::SpacePower> {
        let space = self.direct_host_space(host).await?;
        let r = space
            .spacesd()?
            .host_spaces()
            .set_host_space_power(pb::SetHostSpacePowerRequest {
                space: on_host.to_string(),
                on,
            })
            .await
            .map(|r| r.into_inner())
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?;
        Ok(crate::SpacePower {
            space: id.to_string(),
            state: r.state,
            power: r.power,
            message: r.message,
        })
    }

    /// Deletes a Space a host provides: the host deletes its sandbox and
    /// relay machine; this device removes the machine too (best effort).
    pub(crate) async fn delete_on_host(
        &self,
        machine: &crate::relay::RelayMachine,
        host_id: &str,
    ) -> Result<String> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let host = client.machine(&token, host_id).await.map_err(|e| match e {
            cua_host::Error::NotFound(_) => Error::NotFound(format!(
                "the host of relay:{} (machine {host_id}) is no longer on the relay",
                machine.id
            )),
            other => other.into(),
        })?;
        let space = self.host_space(&host).await?;
        let message = space
            .spacesd()?
            .host_spaces()
            .delete_host_space(pb::DeleteHostSpaceRequest {
                space: machine.id.clone(),
            })
            .await
            .map(|r| r.into_inner().message)
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?;
        match client.delete(&token, &machine.id).await {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => {}
            Err(e) => tracing::warn!(machine = %machine.id, "relay machine not removed: {e}"),
        }
        Ok(message)
    }

    /// Turns a Space a host provides off or on, on that host
    /// ([`HostSpacesServer::set_power`]).
    pub(crate) async fn power_on_host(
        &self,
        machine: &crate::relay::RelayMachine,
        host_id: &str,
        on: bool,
    ) -> Result<crate::SpacePower> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let host = client.machine(&token, host_id).await.map_err(|e| match e {
            cua_host::Error::NotFound(_) => Error::NotFound(format!(
                "the host of relay:{} (machine {host_id}) is no longer on the relay",
                machine.id
            )),
            other => other.into(),
        })?;
        let space = self.host_space(&host).await?;
        let r = space
            .spacesd()?
            .host_spaces()
            .set_host_space_power(pb::SetHostSpacePowerRequest {
                space: machine.id.clone(),
                on,
            })
            .await
            .map(|r| r.into_inner())
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?;
        Ok(crate::SpacePower {
            space: SpaceId::Relay {
                machine_id: machine.id.clone(),
            }
            .to_string(),
            state: r.state,
            power: r.power,
            message: r.message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_relay_is_reached_at_the_host_from_a_guest() {
        assert_eq!(
            guest_relay_url("https://relay.cua.ai", "container"),
            "https://relay.cua.ai"
        );
        assert_eq!(
            guest_relay_url("http://127.0.0.1:8080", "container"),
            "http://host.docker.internal:8080"
        );
        assert_eq!(
            guest_relay_url("http://localhost:9/base", "container"),
            "http://host.docker.internal:9/base"
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                guest_relay_url("http://127.0.0.1:8080", "vm"),
                "http://192.168.64.1:8080"
            );
        }
    }

    #[test]
    fn aliases_resolve_and_the_os_is_known() {
        let (r, os) = resolve_image("macos:26");
        assert!(r.contains("macos"), "{r}");
        assert_eq!(os, "macos");
        let (_, os) = resolve_image("linux");
        assert_eq!(os, "linux");
        let (r, os) = resolve_image("ghcr.io/trycua/macos-sequoia-cua:latest");
        assert_eq!(
            (r.as_str(), os.as_str()),
            ("ghcr.io/trycua/macos-sequoia-cua:latest", "macos")
        );
        let (r, os) = resolve_image("");
        assert!(r.contains("linux"));
        assert_eq!(os, "linux");
    }

    #[test]
    fn callers() {
        let c = HostCaller::from_metadata(Some(
            r#"{"account":"ada","email":"ada@x.io","name":"Ada","role":"owner","via":"relay"}"#,
        ))
        .unwrap();
        assert_eq!(c.label(), "Ada <ada@x.io> (ada)");
        assert!(c.is_owner() && c.may_create());
        let local = HostCaller::from_metadata(None).unwrap();
        assert_eq!(local.label(), "local");
        assert!(local.is_owner());
        let viewer = HostCaller {
            role: "viewer".into(),
            ..Default::default()
        };
        assert!(!viewer.may_create());
        assert!(HostCaller::from_metadata(Some("nope")).is_err());
    }

    /// A host that answered is listed with its limits unless it does not
    /// provide Spaces; one that did not answer only when it is known.
    #[test]
    fn host_offers_follow_the_answer() {
        let answer = |provide: bool| pb::GetHostSpacesResponse {
            os: "macos".into(),
            settings: Some(pb::HostSpacesSettings {
                provide_spaces: provide,
                ..Default::default()
            }),
            capacity: vec![pb::HostSpacesCapacity {
                resource: "macos_vms".into(),
                used: 2,
                limit: 2,
                reason: "two per Mac".into(),
            }],
            ..Default::default()
        };
        let offer = |a, keep| host_offer("m1".into(), "Mac mini".into(), "relay", a, keep);
        let on = offer(Some(Ok(answer(true))), false).unwrap();
        assert!(on.online && on.os == "macos");
        assert_eq!(
            on.limits,
            vec![HostLimit {
                resource: "macos_vms".into(),
                used: 2,
                limit: 2,
                reason: "two per Mac".into(),
            }]
        );
        assert_eq!(
            offer(Some(Ok(answer(false))), true),
            None,
            "does not provide"
        );
        assert_eq!(
            offer(Some(Err(Error::host("host spaces", "no"))), true),
            None,
            "answered without providing"
        );
        let silent = offer(None, true).unwrap();
        assert!(!silent.online && silent.limits.is_empty());
        assert_eq!(offer(None, false), None, "unknown and silent");
    }

    /// Without a relay account only the hosts added by address are listed;
    /// one that does not answer is offline.
    #[tokio::test]
    async fn a_direct_host_that_does_not_answer_is_listed_offline() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(dir.path().join("cua")).build();
        assert!(spaces.hosts().await.unwrap().is_empty());
        spaces
            .registry()
            .upsert_direct_host("Studio", "direct:127.0.0.1:1")
            .unwrap();
        assert_eq!(
            spaces.hosts().await.unwrap(),
            vec![HostOffer {
                id: "Studio".into(),
                name: "Studio".into(),
                via: "direct".into(),
                online: false,
                os: String::new(),
                limits: vec![],
            }]
        );
    }

    /// With no relay account, `host:<name>` finds the hosts added with
    /// `cua spaces add <addr> --host`, with the same matching as relay
    /// hosts: the words of the name, and ambiguous_host for a tie.
    #[tokio::test]
    async fn direct_hosts_resolve_by_name_and_ties_are_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(dir.path().join("cua")).build();
        let e = spaces.resolve_host("spare mac mini").await.unwrap_err();
        assert_eq!(e.tag(), "not_found", "{e}");
        assert!(e.to_string().contains("--host"), "{e}");

        let reg = spaces.registry();
        reg.upsert_direct_host("Mac mini (spare)", "direct:100.64.0.9:3211")
            .unwrap();
        reg.upsert_direct_host("Mac mini (office)", "direct:100.64.0.10:3211")
            .unwrap();
        reg.upsert_direct_host("Studio", "direct:192.168.1.20:3211")
            .unwrap();
        match spaces.resolve_host("spare mac mini").await.unwrap() {
            ResolvedHost::Direct(h) => assert_eq!(h.space, "direct:100.64.0.9:3211"),
            other => panic!("{other:?}"),
        }
        let studio = spaces.resolve_host("host:studio").await.unwrap();
        assert_eq!((studio.via(), studio.name().as_str()), ("direct", "Studio"));
        match spaces.resolve_host("mac mini").await.unwrap_err() {
            Error::AmbiguousHost { choices, .. } => {
                assert!(
                    choices.contains(&"host:Mac mini (spare)".to_string())
                        && choices.contains(&"host:Mac mini (office)".to_string()),
                    "{choices:?}"
                );
            }
            other => panic!("{other}"),
        }
        let e = spaces.resolve_host("windows laptop").await.unwrap_err();
        assert_eq!(e.tag(), "not_found", "{e}");
        assert!(e.to_string().contains("Studio"), "{e}");
    }

    #[test]
    fn host_addresses_lose_their_port_and_brackets() {
        assert_eq!(authority_host("100.64.0.9:3211"), "100.64.0.9");
        assert_eq!(
            authority_host("[fd7a:115c:a1e0::9]:3211"),
            "fd7a:115c:a1e0::9"
        );
        assert_eq!(
            authority_host("mini.tailnet.ts.net:3211"),
            "mini.tailnet.ts.net"
        );
    }

    #[test]
    fn host_errors_keep_their_kind_across_the_wire() {
        for e in [
            Error::LimitExceeded("full".into()),
            Error::Relay(cua_host::Error::PermissionDenied("no".into())),
            Error::host("host spaces", "off"),
            Error::NotFound("x".into()),
        ] {
            let status = to_status(&e);
            let back = from_host(cua_spacesd_client::Error::from(status));
            assert_eq!(back.tag(), e.tag(), "{e}");
        }
    }
}
