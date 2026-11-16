//! Network settings of Spaces: kept with the Space ([`SpaceMeta::network`]),
//! carried to the runtime that starts it, and compiled there to rules the
//! host enforces outside the guest ([`cua_vmm::network`] has the model:
//! presets, layers, the support table and the fail-closed plan).
//!
//! - A new Space gets what its create asks for: [`SpaceCreate::network_settings`],
//!   else what the old options say (`network=none` is `none`, an egress
//!   policy is `custom` from `internet`, nothing is `internet`).
//! - A child Space (`parent`) gets the intersection of its own settings and
//!   its parent's: it can only tighten. `widen` skips that and records who
//!   widened it.
//! - A Space from before network settings gets them, migrated the same
//!   way, on its next start ([`migrate_meta`]), and a one-time notice.
//! - Forks keep their source's settings and tree
//!   ([`crate::registry::Registry::inherit_meta`]).
//!
//! [`SpaceMeta::network`]: crate::registry::SpaceMeta::network
//! [`SpaceCreate::network_settings`]: crate::SpaceCreate::network_settings

use crate::registry::SpaceMeta;
use crate::{Error, Result, Spaces};
use cua_proto::daemon::v1 as dpb;
use cua_proto::env::v1 as epb;

pub use cua_vmm::network::{
    Applied, EgressVia, Mode, NO_NETWORK_NOTE, NetworkContext, NetworkPlan, NetworkPreset,
    NetworkSettings, NetworkSupport, Placement, UNPROTECTED_NOTE, network_support,
};

/// The runtime of a local Space's backend word (`qemu`, `docker`,
/// `gvisor`, `vz`, ...).
pub fn backend_kind(word: &str) -> Option<cua_vmm::BackendKind> {
    use cua_vmm::BackendKind as B;
    Some(match word {
        "qemu" => B::Qemu,
        "qemu-docker" => B::QemuDocker,
        "lume" => B::Lume,
        "container" | "docker" | "podman" | "runc" | "runsc" | "gvisor" => B::Container,
        "managed" => B::Managed,
        "firecracker" => B::Firecracker,
        "vz" => B::Vz,
        "containerization" | "apple" => B::Containerization,
        _ => return None,
    })
}

/// The note a Space on this computer's `backend` gets for `settings`: why
/// it has no network (its runtime can't protect it yet), or that the user
/// chose No protection. `None` when its rules are enforced.
pub fn note_for(backend: &str, settings: &NetworkSettings) -> Option<String> {
    let kind = backend_kind(backend)?;
    let support = network_support(kind, std::env::consts::OS, Placement::Local);
    match cua_vmm::network::plan(
        settings,
        support,
        &NetworkContext::default(),
        Mode::from_env(),
    ) {
        Ok(NetworkPlan::NoNetwork { note: Some(n) }) | Ok(NetworkPlan::Open { note: Some(n) }) => {
            Some(n.to_string())
        }
        _ => None,
    }
}

/// While the operator override `CUA_SECURE_DEFAULT=0` has the secure default off, a Space gets
/// the network it always had: only the old allow and deny lists. Anything else is refused.
pub fn require_secure_spaces(settings: Option<&NetworkSettings>) -> Result<()> {
    if Mode::from_env() == Mode::Secure {
        return Ok(());
    }
    // What the old calls (`network = none`, the egress lists) already
    // asked for: Internet, None, and Custom from Internet.
    let beyond_lists = settings.is_some_and(|s| {
        !(matches!(s.preset, NetworkPreset::Internet | NetworkPreset::None)
            || (s.preset == NetworkPreset::Custom && s.custom_base == NetworkPreset::Internet))
            || !s.localhost_ports.is_empty()
            || s.all_localhost_ports
            || s.widened_by.is_some()
            || s.via.is_some()
            || !s.custom.split_domains().1.is_empty()
    });
    if beyond_lists {
        return Err(Error::invalid(
            cua_vmm::network::secure_default_off_message(),
        ));
    }
    Ok(())
}

/// Refuses network settings a Space in your own cloud account or a
/// provider can't keep: anything but No protection (the provider has no
/// point cua controls to enforce them).
pub fn refuse_on_provider(settings: &NetworkSettings) -> Result<()> {
    if Mode::from_env() == Mode::Legacy || settings.preset == NetworkPreset::Unprotected {
        return Ok(());
    }
    match network_support(
        cua_vmm::BackendKind::Container,
        "linux",
        Placement::Provider,
    ) {
        NetworkSupport::Refuse { why } => Err(Error::invalid(why)),
        NetworkSupport::Enforced { .. } | NetworkSupport::NoNetworkOnly { .. } => Ok(()),
    }
}

/// What no setting opens, in plain words (`cua network show`).
pub const ALWAYS_BLOCKED: [&str; 4] = [
    "Cloud metadata (169.254.169.254 and the other clouds' endpoints)",
    "Spaces in other trees",
    "The Cua daemon on this computer",
    "This computer's own addresses (only \"localhost\" opens listed ports)",
];

/// A Space's network as `cua network show` says it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct NetworkInfo {
    /// Its settings.
    pub settings: NetworkSettings,
    /// One plain line.
    pub summary: String,
    /// What the settings open beyond "internet".
    pub widenings: Vec<String>,
    /// `enforced`, `no_network`, `open`, `refused`; empty when unknown.
    pub enforcement: String,
    /// Where it is enforced, or why its network is cut.
    pub point: String,
    /// Why its network is not what was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Its parent and tree root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// What no setting opens.
    pub always_blocked: Vec<String>,
    /// An update changed its settings; say so once.
    pub notice_pending: bool,
    /// A VM's address in its tree ([`cua_vmm::tree`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree_address: Option<String>,
    /// Its topology (names, members, host, exit, path) as JSON text
    /// ([`crate::Spaces::network_status`]); empty when unknown.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub topology_json: String,
}

/// The tree address of this computer's VM Space `name` ([`cua_vmm::tree`]),
/// if it has one.
pub fn tree_address(name: &str) -> Option<String> {
    cua_vmm::tree::address_by_name(name).map(|a| a.to_string())
}

/// [`NetworkInfo`] in the daemon's wire form.
pub fn info_to_pb(i: &NetworkInfo) -> dpb::SpaceNetworkInfo {
    dpb::SpaceNetworkInfo {
        settings: Some(to_pb(&i.settings)),
        summary: i.summary.clone(),
        widenings: i.widenings.clone(),
        enforcement: i.enforcement.clone(),
        point: i.point.clone(),
        note: i.note.clone().unwrap_or_default(),
        parent: i.parent.clone().unwrap_or_default(),
        root: i.root.clone().unwrap_or_default(),
        always_blocked: i.always_blocked.clone(),
        notice_pending: i.notice_pending,
        tree_address: i.tree_address.clone().unwrap_or_default(),
        topology_json: i.topology_json.clone(),
    }
}

/// How `settings` fare on `backend` here: (`enforcement`, `point`).
pub fn enforcement_of(backend: &str, settings: &NetworkSettings) -> (String, String) {
    let Some(kind) = backend_kind(backend) else {
        return (String::new(), String::new());
    };
    let support = network_support(kind, std::env::consts::OS, Placement::Local);
    let point = match support {
        NetworkSupport::Enforced { point } => point,
        NetworkSupport::NoNetworkOnly { why } | NetworkSupport::Refuse { why } => why,
    };
    let word = match cua_vmm::network::plan(
        settings,
        support,
        &NetworkContext::default(),
        Mode::from_env(),
    ) {
        Ok(NetworkPlan::Enforce(_)) => "enforced",
        Ok(NetworkPlan::NoNetwork { .. }) => "no_network",
        Ok(NetworkPlan::Open { .. }) => "open",
        Ok(NetworkPlan::Refuse(_)) | Err(_) => "refused",
    };
    (word.into(), point.into())
}

/// What a Space's listing shows of its network.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SpaceNetwork {
    /// Its settings.
    pub settings: NetworkSettings,
    /// Why its network is not what was asked, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Its settings were changed by an update; say so once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notice_pending: bool,
}

/// The one-time notice of a Space moved to the new defaults.
pub const MIGRATION_NOTICE: &str = "This Space now uses Internet only. It can't reach this \
     computer or your local network. Change it with `cua network set`.";

/// The settings of a Space from before network settings
/// ([`NetworkSettings::migrate`] of its egress policy), and whether they
/// open anything its old policy's lists named (kept, and labelled by
/// [`NetworkSettings::widenings`]). `restrict_network`: it was created with
/// `network=none`.
pub fn migrate_meta(meta: &SpaceMeta, restrict_network: bool) -> NetworkSettings {
    NetworkSettings::migrate(&meta.egress, restrict_network)
}

/// `settings` in the daemon's wire form.
pub fn to_pb(s: &NetworkSettings) -> dpb::NetworkSettings {
    dpb::NetworkSettings {
        preset: s.preset.as_str().into(),
        localhost_ports: s.localhost_ports.iter().map(|p| u32::from(*p)).collect(),
        all_localhost_ports: s.all_localhost_ports,
        allow: s.custom.allow.clone(),
        deny: s.custom.deny.clone(),
        custom_base: match s.custom_base {
            NetworkPreset::None => "none".into(),
            _ => String::new(),
        },
        widened_by: s.widened_by.clone().unwrap_or_default(),
        via: s.via.as_ref().map(|v| v.as_word()).unwrap_or_default(),
    }
}

/// The daemon's wire form back, checked ([`NetworkSettings::validate`]).
pub fn from_pb(p: &dpb::NetworkSettings) -> Result<NetworkSettings> {
    settings_from_parts(
        &p.preset,
        &p.localhost_ports,
        p.all_localhost_ports,
        &p.allow,
        &p.deny,
        &p.custom_base,
        &p.widened_by,
        &p.via,
    )
}

/// `settings` as a host's create carries them.
pub fn to_host_pb(s: &NetworkSettings) -> epb::HostNetworkSettings {
    let p = to_pb(s);
    epb::HostNetworkSettings {
        preset: p.preset,
        localhost_ports: p.localhost_ports,
        all_localhost_ports: p.all_localhost_ports,
        allow: p.allow,
        deny: p.deny,
        custom_base: p.custom_base,
        widened_by: p.widened_by,
        via: p.via,
    }
}

/// A host's create's settings back, checked.
pub fn from_host_pb(p: &epb::HostNetworkSettings) -> Result<NetworkSettings> {
    settings_from_parts(
        &p.preset,
        &p.localhost_ports,
        p.all_localhost_ports,
        &p.allow,
        &p.deny,
        &p.custom_base,
        &p.widened_by,
        &p.via,
    )
}

#[allow(clippy::too_many_arguments)]
fn settings_from_parts(
    preset: &str,
    ports: &[u32],
    all_ports: bool,
    allow: &[String],
    deny: &[String],
    base: &str,
    widened_by: &str,
    via: &str,
) -> Result<NetworkSettings> {
    let word = |w: &str| -> Result<NetworkPreset> {
        if w.trim().is_empty() {
            return Ok(NetworkPreset::Internet);
        }
        NetworkPreset::parse(w).map_err(|e| Error::invalid(e.0))
    };
    let localhost_ports = ports
        .iter()
        .map(|p| {
            u16::try_from(*p)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| Error::invalid(format!("localhost port {p} is not a port")))
        })
        .collect::<Result<Vec<u16>>>()?;
    let s = NetworkSettings {
        preset: word(preset)?,
        localhost_ports,
        all_localhost_ports: all_ports,
        custom: crate::egress::EgressPolicy {
            allow: allow.to_vec(),
            deny: deny.to_vec(),
        },
        custom_base: word(base)?,
        widened_by: (!widened_by.is_empty()).then(|| widened_by.to_string()),
        via: cua_vmm::network::EgressVia::parse(via).map_err(|e| Error::invalid(e.0))?,
    };
    s.validate().map_err(|e| Error::invalid(e.0))?;
    Ok(s)
}

/// `s` with the exit `word` ([`cua_vmm::network::EgressVia::parse`]:
/// `internet`, `this-computer`, `space:<name>` or `socks5://host:port`).
pub fn with_via(mut s: NetworkSettings, word: &str) -> Result<NetworkSettings> {
    s.via = cua_vmm::network::EgressVia::parse(word).map_err(|e| Error::invalid(e.0))?;
    s.validate().map_err(|e| Error::invalid(e.0))?;
    Ok(s)
}

/// Refuses an exit an agent may not choose: this computer (it shares this
/// machine's network, which only the user decides, as `hotspot_start`
/// asks) and a SOCKS5 proxy. A Space of its tree, or none, an agent may
/// pick.
pub fn refuse_exit_by_agent(
    settings: &NetworkSettings,
    current: Option<&NetworkSettings>,
) -> Result<()> {
    use cua_vmm::network::EgressVia as V;
    if settings.via == current.and_then(|c| c.via.clone()) {
        return Ok(());
    }
    match &settings.via {
        None | Some(V::Space(_)) => Ok(()),
        Some(V::ThisMachine) | Some(V::Socks5(_)) => Err(Error::invalid(
            "exiting through this computer or a proxy is the user's to choose: ask them to run \
             `cua network set <space> --via ...` (or use the app)",
        )),
    }
}

/// Who widened a child and when: `user@2026-10-26T12:00:00Z`.
pub fn widened_stamp() -> String {
    let who = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "user".into());
    format!(
        "{who}@{}",
        humantime::format_rfc3339_seconds(std::time::SystemTime::now())
    )
}

/// The network a create resolved: the settings the Space gets and its
/// place in a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreateNetwork {
    /// What the Space gets.
    pub settings: NetworkSettings,
    /// Its parent's canonical id.
    pub parent: Option<String>,
    /// Its tree's root's canonical id.
    pub root: Option<String>,
    /// Its depth below the root (0: top-level).
    pub depth: u32,
}

/// Settings from one word as the CLI, SDK and MCP take it: `internet`
/// (`default` and empty too), `none`, `lan`, `unprotected`, `custom` (from
/// `internet`), `custom:none`, `localhost:5432,3000` or `localhost:all`
/// (`localhost+lan:5432,3000` adds the local network too),
/// with `allow` and `deny` lists on top.
pub fn parse_network(word: &str, allow: &[String], deny: &[String]) -> Result<NetworkSettings> {
    let w = word.trim();
    let (head, rest) = match w.split_once(':') {
        Some((h, r)) => (h, Some(r)),
        None => (w, None),
    };
    let preset = if head.is_empty() {
        NetworkPreset::Internet
    } else {
        NetworkPreset::parse(head).map_err(|e| Error::invalid(e.0))?
    };
    let mut s = NetworkSettings::preset(preset);
    match (preset, rest) {
        (_, None) => {}
        (p, Some(r)) if p.opens_localhost() && r.trim().eq_ignore_ascii_case("all") => {
            s.all_localhost_ports = true
        }
        (p, Some(r)) if p.opens_localhost() => {
            for p in r.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let port =
                    p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| {
                        Error::invalid(format!("localhost port {p:?} is not a port"))
                    })?;
                if !s.localhost_ports.contains(&port) {
                    s.localhost_ports.push(port);
                }
            }
        }
        (NetworkPreset::Custom, Some(base)) => {
            s.custom_base = NetworkPreset::parse(base).map_err(|e| Error::invalid(e.0))?;
        }
        (_, Some(_)) => {
            return Err(Error::invalid(format!(
                "network {w:?}: only localhost takes ports (localhost:5432,3000) and only \
                 custom a base (custom:none)"
            )));
        }
    }
    s.custom = crate::egress::EgressPolicy {
        allow: allow.to_vec(),
        deny: deny.to_vec(),
    };
    // Lists on internet or none make it custom from that base.
    if !s.custom.is_empty() && matches!(preset, NetworkPreset::Internet | NetworkPreset::None) {
        s.custom_base = preset;
        s.preset = NetworkPreset::Custom;
    }
    s.validate().map_err(|e| Error::invalid(e.0))?;
    Ok(s)
}

/// Refuses settings an agent (an MCP call) may not choose: anything that
/// opens more than Internet only (`localhost`, `lan`, `unprotected`, an
/// allow past the default blocks), or, with `current`, more than the Space
/// has now. Opening more is the user's to do, in the app or the CLI.
pub fn refuse_widening_by_agent(
    settings: &NetworkSettings,
    current: Option<&NetworkSettings>,
) -> Result<()> {
    let user_only = |what: &str| {
        Err(Error::invalid(format!(
            "{what} is the user's to choose: ask them to run `cua network set` (or use the app)"
        )))
    };
    if !settings.widenings().is_empty() {
        return user_only("Opening more than Internet only");
    }
    if let Some(current) = current {
        // No more open than now: cutting it down to what the Space has now
        // changes nothing.
        // Names count too: a name allowed now that the Space didn't let
        // through before is more open.
        let ctx = NetworkContext::default();
        let want = cua_vmm::network::compile_named(settings, &ctx, Mode::Secure)
            .map_err(|e| Error::invalid(e.0))?;
        let within =
            cua_vmm::network::compile_named(&settings.intersect(current), &ctx, Mode::Secure)
                .map_err(|e| Error::invalid(e.0))?;
        if want.0 != within.0 || want.1.allow.iter().any(|n| !within.1.allow.contains(n)) {
            return user_only("Opening more than the Space has now");
        }
    }
    Ok(())
}

/// The settings a parent Space has: its own, or what it would migrate to.
pub(crate) fn settings_of(meta: Option<&SpaceMeta>) -> NetworkSettings {
    match meta {
        Some(m) => m.network.clone().unwrap_or_else(|| migrate_meta(m, false)),
        None => NetworkSettings::internet(),
    }
}

/// [`CreateNetwork`] from what a create asks (`asked`), the parent's id
/// and record (if any) and `widen`.
pub(crate) fn resolve_create(
    asked: NetworkSettings,
    parent: Option<(String, Option<SpaceMeta>)>,
    widen: bool,
) -> Result<CreateNetwork> {
    asked.validate().map_err(|e| Error::invalid(e.0))?;
    let Some((parent_id, parent_meta)) = parent else {
        if widen {
            return Err(Error::invalid(
                "widen goes with parent: only a child Space can be allowed more than its parent",
            ));
        }
        return Ok(CreateNetwork {
            settings: asked,
            parent: None,
            root: None,
            depth: 0,
        });
    };
    let root = parent_meta
        .as_ref()
        .and_then(|m| m.root.clone())
        .unwrap_or_else(|| parent_id.clone());
    let theirs = settings_of(parent_meta.as_ref());
    let settings = if widen {
        // Allowed more than its parent, but never out of its parent's exit
        // (a parent sends its whole tree through it).
        let via = if asked.preset == NetworkPreset::None {
            None
        } else {
            theirs.via.clone().or_else(|| asked.via.clone())
        };
        NetworkSettings {
            widened_by: asked.widened_by.clone().or_else(|| Some(widened_stamp())),
            via,
            ..asked
        }
    } else {
        asked.intersect(&theirs)
    };
    Ok(CreateNetwork {
        settings,
        depth: parent_meta.as_ref().map_or(0, |m| m.depth) + 1,
        parent: Some(parent_id),
        root: Some(root),
    })
}

impl Spaces {
    /// The network a create asks for, intersected with its parent's.
    pub(crate) fn create_network(&self, opts: &crate::SpaceCreate) -> Result<CreateNetwork> {
        let asked = opts.network_settings.clone().unwrap_or_else(|| {
            NetworkSettings::from_legacy(
                &opts.egress,
                opts.network == cua_sandbox_core::NetworkMode::None,
            )
        });
        let parent = match opts.parent.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(p) => {
                let id = self.resolve(p)?.to_string();
                if self.inner.registry.get(&id)?.is_none()
                    && self.inner.registry.meta(&id).is_none()
                {
                    return Err(Error::NotFound(format!("parent {p}")));
                }
                let meta = self.inner.registry.meta(&id);
                // Not deeper than its tree allows.
                let root_meta = match meta.as_ref().and_then(|m| m.root.clone()) {
                    Some(r) => self.inner.registry.meta(&r),
                    None => meta.clone(),
                };
                crate::tree::child_depth(
                    p,
                    meta.as_ref().map_or(0, |m| m.depth),
                    crate::tree::depth_limit(root_meta.as_ref()),
                )?;
                Some((id, meta))
            }
        };
        // An exit Space by its id, in the tree the new Space joins.
        let asked = match (&asked.via, &parent) {
            (Some(EgressVia::Space(name)), Some((pid, pmeta))) => {
                let exit = self.resolve(name)?.to_string();
                let root = pmeta
                    .as_ref()
                    .and_then(|m| m.root.clone())
                    .unwrap_or_else(|| pid.clone());
                let exit_root = self
                    .inner
                    .registry
                    .meta(&exit)
                    .and_then(|m| m.root)
                    .unwrap_or_else(|| exit.clone());
                if exit_root != root {
                    return Err(Error::invalid(format!(
                        "{name} is not in this Space's tree: a Space exits through its own tree's Spaces"
                    )));
                }
                NetworkSettings {
                    via: Some(EgressVia::Space(exit)),
                    ..asked
                }
            }
            (Some(EgressVia::Space(name)), None) => {
                return Err(Error::invalid(format!(
                    "{name} can't be the exit of a new top-level Space: it starts a tree of its own; \
                     set the exit once it has a Space to exit through"
                )));
            }
            _ => asked,
        };
        resolve_create(asked, parent, opts.widen)
    }

    /// Records what a create resolved on the new Space.
    pub(crate) fn record_network(&self, id: &str, net: &CreateNetwork) -> Result<()> {
        self.inner.registry.update_meta(id, |m| {
            m.network = Some(net.settings.clone());
            m.network_generation = m.network_generation.saturating_add(1).max(1);
            m.parent = net.parent.clone();
            m.root = net.root.clone();
            m.depth = net.depth;
            if let Some(p) = &net.parent {
                m.origin = Some(crate::tree::ORIGIN_CHILD.into());
                m.labels.insert(crate::tree::PARENT_LABEL.into(), p.clone());
            }
        })
    }

    /// A Space's network: its settings, a note on why it differs from
    /// them, and whether an update changed them. `None` for a Space with
    /// no record here.
    pub fn network(&self, space: &str) -> Result<Option<SpaceNetwork>> {
        let id = self.resolve(space)?.to_string();
        Ok(self.inner.registry.meta(&id).map(|m| SpaceNetwork {
            settings: settings_of(Some(&m)),
            note: m.network_note.clone(),
            notice_pending: m.network_notice_pending || m.network.is_none(),
        }))
    }

    /// Moves a Space from before network settings to them (at its next
    /// start): its old egress policy becomes `custom`, nothing becomes
    /// `internet`, and a one-time notice is left. A Space that has them is
    /// left as it is. Returns its settings.
    pub(crate) fn migrate_network(&self, id: &str) -> Result<NetworkSettings> {
        let Some(meta) = self.inner.registry.meta(id) else {
            // Nothing recorded: nothing it was created with but the
            // default.
            let s = NetworkSettings::internet();
            self.inner.registry.update_meta(id, |m| {
                m.network = Some(s.clone());
                m.network_generation = 1;
                m.network_notice_pending = true;
            })?;
            return Ok(s);
        };
        if let Some(s) = meta.network {
            return Ok(s);
        }
        let s = migrate_meta(&meta, false);
        self.inner.registry.update_meta(id, |m| {
            m.network = Some(s.clone());
            m.network_generation = m.network_generation.saturating_add(1);
            m.network_notice_pending = true;
        })?;
        tracing::info!(space = %id, preset = %s.preset, "moved a Space to network settings");
        Ok(s)
    }

    /// Replaces a Space's network settings. A child's are intersected with
    /// its parent's (no more open) unless `widen`, which records who widened
    /// it. With `tree`, its children get the new settings too (each
    /// intersected with its own parent's). Says whether they hold now or
    /// from its next start. A Space on this computer only: a Space another
    /// machine runs keeps its settings there.
    pub async fn set_network(
        &self,
        space: &str,
        settings: &NetworkSettings,
        widen: bool,
        tree: bool,
    ) -> Result<cua_vmm::network::Applied> {
        settings.validate().map_err(|e| Error::invalid(e.0))?;
        // Changing a Space's network needs the secure default.
        if Mode::from_env() == Mode::Legacy {
            return Err(Error::invalid(
                cua_vmm::network::secure_default_off_message(),
            ));
        }
        let id = self.resolve(space)?;
        let crate::SpaceId::Local { name } = &id else {
            return Err(Error::WrongProvider {
                op: "Changing network settings".into(),
                provider: format!(
                    "{} (only a Space on this computer; change it on the machine that runs it)",
                    id.provider()
                ),
            });
        };
        let key = id.to_string();
        let meta = self.inner.registry.meta(&key).unwrap_or_default();
        let settings = &self.resolve_via(&key, settings)?;
        let parent = meta.parent.clone().map(|p| {
            let m = self.inner.registry.meta(&p);
            (p, m)
        });
        let resolved = resolve_create(settings.clone(), parent, widen && meta.parent.is_some())?;
        if widen && meta.parent.is_none() {
            return Err(Error::invalid(
                "widen goes with a child Space: a top-level Space has no parent to stay within",
            ));
        }
        let applied = self
            .inner
            .sandboxes
            .set_network(name, &resolved.settings)
            .await
            .map_err(crate::egress::sandbox_error)?;
        let backend = self
            .inner
            .sandboxes
            .connect(name)
            .await
            .ok()
            .map(|sb| sb.runtime_type().to_string());
        let note = backend.and_then(|b| note_for(&b, &resolved.settings));
        self.inner.registry.update_meta(&key, |m| {
            m.network = Some(resolved.settings.clone());
            m.network_generation = m.network_generation.saturating_add(1);
            m.network_note = note;
            m.network_notice_pending = false;
            // The address lists (names live in `network` only).
            m.egress = resolved.settings.custom.split_domains().0;
        })?;
        // Its children stay within it: each keeps its own settings, intersected
        // with the new ones (a child someone widened keeps that), or with
        // `tree` takes the new ones.
        for child in crate::tree::children_of(&self.inner.registry.all_meta(), &key) {
            let current = self.inner.registry.meta(&child);
            let widened = current
                .as_ref()
                .and_then(|m| m.network.as_ref())
                .is_some_and(|n| n.widened_by.is_some());
            if widened && !tree {
                // It keeps what it was allowed, but takes the parent's exit.
                let own = settings_of(current.as_ref());
                Box::pin(self.set_network(&child, &own, true, false)).await?;
                continue;
            }
            let child_settings = if tree {
                settings.clone()
            } else {
                settings_of(current.as_ref())
            };
            Box::pin(self.set_network(&child, &child_settings, false, tree)).await?;
        }
        // Its exit and published host ports follow at once.
        #[cfg(feature = "spaces-hotspot")]
        self.exit_changed(&key);
        Ok(applied)
    }

    /// `settings` with an exit Space named by its canonical id, checked:
    /// another Space of the same tree.
    pub(crate) fn resolve_via(
        &self,
        key: &str,
        settings: &NetworkSettings,
    ) -> Result<NetworkSettings> {
        let current = crate::network::settings_of(self.inner.registry.meta(key).as_ref());
        // Choosing an exit needs the secure default.
        if settings.via.is_some() && settings.via != current.via && Mode::from_env() == Mode::Legacy
        {
            return Err(Error::invalid(
                cua_vmm::network::secure_default_off_message(),
            ));
        }
        let Some(cua_vmm::network::EgressVia::Space(name)) = &settings.via else {
            return Ok(settings.clone());
        };
        let exit = self.resolve(name)?.to_string();
        if exit == key {
            return Err(Error::invalid("a Space can't exit through itself"));
        }
        let root_of = |id: &str| {
            self.inner
                .registry
                .meta(id)
                .and_then(|m| m.root)
                .unwrap_or_else(|| id.to_string())
        };
        if root_of(&exit) != root_of(key) {
            return Err(Error::invalid(format!(
                "{name} is not in this Space's tree: a Space exits through its own tree's Spaces"
            )));
        }
        let mut s = settings.clone();
        s.via = Some(cua_vmm::network::EgressVia::Space(exit));
        Ok(s)
    }

    /// What a Space on this computer tried to reach and its network refused,
    /// most recent first (`cua network log`). A Space another computer runs
    /// is `WrongProvider`.
    pub async fn network_events(&self, space: &str) -> Result<Vec<cua_vmm::egress::NetworkEvent>> {
        let id = self.resolve(space)?;
        let crate::SpaceId::Local { name } = &id else {
            return Err(Error::WrongProvider {
                op: "Listing blocked connections".into(),
                provider: format!(
                    "{} (only a Space on this computer; ask the computer that runs it)",
                    id.provider()
                ),
            });
        };
        self.inner
            .sandboxes
            .network_events(name)
            .await
            .map_err(crate::egress::sandbox_error)
    }

    /// A Space's network as `cua network show` says it.
    pub async fn network_info(&self, space: &str) -> Result<NetworkInfo> {
        let id = self.resolve(space)?;
        let key = id.to_string();
        let meta = self.inner.registry.meta(&key);
        let settings = settings_of(meta.as_ref());
        let (enforcement, point) = match &id {
            crate::SpaceId::Local { name } => match self.inner.sandboxes.connect(name).await {
                Ok(sb) => enforcement_of(sb.runtime_type(), &settings),
                Err(_) => (String::new(), String::new()),
            },
            _ => (
                String::new(),
                "kept by the machine that runs it".to_string(),
            ),
        };
        Ok(NetworkInfo {
            summary: settings.summary(),
            widenings: settings.widenings(),
            enforcement,
            point,
            note: meta.as_ref().and_then(|m| m.network_note.clone()),
            parent: meta.as_ref().and_then(|m| m.parent.clone()),
            root: meta.as_ref().and_then(|m| m.root.clone()),
            always_blocked: ALWAYS_BLOCKED.iter().map(|s| s.to_string()).collect(),
            notice_pending: meta
                .as_ref()
                .is_some_and(|m| m.network_notice_pending || m.network.is_none()),
            tree_address: match &id {
                crate::SpaceId::Local { name } => tree_address(name),
                _ => None,
            },
            topology_json: self
                .network_status(&key)
                .await
                .map(|v| v.to_string())
                .unwrap_or_default(),
            settings,
        })
    }

    /// Clears a Space's one-time notice (the CLI or app showed it).
    pub fn ack_network_notice(&self, space: &str) -> Result<()> {
        let id = self.resolve(space)?.to_string();
        self.inner
            .registry
            .update_meta(&id, |m| m.network_notice_pending = false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_word_networks() {
        let p = |w: &str| parse_network(w, &[], &[]);
        assert_eq!(p("").unwrap(), NetworkSettings::internet());
        assert_eq!(p("default").unwrap(), NetworkSettings::internet());
        assert_eq!(p("none").unwrap(), NetworkSettings::none());
        let l = p("localhost:5432, 3000").unwrap();
        assert_eq!(l.localhost_ports, [5432, 3000]);
        assert!(p("localhost:all").unwrap().all_localhost_ports);
        assert!(p("localhost").is_err(), "ports are needed");
        assert!(p("localhost:0").is_err());
        let b = p("localhost+lan:5432, 3000").unwrap();
        assert_eq!(b.preset, NetworkPreset::LocalhostLan);
        assert_eq!(b.localhost_ports, [5432, 3000]);
        assert!(p("localhost+lan:all").unwrap().all_localhost_ports);
        assert!(p("localhost+lan").is_err(), "ports are needed");
        assert_eq!(
            p("lan+localhost:80").unwrap().preset,
            NetworkPreset::LocalhostLan
        );
        assert_eq!(p("custom:none").unwrap().custom_base, NetworkPreset::None);
        assert!(p("lan:5432").is_err());
        assert!(p("open").is_err());
        let c = parse_network("internet", &["10.0.5.0/24".into()], &[]).unwrap();
        assert_eq!(
            (c.preset, c.custom_base),
            (NetworkPreset::Custom, NetworkPreset::Internet)
        );
        let c = parse_network("none", &["1.1.1.1".into()], &[]).unwrap();
        assert_eq!(
            (c.preset, c.custom_base),
            (NetworkPreset::Custom, NetworkPreset::None)
        );
        // Names are rules too (the gateway's resolver enforces them).
        let n = parse_network("internet", &["pypi.org".into()], &[]).unwrap();
        assert_eq!(n.preset, NetworkPreset::Custom);
        assert_eq!(n.custom.allow, ["pypi.org"]);
        assert!(parse_network("internet", &["bad name!".into()], &[]).is_err());
    }

    #[test]
    fn agents_only_tighten() {
        let internet = NetworkSettings::internet();
        assert!(refuse_widening_by_agent(&internet, None).is_ok());
        assert!(refuse_widening_by_agent(&NetworkSettings::none(), Some(&internet)).is_ok());
        let lan = NetworkSettings::preset(NetworkPreset::Lan);
        let e = refuse_widening_by_agent(&lan, None).unwrap_err();
        assert!(e.to_string().contains("cua network set"), "{e}");
        // Both extras together are the user's to choose too, and from `lan`
        // alone they are more than the Space has.
        let both = parse_network("localhost+lan:5432", &[], &[]).unwrap();
        assert!(refuse_widening_by_agent(&both, None).is_err());
        assert!(refuse_widening_by_agent(&both, Some(&lan)).is_err());
        assert!(refuse_widening_by_agent(&lan, Some(&both)).is_err());
        assert!(
            refuse_widening_by_agent(&NetworkSettings::preset(NetworkPreset::Unprotected), None)
                .is_err()
        );
        let one = parse_network("custom", &["1.1.1.1".into()], &["all".into()]).unwrap();
        assert!(
            refuse_widening_by_agent(&one, Some(&internet)).is_ok(),
            "tighter"
        );
        assert!(
            refuse_widening_by_agent(&internet, Some(&one)).is_err(),
            "wider than now"
        );
        let deny = parse_network("internet", &[], &["8.8.8.8".into()]).unwrap();
        assert!(refuse_widening_by_agent(&deny, Some(&internet)).is_ok());
        let private = parse_network("internet", &["10.0.5.0/24".into()], &[]).unwrap();
        assert!(
            refuse_widening_by_agent(&private, None).is_err(),
            "a LAN allow"
        );
    }

    #[test]
    fn notes_and_provider_refusals_in_plain_words() {
        if Mode::from_env() == Mode::Legacy {
            return;
        }
        let internet = NetworkSettings::internet();
        assert_eq!(note_for("docker", &internet), None);
        assert_eq!(note_for("unknown", &internet), None);
        if cfg!(target_os = "macos") {
            // Behind the Cua network gateway: enforced, so no note.
            assert_eq!(note_for("vz", &internet), None);
        }
        let open = NetworkSettings::preset(NetworkPreset::Unprotected);
        assert_eq!(
            note_for("docker", &open),
            None,
            "enforced: its platform denies hold"
        );
        let e = refuse_on_provider(&internet).unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert!(e.to_string().contains("No protection"), "{e}");
        assert!(refuse_on_provider(&open).is_ok());
        for w in [
            "qemu",
            "docker",
            "gvisor",
            "vz",
            "lume",
            "firecracker",
            "containerization",
        ] {
            assert!(backend_kind(w).is_some(), "{w}");
        }
    }

    fn meta(allow: &[&str], deny: &[&str]) -> SpaceMeta {
        SpaceMeta {
            egress: crate::egress::EgressPolicy {
                allow: allow.iter().map(|s| s.to_string()).collect(),
                deny: deny.iter().map(|s| s.to_string()).collect(),
            },
            ..Default::default()
        }
    }

    /// Old `space-meta.json` entries become the expected settings.
    #[test]
    fn migration_fixtures() {
        let old: std::collections::BTreeMap<String, SpaceMeta> = serde_json::from_str(
            r#"{
                "local:plain": {"labels": {"team": "a"}, "created_at_unix": 5},
                "local:policy": {"egress": {"allow": ["192.168.1.0/24"], "deny": ["all"]}},
                "local:allow-only": {"egress": {"allow": ["10.0.0.5"]}}
            }"#,
        )
        .unwrap();
        for m in old.values() {
            assert!(m.network.is_none(), "from before network settings");
        }
        assert_eq!(
            migrate_meta(&old["local:plain"], false),
            NetworkSettings::internet()
        );
        assert_eq!(
            migrate_meta(&old["local:plain"], true),
            NetworkSettings::none()
        );
        let p = migrate_meta(&old["local:policy"], false);
        assert_eq!(p.preset, NetworkPreset::Custom);
        assert_eq!(p.custom.allow, ["192.168.1.0/24"]);
        assert_eq!(p.custom.deny, ["all"]);
        assert_eq!(
            p.widenings(),
            ["Allows your local network (192.168.1.0/24)"]
        );
        let a = migrate_meta(&old["local:allow-only"], false);
        assert_eq!(a.widenings(), ["Allows your local network (10.0.0.5/32)"]);
        // A new file round-trips with its settings.
        let mut m = old["local:plain"].clone();
        m.network = Some(NetworkSettings::none());
        m.network_generation = 1;
        let back: SpaceMeta = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn wire_forms_round_trip_and_are_checked() {
        let s = NetworkSettings {
            preset: NetworkPreset::Localhost,
            localhost_ports: vec![5432, 3000],
            custom: crate::egress::EgressPolicy {
                allow: vec!["1.1.1.1".into()],
                deny: vec!["8.8.8.8".into()],
            },
            widened_by: Some("me@now".into()),
            ..Default::default()
        };
        assert_eq!(from_pb(&to_pb(&s)).unwrap(), s);
        assert_eq!(from_host_pb(&to_host_pb(&s)).unwrap(), s);
        let none_base = NetworkSettings {
            preset: NetworkPreset::Custom,
            custom_base: NetworkPreset::None,
            ..Default::default()
        };
        assert_eq!(to_pb(&none_base).custom_base, "none");
        assert_eq!(from_pb(&to_pb(&none_base)).unwrap(), none_base);
        // Empty is internet.
        assert_eq!(
            from_pb(&dpb::NetworkSettings::default()).unwrap(),
            NetworkSettings::internet()
        );
        let bad = |p: dpb::NetworkSettings| from_pb(&p).unwrap_err().tag();
        assert_eq!(
            bad(dpb::NetworkSettings {
                preset: "open".into(),
                ..Default::default()
            }),
            "invalid_argument"
        );
        assert_eq!(
            bad(dpb::NetworkSettings {
                preset: "localhost".into(),
                localhost_ports: vec![70000],
                ..Default::default()
            }),
            "invalid_argument"
        );
        assert_eq!(
            bad(dpb::NetworkSettings {
                preset: "localhost".into(),
                ..Default::default()
            }),
            "invalid_argument"
        );
        assert_eq!(
            bad(dpb::NetworkSettings {
                deny: vec!["bad name!".into()],
                ..Default::default()
            }),
            "invalid_argument"
        );
    }

    #[test]
    fn a_child_gets_the_intersection_unless_widened() {
        let parent = SpaceMeta {
            network: Some(NetworkSettings::none()),
            ..Default::default()
        };
        let c = resolve_create(
            NetworkSettings::preset(NetworkPreset::Lan),
            Some(("local:p".into(), Some(parent.clone()))),
            false,
        )
        .unwrap();
        assert_eq!(c.settings, NetworkSettings::none());
        assert_eq!(c.parent.as_deref(), Some("local:p"));
        assert_eq!(c.root.as_deref(), Some("local:p"));
        // A grandchild keeps the root.
        let child = SpaceMeta {
            network: Some(NetworkSettings::internet()),
            parent: Some("local:p".into()),
            root: Some("local:p".into()),
            ..Default::default()
        };
        let g = resolve_create(
            NetworkSettings::internet(),
            Some(("local:c".into(), Some(child))),
            false,
        )
        .unwrap();
        assert_eq!(g.root.as_deref(), Some("local:p"));
        assert_eq!(g.depth, 1, "the parent record says depth 0 here");
        // Widened: what it asked, stamped.
        let w = resolve_create(
            NetworkSettings::preset(NetworkPreset::Lan),
            Some(("local:p".into(), Some(parent))),
            true,
        )
        .unwrap();
        assert_eq!(w.settings.preset, NetworkPreset::Lan);
        assert!(
            w.settings
                .widened_by
                .as_deref()
                .is_some_and(|s| s.contains('@'))
        );
        // A parent from before network settings counts as its migration.
        let old = meta(&[], &["all"]);
        let c = resolve_create(
            NetworkSettings::internet(),
            Some(("local:old".into(), Some(old))),
            false,
        )
        .unwrap();
        assert!(c.settings.custom.deny.contains(&"all".to_string()));
        // No parent: as asked; widen alone is refused.
        assert_eq!(
            resolve_create(NetworkSettings::none(), None, false)
                .unwrap()
                .settings,
            NetworkSettings::none()
        );
        assert_eq!(
            resolve_create(NetworkSettings::none(), None, true)
                .unwrap_err()
                .tag(),
            "invalid_argument"
        );
    }
}
