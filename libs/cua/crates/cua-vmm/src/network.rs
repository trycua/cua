//! Network settings of a Space: a preset, the user's own lists, and what
//! the host compiles them to ([`compile`] → [`Rules`]).
//!
//! **Presets.** [`NetworkPreset::Internet`] is the default: the public
//! internet and the Space's own tree. [`NetworkPreset::None`] reaches
//! nothing. [`NetworkPreset::Localhost`] and [`NetworkPreset::Lan`] add the
//! host's loopback (on listed ports) or the local network, each on top of
//! `internet` and independent of the other; [`NetworkPreset::LocalhostLan`]
//! (`localhost+lan`) adds both.
//! [`NetworkPreset::Custom`] starts from `internet` or `none` and adds the
//! user's allow and deny lists. [`NetworkPreset::Unprotected`] opens
//! everything but the platform denies.
//!
//! **Layers**, first match wins ([`compile`]):
//!
//! 1. Platform denies, always (also for `unprotected`): cloud metadata
//!    endpoints, other trees' Spaces, the daemon's ports on the host.
//! 2. The runtime's resolver on port 53.
//! 3. The host itself: its loopback through the runtime's aliases, its own
//!    addresses, and every address local to the enforcing point (a
//!    [`Rule::host_local`] rule: nft `fib daddr type local`, iptables
//!    `addrtype LOCAL`). Only the `localhost` preset opens the aliases, and
//!    only on the ports it lists. A custom allow does not lift these, so a
//!    child can never reach the host past its parent through a numeric
//!    allow.
//! 4. The user's allow list, then deny list (an allow is an exception to a
//!    deny, as before). User rules never beat layers 1 to 3. A child's
//!    lists hold its parent's denies too ([`NetworkSettings::intersect`]).
//! 5. The Space's own tree is allowed. After the user's lists: an explicit
//!    deny that names a member of the tree (or its whole range) blocks it;
//!    a deny that matches nothing in the tree changes nothing. On a tree's
//!    own bridge, members talk frame to frame, so those denies are enforced
//!    on the bridge as well ([`crate::egress::engine_ruleset`]).
//! 6. Default denies: private, link-local, CGNAT, loopback, multicast and
//!    other reserved ranges (`lan` lifts the private and link-local ones).
//! 7. Default action: accept, or drop for `none` and `custom` from `none`.
//!
//! **Trees.** A child Space's settings are the intersection of its own and
//! its parent's ([`NetworkSettings::intersect`]): it can only tighten.
//!
//! **Fail closed.** [`network_support`] answers, for every runtime, host OS
//! and placement, whether the runtime enforces the rules, can only run the
//! Space without network, or refuses it. [`plan`] turns settings and that
//! answer into what a start does; nothing but `unprotected` ever starts
//! less strict than asked.
//!
//! **Always on.** The secure default applies to every Space. The only way
//! out is an operator's emergency override, not a setting: `CUA_SECURE_DEFAULT=0`
//! makes a process keep the old open default ([`Mode::Legacy`]); `=1` forces the
//! secure one.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::egress::{EgressPolicy, Net, PolicyError, Proto, Reason, Rule, Rules};
use crate::types::BackendKind;

/// A Space's network preset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPreset {
    /// The public internet and the Space's own tree (the default).
    #[default]
    Internet,
    /// No network.
    None,
    /// `internet`, plus the host's loopback on listed ports.
    Localhost,
    /// `internet`, plus the local network.
    Lan,
    /// `internet`, plus the host's loopback on listed ports and the local
    /// network: `localhost` and `lan` together. The two extras are
    /// independent; this is the one preset that has both.
    #[serde(rename = "localhost+lan")]
    LocalhostLan,
    /// `internet` or `none`, plus the user's allow and deny lists.
    Custom,
    /// Everything but the platform denies.
    Unprotected,
}

impl NetworkPreset {
    /// Every preset.
    pub const ALL: [NetworkPreset; 7] = [
        NetworkPreset::Internet,
        NetworkPreset::None,
        NetworkPreset::Localhost,
        NetworkPreset::Lan,
        NetworkPreset::LocalhostLan,
        NetworkPreset::Custom,
        NetworkPreset::Unprotected,
    ];

    /// The word the CLI, SDK and JSON use (`internet`).
    pub fn as_str(self) -> &'static str {
        match self {
            NetworkPreset::Internet => "internet",
            NetworkPreset::None => "none",
            NetworkPreset::Localhost => "localhost",
            NetworkPreset::Lan => "lan",
            NetworkPreset::LocalhostLan => "localhost+lan",
            NetworkPreset::Custom => "custom",
            NetworkPreset::Unprotected => "unprotected",
        }
    }

    /// The name people see ("Internet only").
    pub fn label(self) -> &'static str {
        match self {
            NetworkPreset::Internet => "Internet only",
            NetworkPreset::None => "No network",
            NetworkPreset::Localhost => "Allow this computer's localhost",
            NetworkPreset::Lan => "Allow my local network",
            NetworkPreset::LocalhostLan => "Allow this computer's localhost and my local network",
            NetworkPreset::Custom => "Custom",
            NetworkPreset::Unprotected => "No protection",
        }
    }

    /// Whether the preset opens the host's loopback (on listed ports):
    /// `localhost`, alone or with `lan`.
    pub fn opens_localhost(self) -> bool {
        matches!(self, NetworkPreset::Localhost | NetworkPreset::LocalhostLan)
    }

    /// Whether the preset opens the local network: `lan`, alone or with
    /// `localhost`.
    pub fn opens_lan(self) -> bool {
        matches!(self, NetworkPreset::Lan | NetworkPreset::LocalhostLan)
    }

    /// The `internet` preset plus the extras asked for: `internet`,
    /// `localhost`, `lan` or `localhost+lan`.
    pub fn with_extras(localhost: bool, lan: bool) -> NetworkPreset {
        match (localhost, lan) {
            (false, false) => NetworkPreset::Internet,
            (true, false) => NetworkPreset::Localhost,
            (false, true) => NetworkPreset::Lan,
            (true, true) => NetworkPreset::LocalhostLan,
        }
    }

    /// Parses a preset word (case-insensitive). `default` is `internet`,
    /// as the old `network="default"` was; `lan+localhost` is
    /// `localhost+lan`.
    pub fn parse(text: &str) -> Result<NetworkPreset, PolicyError> {
        let t = text.trim().to_ascii_lowercase();
        if t == "default" {
            return Ok(NetworkPreset::Internet);
        }
        if t == "lan+localhost" {
            return Ok(NetworkPreset::LocalhostLan);
        }
        NetworkPreset::ALL
            .into_iter()
            .find(|p| p.as_str() == t)
            .ok_or_else(|| {
                PolicyError(format!(
                    "network {text:?}: expected internet, none, localhost, lan, localhost+lan, \
                     custom or unprotected"
                ))
            })
    }
}

impl std::fmt::Display for NetworkPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Most ports the `localhost` preset lists.
pub const MAX_LOCALHOST_PORTS: usize = 64;

/// Where a Space's internet traffic leaves ("Exit through"): this computer
/// (the hotspot), another Space of its tree, or a SOCKS5 proxy. `None` on
/// [`NetworkSettings::via`] is the Space's own network (the default).
///
/// With an exit the Space reaches the internet only through it: its own
/// direct internet is closed ([`compile`] drops it), the exit applies the
/// Space's rules first ([`NetworkSettings::exit_rules`]), and when the exit
/// goes away the Space has no internet (fail closed). Its tree, its
/// resolver and the host ports it may reach stay direct.
/// ([`NetworkSettings::exit_settings`] are the rules the exit applies.)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum EgressVia {
    /// This computer's network (what the hotspot gives a Space).
    ThisMachine,
    /// Another Space of the tree, by id (it must offer the `exit` feature).
    Space(String),
    /// A SOCKS5 proxy: `socks5://[user:pass@]host:port` (`socks5h://` too).
    Socks5(String),
}

impl EgressVia {
    /// The word: `this-computer`, `space:<id>` or the proxy's URL.
    pub fn as_word(&self) -> String {
        match self {
            EgressVia::ThisMachine => "this-computer".into(),
            EgressVia::Space(id) => format!("space:{id}"),
            EgressVia::Socks5(url) => url.clone(),
        }
    }

    /// Parses [`EgressVia::as_word`]'s words (`hotspot` and `this` mean this
    /// machine). `internet` or empty is `None`: the Space's own network.
    pub fn parse(word: &str) -> Result<Option<EgressVia>, PolicyError> {
        let w = word.trim();
        match w.to_ascii_lowercase().as_str() {
            "" | "internet" | "direct" => return Ok(None),
            "this-computer" | "this-machine" | "this" | "hotspot" | "this computer" => {
                return Ok(Some(EgressVia::ThisMachine));
            }
            _ => {}
        }
        if let Some(id) = w.strip_prefix("space:") {
            let id = id.trim();
            if id.is_empty() {
                return Err(PolicyError(
                    "space: names the exit Space (space:<id or name>)".into(),
                ));
            }
            return Ok(Some(EgressVia::Space(id.to_string())));
        }
        let lower = w.to_ascii_lowercase();
        if lower.starts_with("socks5://") || lower.starts_with("socks5h://") {
            socks5_target(w)?;
            return Ok(Some(EgressVia::Socks5(w.to_string())));
        }
        Err(PolicyError(format!(
            "exit {w:?}: internet, this-computer, space:<name> or socks5://host:port"
        )))
    }

    /// One plain line ("This computer", "the Space research-swarm",
    /// "SOCKS5 proxy 10.0.0.5:1080"), credentials left out.
    pub fn label(&self) -> String {
        match self {
            EgressVia::ThisMachine => "This computer".into(),
            EgressVia::Space(id) => format!("the Space {}", id.rsplit(':').next().unwrap_or(id)),
            EgressVia::Socks5(url) => match socks5_target(url) {
                Ok(t) => format!("SOCKS5 proxy {}:{}", t.host, t.port),
                Err(_) => "a SOCKS5 proxy".into(),
            },
        }
    }
}

impl TryFrom<String> for EgressVia {
    type Error = PolicyError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        EgressVia::parse(&s)?.ok_or_else(|| PolicyError("an exit, not internet".into()))
    }
}

impl From<EgressVia> for String {
    fn from(v: EgressVia) -> String {
        v.as_word()
    }
}

/// A SOCKS5 proxy's address and credentials, from its URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socks5Target {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub pass: Option<String>,
    /// `socks5h`: names are resolved by the proxy (always, for this exit).
    pub remote_dns: bool,
}

/// Parses `socks5://[user:pass@]host:port`.
pub fn socks5_target(url: &str) -> Result<Socks5Target, PolicyError> {
    let bad = || PolicyError(format!("{url:?}: socks5://[user:pass@]host:port"));
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    let remote_dns = match scheme.to_ascii_lowercase().as_str() {
        "socks5" => false,
        "socks5h" => true,
        _ => return Err(bad()),
    };
    let rest = rest.trim_end_matches('/');
    let (creds, hostport) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (host, port) = if let Some(r) = hostport.strip_prefix('[') {
        let (h, p) = r.split_once("]:").ok_or_else(bad)?;
        (h.to_string(), p)
    } else {
        let (h, p) = hostport.rsplit_once(':').ok_or_else(bad)?;
        (h.to_string(), p)
    };
    let port: u16 = port.parse().ok().filter(|p| *p > 0).ok_or_else(bad)?;
    if host.is_empty() || host.contains('/') {
        return Err(bad());
    }
    let (user, pass) = match creds {
        Some(c) => {
            let (u, p) = c.split_once(':').unwrap_or((c, ""));
            (Some(u.to_string()), Some(p.to_string()))
        }
        None => (None, None),
    };
    Ok(Socks5Target {
        host,
        port,
        user,
        pass,
        remote_dns,
    })
}

/// A Space's network settings: stored with the Space, carried to its
/// runtime in the start spec, and compiled to [`Rules`] by [`compile`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// The preset.
    #[serde(default)]
    pub preset: NetworkPreset,
    /// `localhost`: the host loopback ports the Space may reach. Empty is
    /// refused unless `all_localhost_ports`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub localhost_ports: Vec<u16>,
    /// `localhost`: every port of the host's loopback (the daemon's stay
    /// blocked).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_localhost_ports: bool,
    /// The user's allow and deny lists: what `custom` adds to its base, and
    /// what any other preset adds on top.
    #[serde(default, skip_serializing_if = "EgressPolicy::is_empty")]
    pub custom: EgressPolicy,
    /// What `custom` starts from: `internet` or `none`.
    #[serde(default)]
    pub custom_base: NetworkPreset,
    /// Set when the Space was allowed more than its parent (`--widen`):
    /// who and when (`user@2026-10-26T12:00:00Z`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widened_by: Option<String>,
    /// Where its internet traffic leaves ([`EgressVia`]); `None`: its own
    /// network. A child takes its parent's and can't leave it
    /// ([`NetworkSettings::intersect`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<EgressVia>,
}

impl NetworkSettings {
    /// Settings with `preset` and nothing else.
    pub fn preset(preset: NetworkPreset) -> NetworkSettings {
        NetworkSettings {
            preset,
            ..Default::default()
        }
    }

    /// The default: `internet`.
    pub fn internet() -> NetworkSettings {
        NetworkSettings::preset(NetworkPreset::Internet)
    }

    /// `none`.
    pub fn none() -> NetworkSettings {
        NetworkSettings::preset(NetworkPreset::None)
    }

    /// Checks the settings: the lists parse, `custom_base` is `internet` or
    /// `none`, and `localhost` names its ports (or says all).
    pub fn validate(&self) -> Result<(), PolicyError> {
        // Names (`example.com`, `*.example.com`) are checked by
        // `is_domain`; everything else must be an address.
        let (addresses, domains) = self.custom.split_domains();
        addresses.parse()?;
        if domains.allow.len() > crate::egress::MAX_ENTRIES
            || domains.deny.len() > crate::egress::MAX_ENTRIES
        {
            return Err(PolicyError(format!(
                "at most {} names in a list",
                crate::egress::MAX_ENTRIES
            )));
        }
        if !matches!(
            self.custom_base,
            NetworkPreset::Internet | NetworkPreset::None
        ) {
            return Err(PolicyError(format!(
                "custom starts from internet or none, not {}",
                self.custom_base
            )));
        }
        if self.preset == NetworkPreset::None && !self.custom.is_empty() {
            return Err(PolicyError(
                "no network takes no allow or deny list; use custom from none to allow a few \
                 destinations"
                    .into(),
            ));
        }
        let localhost = self.preset.opens_localhost();
        if !localhost && (!self.localhost_ports.is_empty() || self.all_localhost_ports) {
            return Err(PolicyError(
                "localhost ports go with the localhost preset".into(),
            ));
        }
        if localhost && self.localhost_ports.is_empty() && !self.all_localhost_ports {
            return Err(PolicyError(
                "the localhost preset needs the ports to open (localhost:5432,3000), or all \
                 ports said explicitly"
                    .into(),
            ));
        }
        if self.localhost_ports.contains(&0) {
            return Err(PolicyError("localhost port 0 is not a port".into()));
        }
        if self.localhost_ports.len() > MAX_LOCALHOST_PORTS {
            return Err(PolicyError(format!(
                "{} localhost ports: at most {MAX_LOCALHOST_PORTS}",
                self.localhost_ports.len()
            )));
        }
        if self.via.is_some() && self.preset == NetworkPreset::None {
            return Err(PolicyError(
                "no network has no exit; pick another preset to exit through".into(),
            ));
        }
        if let Some(EgressVia::Socks5(url)) = &self.via {
            socks5_target(url)?;
        }
        Ok(())
    }

    /// The rules the exit applies to each connection it makes for the
    /// Space ([`EgressVia`]): the Space's own settings as if it had no exit,
    /// lists and preset included. Its tree, the host and the platform's
    /// denies stay closed there too.
    pub fn exit_settings(&self) -> NetworkSettings {
        NetworkSettings {
            via: None,
            ..self.clone()
        }
    }

    /// What the preset starts from, `custom` resolved: `none` for `none`
    /// and `custom` from `none`, the preset itself otherwise.
    pub fn base(&self) -> NetworkPreset {
        match self.preset {
            NetworkPreset::Custom => self.custom_base,
            p => p,
        }
    }

    /// Whether a connection no rule names is dropped.
    pub fn default_drops(&self) -> bool {
        self.base() == NetworkPreset::None
    }

    /// The settings an existing Space gets from what it was created with
    /// before network settings existed (migration): `network=none` is
    /// `none`, an egress policy is `custom` from `internet` with its lists
    /// kept, nothing is `internet`.
    pub fn migrate(egress: &EgressPolicy, restrict_network: bool) -> NetworkSettings {
        if restrict_network {
            // The old `none` cut everything, whatever the lists said.
            return NetworkSettings::none();
        }
        if egress.is_empty() {
            return NetworkSettings::internet();
        }
        NetworkSettings {
            preset: NetworkPreset::Custom,
            custom: egress.clone(),
            custom_base: NetworkPreset::Internet,
            ..Default::default()
        }
    }

    /// The settings the old create options ask for: `network=none` and an
    /// egress policy (the same mapping as [`NetworkSettings::migrate`]).
    pub fn from_legacy(egress: &EgressPolicy, restrict_network: bool) -> NetworkSettings {
        NetworkSettings::migrate(egress, restrict_network)
    }

    /// These settings with `policy` as their lists (a legacy
    /// `set_egress`): `internet` with lists becomes `custom` from
    /// `internet`, `none` with lists `custom` from `none`, and `custom`
    /// without lists goes back to its base.
    pub fn with_lists(settings: Option<NetworkSettings>, policy: &EgressPolicy) -> NetworkSettings {
        use NetworkPreset as P;
        let mut s = settings.unwrap_or_else(NetworkSettings::internet);
        s.custom = policy.clone();
        match s.preset {
            P::Internet | P::None if !policy.is_empty() => {
                s.custom_base = s.preset;
                s.preset = P::Custom;
            }
            P::Custom if policy.is_empty() => {
                s.preset = s.custom_base;
                s.custom_base = P::Internet;
            }
            _ => {}
        }
        s
    }

    /// What these settings open beyond `internet`, one plain line each
    /// ("Allows your local network (192.168.1.0/24)"). Empty for `internet`
    /// and anything stricter.
    pub fn widenings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.preset.opens_localhost() {
            out.push(if self.all_localhost_ports {
                "Allows this computer's localhost (all ports)".to_string()
            } else {
                format!(
                    "Allows this computer's localhost (ports {})",
                    join_ports(&self.localhost_ports)
                )
            });
        }
        if self.preset.opens_lan() {
            out.push("Allows your local network".to_string());
        }
        if self.preset == NetworkPreset::Unprotected {
            out.push("No protection: allows everything but cloud metadata".to_string());
        }
        if let Ok(rules) = self.custom.split_domains().0.parse() {
            for r in rules.rules.iter().filter(|r| r.accept) {
                let lan = default_lan().iter().any(|n| n.overlaps(&r.net));
                let reserved = default_reserved().iter().any(|n| n.overlaps(&r.net));
                if lan {
                    out.push(format!("Allows your local network ({})", r.net.cidr()));
                } else if reserved {
                    out.push(format!("Allows a reserved range ({})", r.net.cidr()));
                }
            }
        }
        out
    }

    /// One plain line for people ("Internet only", "Allow this computer's
    /// localhost (ports 5432, 3000)", "Internet only, through This
    /// machine").
    pub fn summary(&self) -> String {
        let base = self.preset_summary();
        match &self.via {
            Some(v) => format!("{base}, through {}", v.label()),
            None => base,
        }
    }

    fn preset_summary(&self) -> String {
        match self.preset {
            p if p.opens_localhost() && self.all_localhost_ports => {
                format!("{} (all ports)", self.preset.label())
            }
            p if p.opens_localhost() => format!(
                "{} (ports {})",
                self.preset.label(),
                join_ports(&self.localhost_ports)
            ),
            NetworkPreset::Custom => format!(
                "Custom, from {}",
                match self.custom_base {
                    NetworkPreset::None => "no network",
                    _ => "internet only",
                }
            ),
            p => p.label().to_string(),
        }
    }

    /// The settings of a child Space under a parent with `parent`: no more
    /// open than either (the stricter preset, the localhost ports both
    /// list, every deny of both, and only the allows the other side also
    /// allows). `widened_by` is dropped: a widened child skips this.
    pub fn intersect(&self, parent: &NetworkSettings) -> NetworkSettings {
        use NetworkPreset as P;
        let (preset, base) = meet(self, parent);
        let mut out = NetworkSettings {
            preset,
            custom_base: if preset == P::Custom {
                base
            } else {
                P::Internet
            },
            ..Default::default()
        };
        if preset.opens_localhost() {
            let both_all = self.all_localhost_ports && parent.all_localhost_ports;
            out.all_localhost_ports = both_all;
            if !both_all {
                let mine: Vec<u16> = ports_of(self);
                let theirs: Vec<u16> = ports_of(parent);
                out.localhost_ports = match (self.all_localhost_ports, parent.all_localhost_ports) {
                    (true, _) => theirs,
                    (_, true) => mine,
                    _ => mine.into_iter().filter(|p| theirs.contains(p)).collect(),
                };
                if out.localhost_ports.is_empty() {
                    // No port both list: the localhost extra goes, a shared
                    // `lan` extra stays.
                    out.preset = NetworkPreset::with_extras(false, preset.opens_lan());
                }
            }
        }
        let ctx = NetworkContext::default();
        let compiled = |s: &NetworkSettings| {
            compile_named(s, &ctx, Mode::Secure)
                .map(|(r, _)| r)
                .unwrap_or_else(|_| drop_all())
        };
        let mine = compiled(self);
        let theirs = compiled(parent);
        // A name stays allowed only where the other side lets it through
        // too: it names it, or it reaches the internet and doesn't block it.
        let name_ok = |other: &NetworkSettings, name: &str| {
            let (_, d) = other.custom.split_domains();
            let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
            if d.deny.contains(&name) {
                false
            } else {
                d.allow.contains(&name) || !other.default_drops()
            }
        };
        let keep =
            |list: &[String], other: &Rules, other_settings: &NetworkSettings| -> Vec<String> {
                list.iter()
                    .filter(|e| match Net::parse(e) {
                        _ if crate::egress::is_domain(e) => name_ok(other_settings, e),
                        Ok(net) => other.allows_net(&net),
                        // `all` (both families): kept only if the other side
                        // allows both.
                        Err(_) => ["0.0.0.0/0", "::/0"]
                            .iter()
                            .all(|a| other.allows_net(&Net::parse(a).expect("valid"))),
                    })
                    .cloned()
                    .collect()
            };
        let mut allow = keep(&self.custom.allow, &theirs, parent);
        for e in keep(&parent.custom.allow, &mine, self) {
            if !allow.contains(&e) {
                allow.push(e);
            }
        }
        let mut deny = self.custom.deny.clone();
        for e in &parent.custom.deny {
            if !deny.contains(e) {
                deny.push(e.clone());
            }
        }
        if out.preset != P::None {
            out.custom = EgressPolicy { allow, deny };
            // The exit: the parent's, which a child can't leave (a parent
            // sends its whole tree through a proxy Space); else the child's
            // own.
            out.via = parent.via.clone().or_else(|| self.via.clone());
        }
        out
    }
}

fn ports_of(s: &NetworkSettings) -> Vec<u16> {
    if s.preset.opens_localhost() {
        s.localhost_ports.clone()
    } else {
        Vec::new()
    }
}

/// The stricter of two settings' presets, as (preset, custom base).
fn meet(a: &NetworkSettings, b: &NetworkSettings) -> (NetworkPreset, NetworkPreset) {
    use NetworkPreset as P;
    let (pa, pb) = (a.preset, b.preset);
    if pa == P::None || pb == P::None {
        return (P::None, P::Internet);
    }
    if a.default_drops() || b.default_drops() {
        // Either reaches nothing it does not list.
        return (P::Custom, P::None);
    }
    match (pa, pb) {
        (P::Unprotected, other) | (other, P::Unprotected) => (other, P::Internet),
        (P::Custom, _) | (_, P::Custom) => (P::Custom, P::Internet),
        // The two extras are independent: the meet keeps those both open.
        (P::Internet | P::Lan | P::Localhost | P::LocalhostLan, _) => (
            P::with_extras(
                pa.opens_localhost() && pb.opens_localhost(),
                pa.opens_lan() && pb.opens_lan(),
            ),
            P::Internet,
        ),
        (P::None, _) => (P::None, P::Internet),
    }
}

fn join_ports(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn drop_all() -> Rules {
    Rules {
        rules: all_nets()
            .into_iter()
            .map(|n| Rule::net(n, false, Reason::DefaultNone))
            .collect(),
    }
}

fn all_nets() -> [Net; 2] {
    [
        Net::parse("0.0.0.0/0").expect("valid"),
        Net::parse("::/0").expect("valid"),
    ]
}

/// Cloud metadata endpoints: AWS (IPv4 and IPv6), ECS, Azure's wireserver
/// and Alibaba's. Always blocked.
pub const METADATA: [&str; 5] = [
    "169.254.169.254/32",
    "169.254.170.2/32",
    "fd00:ec2::254/128",
    "168.63.129.16/32",
    "100.100.100.200/32",
];

/// Private and link-local networks: blocked unless `lan`.
pub const DEFAULT_DENY_LAN: [&str; 6] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
];

/// Loopback, CGNAT (Tailscale's range too), multicast and other reserved
/// ranges: always blocked by default (no preset lifts them).
pub const DEFAULT_DENY_RESERVED: [&str; 11] = [
    "0.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "192.0.0.0/24",
    "198.18.0.0/15",
    "224.0.0.0/4",
    "240.0.0.0/4",
    "::1/128",
    "::ffff:0:0/96",
    "64:ff9b::/96",
    "ff00::/8",
];

fn nets(list: &[&str]) -> Vec<Net> {
    list.iter()
        .map(|s| Net::parse(s).expect("a valid built-in network"))
        .collect()
}

fn default_lan() -> Vec<Net> {
    nets(&DEFAULT_DENY_LAN)
}

fn default_reserved() -> Vec<Net> {
    nets(&DEFAULT_DENY_RESERVED)
}

/// What only the runtime knows, for [`compile`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkContext {
    /// The runtime's own resolvers: allowed on port 53.
    pub resolvers: Vec<IpAddr>,
    /// The addresses through which the guest reaches the host's loopback
    /// (`10.0.2.2`, the bridge gateway, `192.168.64.1`).
    pub host_aliases: Vec<IpAddr>,
    /// The host's own interface addresses.
    pub host_addrs: Vec<IpAddr>,
    /// The other Spaces of this Space's tree: allowed.
    pub tree: Vec<Net>,
    /// Spaces of other trees: always blocked.
    pub other_trees: Vec<Net>,
    /// The daemon's ports on the host: always blocked on every host
    /// address and alias.
    pub daemon_ports: Vec<u16>,
}

/// The TCP port the cua daemon listens on, from its discovery file under
/// the cua home (`None` when no daemon has written one). Guests are never
/// let through to it ([`Reason::PlatformDaemon`]).
pub fn daemon_ports() -> Vec<u16> {
    #[derive(Deserialize)]
    struct Discovery {
        #[serde(default)]
        loopback_url: Option<String>,
    }
    let path = crate::host::cua_home().join("daemon.json");
    let Some(url) = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Discovery>(&b).ok())
        .and_then(|d| d.loopback_url)
    else {
        return Vec::new();
    };
    url.rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        .into_iter()
        .collect()
}

/// This host's own interface addresses (loopback left out): what a guest
/// must not reach through a runtime that maps them to the host (pasta).
/// Enumerated at each start.
pub fn host_addresses() -> Vec<IpAddr> {
    #[cfg(unix)]
    {
        let mut out: Vec<IpAddr> = Vec::new();
        // SAFETY: getifaddrs fills a list we only read and then free.
        unsafe {
            let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
            if libc::getifaddrs(&mut head) != 0 {
                return out;
            }
            let mut cur = head;
            while !cur.is_null() {
                let a = (*cur).ifa_addr;
                if !a.is_null() {
                    let ip = match i32::from((*a).sa_family) {
                        libc::AF_INET => {
                            let v4 = &*(a as *const libc::sockaddr_in);
                            Some(IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(
                                v4.sin_addr.s_addr,
                            ))))
                        }
                        libc::AF_INET6 => {
                            let v6 = &*(a as *const libc::sockaddr_in6);
                            Some(IpAddr::V6(std::net::Ipv6Addr::from(v6.sin6_addr.s6_addr)))
                        }
                        _ => None,
                    };
                    if let Some(ip) = ip.filter(|i| !i.is_loopback())
                        && !out.contains(&ip)
                    {
                        out.push(ip);
                    }
                }
                cur = (*cur).ifa_next;
            }
            libc::freeifaddrs(head);
        }
        out
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// The rules a VM runtime enforces: its network settings compiled with
/// `ctx` under the secure default (an old egress policy migrated when it
/// has none), or the old policy alone under `CUA_SECURE_DEFAULT=0`.
pub fn vm_rules(
    settings: Option<&NetworkSettings>,
    legacy: Option<&EgressPolicy>,
    restrict_network: bool,
    ctx: &NetworkContext,
) -> Result<Rules, PolicyError> {
    match Mode::from_env() {
        Mode::Secure => {
            let s = match settings {
                Some(s) => s.clone(),
                None => {
                    NetworkSettings::migrate(&legacy.cloned().unwrap_or_default(), restrict_network)
                }
            };
            compile_mode(&s, ctx, Mode::Secure)
        }
        Mode::Legacy => match legacy {
            Some(p) => p.parse(),
            None => Ok(Rules::default()),
        },
    }
}

/// The host loopback ports a VM's network namespace forwards to the host
/// (pasta `-T`) for the `localhost` preset: its listed ports, never the
/// daemon's. Every port is refused for VMs: a namespace forwards listed
/// ports only. Empty for any other preset or under the old default.
pub fn vm_localhost_ports(settings: Option<&NetworkSettings>) -> Result<Vec<u16>, PolicyError> {
    let Some(s) = settings.filter(|s| s.preset.opens_localhost()) else {
        return Ok(Vec::new());
    };
    if Mode::from_env() == Mode::Legacy {
        return Ok(Vec::new());
    }
    if s.all_localhost_ports {
        return Err(PolicyError(
            "a VM opens the localhost ports you list, not all of them: list them \
             (localhost:5432,3000)"
                .into(),
        ));
    }
    let daemon = daemon_ports();
    let mut out: Vec<u16> = s
        .localhost_ports
        .iter()
        .copied()
        .filter(|p| !daemon.contains(p))
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// pasta's `-T` argument for `ports` (`none` without any).
pub fn pasta_out_spec(ports: &[u16]) -> String {
    if ports.is_empty() {
        "none".into()
    } else {
        ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Whether the secure default applies, or the old open one. The secure
/// default always applies unless an operator forces [`SECURE_DEFAULT_ENV`] to `0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The secure default: presets, platform and default denies.
    Secure,
    /// The old open default: only the user's own lists, and `none`.
    Legacy,
}

/// The environment variable that forces the mode, an operator's emergency override:
/// `0` (or `false`, `off`, `no`) the old open default, `1` (or `true`, `on`,
/// `yes`) the secure one.
pub const SECURE_DEFAULT_ENV: &str = "CUA_SECURE_DEFAULT";

/// What a call says when the operator override [`SECURE_DEFAULT_ENV`]`=0` has turned the secure
/// default off for this process and the call needs it.
pub fn secure_default_off_message() -> String {
    format!(
        "This needs the secure network default, which {SECURE_DEFAULT_ENV}=0 has turned off for this process. Unset it and restart."
    )
}

impl Mode {
    /// The mode of this process: [`SECURE_DEFAULT_ENV`] when it is set, else the secure
    /// default. It decides the network of Spaces made from now on only.
    pub fn from_env() -> Mode {
        if let Some(m) = *MODE_HELD.lock().unwrap_or_else(|e| e.into_inner()) {
            return m;
        }
        Mode::from_value(std::env::var(SECURE_DEFAULT_ENV).ok().as_deref())
    }

    /// [`Mode::from_env`] for a value of the variable.
    pub fn from_value(v: Option<&str>) -> Mode {
        match v.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("0" | "false" | "off" | "no") => Mode::Legacy,
            _ => Mode::Secure,
        }
    }
}

/// A process-wide mode a test holds (see [`hold_mode`]).
static MODE_HELD: std::sync::Mutex<Option<Mode>> = std::sync::Mutex::new(None);
/// Tests that hold a mode take turns.
static MODE_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds this process's mode at `mode` until dropped. Real-VM tests that need an open network
/// hold [`Mode::Legacy`] instead of setting the environment. Holders take turns.
#[must_use = "the mode is held until this is dropped"]
pub struct ModeHold {
    _turn: std::sync::MutexGuard<'static, ()>,
}

/// Waits for its turn, then holds [`Mode::from_env`] at `mode` (see [`ModeHold`]).
pub fn hold_mode(mode: Mode) -> ModeHold {
    let turn = MODE_TURN.lock().unwrap_or_else(|e| e.into_inner());
    *MODE_HELD.lock().unwrap_or_else(|e| e.into_inner()) = Some(mode);
    ModeHold { _turn: turn }
}

impl Drop for ModeHold {
    fn drop(&mut self) {
        *MODE_HELD.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Whether a Space with `settings` (`None`: none recorded) is held to the
/// secure default: it is by default ([`Mode::from_env`]),
/// unless the user chose No protection.
pub fn protects(settings: Option<&NetworkSettings>) -> bool {
    Mode::from_env() == Mode::Secure
        && settings.is_none_or(|s| s.preset != NetworkPreset::Unprotected)
}

/// Why rules by name are refused where the host's firewall enforces a
/// Space's network: it judges addresses, and no resolver of ours stands
/// between the guest and its answers there yet.
pub const DOMAINS_GATEWAY_ONLY: &str = "rules by name (example.com) are kept only by the Cua \
     network gateway (macOS microVMs and VMs, QEMU on macOS and Windows); on this runtime use \
     addresses or CIDRs";

/// [`compile_mode`] for a runtime behind the Cua network gateway, which is
/// the guest's resolver and so keeps rules by name too: the address rules,
/// and the names (empty under the old default).
pub fn compile_named(
    settings: &NetworkSettings,
    ctx: &NetworkContext,
    mode: Mode,
) -> Result<(Rules, crate::egress::Domains), PolicyError> {
    let (addresses, domains) = settings.custom.split_domains();
    let plain = NetworkSettings {
        custom: addresses,
        ..settings.clone()
    };
    let rules = compile_mode(&plain, ctx, mode)?;
    Ok((
        rules,
        if mode == Mode::Secure {
            domains
        } else {
            Default::default()
        },
    ))
}

/// [`vm_rules`] for a VM behind the Cua network gateway: with the names.
pub fn vm_rules_named(
    settings: Option<&NetworkSettings>,
    legacy: Option<&EgressPolicy>,
    restrict_network: bool,
    ctx: &NetworkContext,
) -> Result<(Rules, crate::egress::Domains), PolicyError> {
    match Mode::from_env() {
        Mode::Secure => {
            let s = match settings {
                Some(s) => s.clone(),
                None => {
                    NetworkSettings::migrate(&legacy.cloned().unwrap_or_default(), restrict_network)
                }
            };
            compile_named(&s, ctx, Mode::Secure)
        }
        Mode::Legacy => Ok((
            match legacy {
                Some(p) => p.parse()?,
                None => Rules::default(),
            },
            Default::default(),
        )),
    }
}

/// Compiles `settings` for a runtime whose facts are `ctx`, under the mode
/// the environment says ([`Mode::from_env`]).
pub fn compile(settings: &NetworkSettings, ctx: &NetworkContext) -> Result<Rules, PolicyError> {
    compile_mode(settings, ctx, Mode::from_env())
}

/// [`compile`] under `mode` (see the module docs for the layers).
pub fn compile_mode(
    settings: &NetworkSettings,
    ctx: &NetworkContext,
    mode: Mode,
) -> Result<Rules, PolicyError> {
    settings.validate()?;
    let (addresses, domains) = settings.custom.split_domains();
    if !domains.is_empty() {
        return Err(PolicyError(DOMAINS_GATEWAY_ONLY.into()));
    }
    let mut user = addresses.parse()?;
    // With an exit, the Space's internet goes only through it: the user's
    // allows are the exit's to apply ([`NetworkSettings::exit_settings`]);
    // its denies still close a destination here too.
    let exits = settings.via.is_some();
    if exits {
        user.rules.retain(|r| !r.accept);
    }
    if mode == Mode::Legacy {
        let mut rules = user;
        if settings.default_drops() || exits {
            rules.rules.extend(drop_all().rules);
        }
        return Ok(rules);
    }
    let mut r: Vec<Rule> = Vec::new();
    // 1a. A cloud platform's own resolver (Azure's wireserver, GCP's
    //     metadata server) is also a metadata endpoint. It is opened on
    //     port 53 only, ahead of the metadata deny, so private DNS zones
    //     resolve while its HTTP stays closed.
    if settings.preset != NetworkPreset::None {
        for ip in dedup(ctx.resolvers.iter().copied()) {
            if crate::resolver::is_platform_resolver(ip) {
                r.push(Rule {
                    ports: Some(vec![53]),
                    ..Rule::net(Net::host(ip), true, Reason::Resolver)
                });
            }
        }
    }
    // 1. Platform denies.
    for n in nets(&METADATA) {
        r.push(Rule::net(n, false, Reason::PlatformMetadata));
    }
    for n in &ctx.other_trees {
        r.push(Rule::net(*n, false, Reason::PlatformOtherTree));
    }
    let host: Vec<IpAddr> = dedup(ctx.host_aliases.iter().chain(&ctx.host_addrs).copied());
    if !ctx.daemon_ports.is_empty() {
        for ip in &host {
            r.push(Rule {
                ports: Some(ctx.daemon_ports.clone()),
                proto: Some(Proto::Tcp),
                ..Rule::net(Net::host(*ip), false, Reason::PlatformDaemon)
            });
        }
        r.push(Rule {
            ports: Some(ctx.daemon_ports.clone()),
            proto: Some(Proto::Tcp),
            ..Rule::host_local(false, Reason::PlatformDaemon)
        });
    }
    let none = settings.preset == NetworkPreset::None;
    let unprotected = settings.preset == NetworkPreset::Unprotected;
    // 2. The runtime's resolver, on 53 (it may be a host address, as a
    //    microVM's gateway is).
    if !none {
        for ip in dedup(ctx.resolvers.iter().copied()) {
            r.push(Rule {
                ports: Some(vec![53]),
                ..Rule::net(Net::host(ip), true, Reason::Resolver)
            });
        }
    }
    // 3. The host: only `localhost` opens its aliases, on its ports.
    if !unprotected {
        if settings.preset.opens_localhost() {
            for ip in dedup(ctx.host_aliases.iter().copied()) {
                r.push(Rule {
                    ports: (!settings.all_localhost_ports)
                        .then(|| settings.localhost_ports.clone()),
                    ..Rule::net(Net::host(ip), true, Reason::PresetLocalhost)
                });
            }
        }
        for ip in dedup(ctx.host_aliases.iter().copied()) {
            r.push(Rule::net(Net::host(ip), false, Reason::DefaultHostLoopback));
        }
        for ip in dedup(ctx.host_addrs.iter().copied()) {
            if !ctx.host_aliases.contains(&ip) {
                r.push(Rule::net(Net::host(ip), false, Reason::DefaultHost));
            }
        }
        // Every address of the enforcing host, whatever it is.
        r.push(Rule::host_local(false, Reason::DefaultHost));
    }
    // 4. The user's lists: an explicit deny beats the tree's own allow.
    r.extend(user.rules);
    // 5. The Space's own tree (`custom` from `none` keeps it: own tree
    //    only, no internet).
    if !none {
        for n in &ctx.tree {
            r.push(Rule::net(*n, true, Reason::Tree));
        }
    }
    // 6. Default denies.
    if !unprotected {
        for n in default_reserved() {
            r.push(Rule::net(n, false, Reason::DefaultReserved));
        }
        if !settings.preset.opens_lan() {
            for n in default_lan() {
                r.push(Rule::net(n, false, Reason::DefaultLan));
            }
        }
    }
    // 7. The default action. With an exit, nothing else leaves directly:
    //    if the exit goes away the Space has no internet (fail closed).
    if settings.default_drops() {
        r.extend(drop_all().rules);
    } else if exits {
        r.extend(
            all_nets()
                .into_iter()
                .map(|n| Rule::net(n, false, Reason::DefaultExit)),
        );
    }
    Ok(Rules { rules: r })
}

fn dedup(it: impl Iterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for ip in it {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

// ------------------------------------------------------------- support

/// Where a Space runs, as [`network_support`] sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Placement {
    /// On this computer.
    Local,
    /// On one of your computers (a pool host): its own runtime enforces.
    PoolHost,
    /// In your own cloud account or a Spaces provider (`on=<provider>`).
    Provider,
    /// On a provider because this computer was full (`on=auto` overflow).
    Overflow,
}

impl Placement {
    /// Every placement.
    pub const ALL: [Placement; 4] = [
        Placement::Local,
        Placement::PoolHost,
        Placement::Provider,
        Placement::Overflow,
    ];

    /// `local`, `pool_host`, `provider`, `overflow`.
    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Local => "local",
            Placement::PoolHost => "pool_host",
            Placement::Provider => "provider",
            Placement::Overflow => "overflow",
        }
    }
}

/// What a runtime does with network settings ([`network_support`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkSupport {
    /// The host enforces the rules, outside the guest, at `point`.
    Enforced {
        /// Where, for people.
        point: &'static str,
    },
    /// It can't enforce rules yet, but can run the Space with no network.
    NoNetworkOnly {
        /// Why, in plain words.
        why: &'static str,
    },
    /// It can't run a protected Space at all.
    Refuse {
        /// Why, in plain words, and what to do instead.
        why: &'static str,
    },
}

/// The engine-host point of the container backends.
pub const ENGINE_POINT: &str = "nftables in the container engine's own network namespace, on the \
     Space's bridge port (outside the container, runc and gVisor alike)";
/// The namespace point of the Linux VMs.
pub const NS_POINT: &str = "iptables in the network namespace pasta creates around the VM process";

const WHY_QEMU: &str = "QEMU on this system can't filter one VM's connections yet";
/// Why a macOS VM (Lume) is refused under the secure default.
pub const WHY_LUME: &str = "macOS VMs can't protect your network yet, and Cua reaches a macOS \
     VM over its network, so it can't start without one. Choose No protection to start it.";
/// Why Apple's Containerization is refused under the secure default.
pub const WHY_CONTAINERIZATION: &str = "Apple's Containerization can't protect your network: its \
     guests sit on Apple's vmnet network, which only pf can filter (pf needs root), and a firewall \
     inside the guest would not hold against the Space's own root. Use the Cua microVM monitor \
     (`--runtime vz`: cua-vz and cua-microvm-init, which the Cua Spaces app ships next to `cua`) \
     or choose No protection.";
const WHY_QEMU_DOCKER: &str = "QEMU in a container isn't supported by this build";
const WHY_FIRECRACKER_OS: &str = "Firecracker microVMs run on Linux only";
/// A Windows microVM runs in QEMU's user-mode network, which has no rules.
const WHY_MICROVM_WINDOWS: &str = "microVMs on this PC can't protect your network yet (QEMU's      user-mode network has no rules). Choose No protection to start one with a network, or no      network.";
const WHY_VZ_OS: &str = "Apple microVMs run on a Mac only";
const WHY_MAC_OS: &str = "this runtime runs on a Mac only";
const WHY_PROVIDER: &str = "Spaces in your cloud account can't protect your network yet. \
     Create it on this computer or one of yours, or choose No protection.";

/// What `kind` does with network settings on host OS `os` (`linux`,
/// `macos`, `windows`) at `placement`. Every combination has an answer:
/// the matches have no wildcard, so a new runtime or placement does not
/// build until it says.
pub fn network_support(kind: BackendKind, os: &str, placement: Placement) -> NetworkSupport {
    use NetworkSupport::{Enforced, NoNetworkOnly, Refuse};
    match placement {
        // A pool host answers for its own runtime, on its own OS (and checks
        // that it enforces before it reports the Space).
        Placement::Local | Placement::PoolHost => {}
        Placement::Provider | Placement::Overflow => {
            return match kind {
                BackendKind::Lume
                | BackendKind::Qemu
                | BackendKind::QemuDocker
                | BackendKind::Container
                | BackendKind::Managed
                | BackendKind::Firecracker
                | BackendKind::Containerization
                | BackendKind::Vz => Refuse { why: WHY_PROVIDER },
            };
        }
    }
    let linux = os == "linux";
    let mac = os == "macos";
    match kind {
        BackendKind::Container | BackendKind::Managed => Enforced {
            point: ENGINE_POINT,
        },
        BackendKind::Firecracker if linux => Enforced { point: NS_POINT },
        BackendKind::Firecracker if os == "windows" => NoNetworkOnly {
            why: WHY_MICROVM_WINDOWS,
        },
        BackendKind::Firecracker => Refuse {
            why: WHY_FIRECRACKER_OS,
        },
        BackendKind::Qemu if linux => Enforced { point: NS_POINT },
        BackendKind::Qemu if mac || os == "windows" => Enforced {
            point: crate::netgw::POINT,
        },
        BackendKind::Qemu => NoNetworkOnly { why: WHY_QEMU },
        BackendKind::Vz if mac => Enforced {
            point: crate::netgw::POINT,
        },
        BackendKind::Vz => Refuse { why: WHY_VZ_OS },
        // Behind its own gateway; Cua reaches it through the gateway's
        // forwards.
        BackendKind::Lume if mac => Enforced {
            point: crate::netgw::POINT,
        },
        BackendKind::Lume => Refuse { why: WHY_MAC_OS },
        BackendKind::Containerization => Refuse {
            why: WHY_CONTAINERIZATION,
        },
        BackendKind::QemuDocker => Refuse {
            why: WHY_QEMU_DOCKER,
        },
    }
}

/// The note a Space gets when it started without network because its
/// runtime can't protect it yet.
pub const NO_NETWORK_NOTE: &str = "This runtime can't protect your network yet, so this Space \
     started without network. Choose No protection to open it.";

/// The note a Space gets when the user chose `unprotected`.
pub const UNPROTECTED_NOTE: &str =
    "You chose No protection: this Space can reach this computer and your local network.";

/// What a start does with a Space's network ([`plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkPlan {
    /// Start with these rules enforced by the host.
    Enforce(Rules),
    /// Start with no network at all; `note` says why when it was not
    /// asked for.
    NoNetwork {
        /// For people.
        note: Option<&'static str>,
    },
    /// Start open: only the rules the runtime can apply (none where it
    /// enforces nothing). Only for `unprotected`, or the revert switch.
    Open {
        /// For people.
        note: Option<&'static str>,
    },
    /// Don't start: why, in plain words.
    Refuse(&'static str),
}

/// What a start does with `settings` on a runtime that answers `support`
/// (see the module docs). Nothing but `unprotected` (or the revert switch)
/// is ever less strict than asked.
pub fn plan(
    settings: &NetworkSettings,
    support: NetworkSupport,
    ctx: &NetworkContext,
    mode: Mode,
) -> Result<NetworkPlan, PolicyError> {
    let rules = compile_mode(settings, ctx, mode)?;
    let none = settings.preset == NetworkPreset::None;
    Ok(match (mode, support) {
        (_, NetworkSupport::Enforced { .. }) => NetworkPlan::Enforce(rules),
        // The old default: an open Space where nothing was asked.
        (Mode::Legacy, _) if rules.is_empty() => NetworkPlan::Open { note: None },
        (Mode::Legacy, NetworkSupport::NoNetworkOnly { .. }) if none => {
            NetworkPlan::NoNetwork { note: None }
        }
        (Mode::Legacy, NetworkSupport::NoNetworkOnly { why } | NetworkSupport::Refuse { why }) => {
            NetworkPlan::Refuse(why)
        }
        (Mode::Secure, _) if settings.preset == NetworkPreset::Unprotected => NetworkPlan::Open {
            note: Some(UNPROTECTED_NOTE),
        },
        (Mode::Secure, NetworkSupport::NoNetworkOnly { .. }) if none => {
            NetworkPlan::NoNetwork { note: None }
        }
        (Mode::Secure, NetworkSupport::NoNetworkOnly { .. }) => NetworkPlan::NoNetwork {
            note: Some(NO_NETWORK_NOTE),
        },
        (Mode::Secure, NetworkSupport::Refuse { why }) => NetworkPlan::Refuse(why),
    })
}

/// What a start of `kind` on this computer does with `spec`'s network, for
/// a runtime that has no point to enforce rules (Lume, Vz, QEMU on macOS
/// or Windows, Apple Containerization): [`NetworkPlan::NoNetwork`] (start
/// it with no network), [`NetworkPlan::Open`] (`unprotected`, or the old
/// default) or an error in plain words. Never more open than asked.
pub fn local_plan(kind: BackendKind, spec: &crate::StartSpec) -> crate::Result<NetworkPlan> {
    local_plan_for(kind, &spec.name, &spec_settings(spec))
}

/// The settings a start spec asks for: its own, else what its old options
/// say.
pub fn spec_settings(spec: &crate::StartSpec) -> NetworkSettings {
    spec.network
        .clone()
        .unwrap_or_else(|| NetworkSettings::migrate(&spec.egress, spec.restrict_network))
}

/// Under the secure default, for a runtime that does not enforce rules on
/// this host: [`local_plan_for`] (`Some`). `None` for a runtime that
/// enforces them, and under the old default (the caller keeps its old
/// checks).
pub fn gate(
    kind: BackendKind,
    name: &str,
    settings: &NetworkSettings,
) -> crate::Result<Option<NetworkPlan>> {
    if Mode::from_env() == Mode::Legacy {
        return Ok(None);
    }
    match network_support(kind, std::env::consts::OS, Placement::Local) {
        NetworkSupport::Enforced { .. } => Ok(None),
        NetworkSupport::NoNetworkOnly { .. } | NetworkSupport::Refuse { .. } => {
            local_plan_for(kind, name, settings).map(Some)
        }
    }
}

/// [`local_plan`] for `settings` of sandbox `name`.
pub fn local_plan_for(
    kind: BackendKind,
    name: &str,
    settings: &NetworkSettings,
) -> crate::Result<NetworkPlan> {
    let support = network_support(kind, std::env::consts::OS, Placement::Local);
    let p = plan(
        settings,
        support,
        &NetworkContext::default(),
        Mode::from_env(),
    )?;
    match p {
        NetworkPlan::Refuse(why) => Err(crate::VmmError::Other(why.to_string())),
        NetworkPlan::Enforce(_) => Err(crate::VmmError::other(format!(
            "{kind} enforces network rules itself; it does not start through local_plan"
        ))),
        other => {
            if let NetworkPlan::NoNetwork { note: Some(n) } = &other {
                tracing::info!(sandbox = %name, runtime = %kind, "{n}");
            }
            Ok(other)
        }
    }
}

/// When a runtime applies new network settings ([`crate::Runtime::set_network`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applied {
    /// At once: the running guest is under them now.
    Now,
    /// At its next start (it is stopped, or its runtime can't change a
    /// running guest's network).
    NextStart,
}

/// The support table as a Markdown table (for the docs; a test keeps them
/// in step).
pub fn support_table_markdown() -> String {
    let mut out =
        String::from("| Runtime | Host | Linux | macOS | Windows |\n|---|---|---|---|---|\n");
    for kind in crate::egress::BACKENDS {
        for placement in [Placement::Local, Placement::Provider] {
            let cell = |os: &str| match network_support(kind, os, placement) {
                NetworkSupport::Enforced { .. } => "Enforced".to_string(),
                NetworkSupport::NoNetworkOnly { .. } => "No network only".to_string(),
                NetworkSupport::Refuse { .. } => "Refused".to_string(),
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                kind.as_str(),
                match placement {
                    Placement::Local | Placement::PoolHost => "this computer or one of yours",
                    Placement::Provider | Placement::Overflow => "your cloud account",
                },
                cell("linux"),
                cell("macos"),
                cell("windows"),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn a_runtime_without_rules_starts_without_network_or_refuses() {
        if Mode::from_env() == Mode::Legacy {
            return;
        }
        let spec = crate::StartSpec::new("x", crate::ImageSource::Existing);
        let os = std::env::consts::OS;
        for kind in crate::egress::BACKENDS {
            let support = network_support(kind, os, Placement::Local);
            match (support, local_plan(kind, &spec)) {
                (NetworkSupport::Enforced { .. }, r) => assert!(r.is_err(), "{kind}"),
                (NetworkSupport::NoNetworkOnly { .. }, r) => assert_eq!(
                    r.unwrap(),
                    NetworkPlan::NoNetwork {
                        note: Some(NO_NETWORK_NOTE)
                    }
                ),
                (NetworkSupport::Refuse { why }, r) => {
                    assert_eq!(r.unwrap_err().to_string(), why, "{kind}")
                }
            }
        }
        // No protection opens it, and says so.
        let mut open = spec.clone();
        open.network = Some(NetworkSettings::preset(NetworkPreset::Unprotected));
        assert_eq!(
            local_plan(BackendKind::Containerization, &open).unwrap(),
            NetworkPlan::Open {
                note: Some(UNPROTECTED_NOTE)
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn host_addresses_leave_out_loopback() {
        let a = super::host_addresses();
        assert!(a.iter().all(|i| !i.is_loopback()), "{a:?}");
    }
}
