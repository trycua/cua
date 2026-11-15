// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Where a public TCP share is served (CUA-2602): the route, chosen per
//! share, so public sharing costs Cua almost nothing when a cloud is linked.
//!
//! | Route | When | Where players connect |
//! |---|---|---|
//! | [`Route::Host`] | the Space runs in your linked cloud | the Space's own machine: `ip:port`, one firewall rule opened for the share |
//! | [`Route::Edge`] | the Space runs on your computer and a cloud is linked | one small server ("share edge") in your cloud, reused for every share; your computer connects out to it |
//! | [`Route::Relay`] | no cloud is linked (or the others cannot) | Cua's relay, with strict caps; can be turned off |
//!
//! [`choose`] picks one from the Space's location, the linked clouds, the
//! settings ([`ShareSettings`]) and what the user asked for. Everything here
//! is pure: the daemon calls it, the firewall and edge managers act on it.

use serde::{Deserialize, Serialize};

/// Where a share is served.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// The Space's own cloud host (a firewall rule opens the port).
    Host,
    /// The share edge in the user's cloud.
    Edge,
    /// Cua's relay.
    Relay,
}

impl Route {
    /// `host`, `edge`, `relay`.
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Host => "host",
            Route::Edge => "edge",
            Route::Relay => "relay",
        }
    }

    /// The one plain line people see next to the address.
    pub fn line(self, cloud_title: Option<&str>) -> String {
        match (self, cloud_title) {
            (Route::Host | Route::Edge, Some(t)) => format!("Through your {t} account"),
            (Route::Host | Route::Edge, None) => "Through your cloud account".into(),
            (Route::Relay, _) => "Through Cua".into(),
        }
    }

    /// Parses a route word.
    pub fn parse(s: &str) -> Option<Route> {
        match s.trim().to_ascii_lowercase().as_str() {
            "host" => Some(Route::Host),
            "edge" => Some(Route::Edge),
            "relay" => Some(Route::Relay),
            _ => None,
        }
    }
}

/// What a share asked for (`--route`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteRequest {
    /// Pick automatically.
    #[default]
    Auto,
    /// This route or fail.
    Exactly(Route),
}

impl RouteRequest {
    /// Parses `auto`, `host`, `edge`, `relay` (empty: auto).
    pub fn parse(s: &str) -> Option<RouteRequest> {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("auto") {
            return Some(RouteRequest::Auto);
        }
        Route::parse(s).map(RouteRequest::Exactly)
    }
}

/// Where shares may land (Settings, Sharing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Landing {
    /// Your cloud when one is linked, else Cua's relay.
    #[default]
    Auto,
    /// Only your cloud: no cloud linked means no public TCP share.
    OnlyMyCloud,
    /// Cua's relay even when a cloud is linked.
    AllowRelay,
}

/// The sharing settings the daemon keeps (`<home>/share/settings.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareSettings {
    /// Where shares may land.
    pub landing: Landing,
    /// Whether Cua's relay may carry public TCP shares at all.
    pub relay_fallback: bool,
    /// The most data public shares may move in a day, in MiB (0: no cap).
    pub daily_cap_mib: u64,
    /// The allowlist new shares start with (CIDRs; empty: anyone).
    pub default_allow: Vec<String>,
    /// Ask the owner before a share an agent made goes public.
    pub approve_agent_shares: bool,
    /// Spaces whose agents may share without asking (qualified ids).
    pub always_allow_spaces: Vec<String>,
    /// Clouds the owner agreed may hold a share edge (`aws`, `gcp`, ...).
    pub edge_consent: Vec<String>,
}

impl Default for ShareSettings {
    fn default() -> Self {
        Self {
            landing: Landing::Auto,
            relay_fallback: true,
            daily_cap_mib: DEFAULT_DAILY_CAP_MIB,
            default_allow: Vec::new(),
            approve_agent_shares: true,
            always_allow_spaces: Vec::new(),
            edge_consent: Vec::new(),
        }
    }
}

/// The daily data cap new installs start with: 2 GiB, a few hours of a
/// small game server.
pub const DEFAULT_DAILY_CAP_MIB: u64 = 2048;

impl ShareSettings {
    /// Reads `<home>/share/settings.json` (defaults when missing or bad).
    pub fn load(home: &std::path::Path) -> Self {
        std::fs::read(home.join("share").join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes it (atomically).
    pub fn save(&self, home: &std::path::Path) -> std::io::Result<()> {
        let dir = home.join("share");
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, dir.join("settings.json"))
    }

    /// Whether an agent's share in `space` needs the owner's approval.
    pub fn needs_approval(&self, space: &str) -> bool {
        self.approve_agent_shares && !self.always_allow_spaces.iter().any(|s| s == space)
    }

    /// Checks and normalizes the allowlist entries (`10.0.0.0/8`,
    /// `203.0.113.7`).
    pub fn check_allow(list: &[String]) -> Result<Vec<String>, String> {
        list.iter()
            .map(|c| normalize_cidr(c).ok_or_else(|| format!("{c:?} is not an address or CIDR")))
            .collect()
    }
}

/// `a.b.c.d/n` or `a.b.c.d` (as `/32`), IPv6 too; `None` when invalid.
pub fn normalize_cidr(s: &str) -> Option<String> {
    let s = s.trim();
    let (ip, len) = match s.split_once('/') {
        Some((ip, len)) => (ip, Some(len)),
        None => (s, None),
    };
    let ip: std::net::IpAddr = ip.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len: u8 = match len {
        Some(l) => l.parse().ok().filter(|l| *l <= max)?,
        None => max,
    };
    Some(format!("{ip}/{len}"))
}

/// Whether `ip` falls in one of `cidrs` (empty: everyone).
pub fn allowed(cidrs: &[String], ip: std::net::IpAddr) -> bool {
    if cidrs.is_empty() {
        return true;
    }
    cidrs.iter().any(|c| in_cidr(c, ip))
}

fn in_cidr(cidr: &str, ip: std::net::IpAddr) -> bool {
    let Some((net, len)) = cidr.split_once('/') else {
        return false;
    };
    let (Ok(net), Ok(len)) = (net.parse::<std::net::IpAddr>(), len.parse::<u32>()) else {
        return false;
    };
    match (net, ip) {
        (std::net::IpAddr::V4(n), std::net::IpAddr::V4(i)) => {
            let mask = if len == 0 {
                0
            } else {
                u32::MAX << (32 - len.min(32))
            };
            u32::from(n) & mask == u32::from(i) & mask
        }
        (std::net::IpAddr::V6(n), std::net::IpAddr::V6(i)) => {
            let mask = if len == 0 {
                0
            } else {
                u128::MAX << (128 - len.min(128))
            };
            u128::from(n) & mask == u128::from(i) & mask
        }
        _ => false,
    }
}

/// Where the shared Space runs, as far as routing cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceLocation {
    /// In a linked cloud (`aws`, `gcp`, ...), on its own machine.
    Cloud {
        /// The cloud word.
        provider: String,
        /// Whether that cloud can open one port of the computer to the
        /// internet (a firewall rule) and the computer publishes share slots.
        can_publish: bool,
    },
    /// On this computer (or another of the user's computers).
    Local,
}

/// A linked cloud that could hold a share edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedCloud {
    /// The cloud word (`aws`).
    pub provider: String,
    /// How people name it (`AWS`).
    pub title: String,
    /// Whether this build can deploy an edge there.
    pub can_edge: bool,
}

/// What [`choose`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutePlan {
    /// The route.
    pub route: Route,
    /// The cloud it uses (`None` for the relay).
    pub cloud: Option<String>,
    /// How people name that cloud.
    pub cloud_title: Option<String>,
    /// Whether this needs the owner's one-time consent first (a share edge
    /// they have not agreed to yet).
    pub needs_consent: bool,
}

impl RoutePlan {
    /// The plain line next to the address.
    pub fn line(&self) -> String {
        self.route.line(self.cloud_title.as_deref())
    }
}

/// Why no route works, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoRoute(pub String);

impl std::fmt::Display for NoRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Picks the route of a share: the Space's own cloud host when it can
/// publish, else a share edge in a linked cloud, else Cua's relay, within
/// the settings and what was asked for.
pub fn choose(
    location: &SpaceLocation,
    clouds: &[LinkedCloud],
    settings: &ShareSettings,
    asked: RouteRequest,
) -> Result<RoutePlan, NoRoute> {
    let relay_ok = settings.relay_fallback;
    let host = match location {
        SpaceLocation::Cloud {
            provider,
            can_publish: true,
        } => Some(provider.clone()),
        _ => None,
    };
    let edge = clouds.iter().find(|c| c.can_edge);
    let title = |p: &str| {
        clouds
            .iter()
            .find(|c| c.provider == p)
            .map(|c| c.title.clone())
    };
    let host_plan = |p: String| RoutePlan {
        route: Route::Host,
        cloud_title: title(&p),
        cloud: Some(p),
        needs_consent: false,
    };
    let edge_plan = |c: &LinkedCloud| RoutePlan {
        route: Route::Edge,
        cloud: Some(c.provider.clone()),
        cloud_title: Some(c.title.clone()),
        needs_consent: !settings.edge_consent.iter().any(|p| p == &c.provider),
    };
    let relay_plan = || RoutePlan {
        route: Route::Relay,
        cloud: None,
        cloud_title: None,
        needs_consent: false,
    };
    let no_relay = || {
        if !relay_ok {
            "public TCP shares through Cua are turned off (Settings, Sharing); link a cloud to share from it"
        } else {
            "shares are set to use only your cloud (Settings, Sharing); link a cloud, or allow Cua's relay"
        }
    };
    match asked {
        RouteRequest::Exactly(Route::Host) => host.map(host_plan).ok_or_else(|| {
            NoRoute("the host route needs a Space that runs in your linked cloud".into())
        }),
        RouteRequest::Exactly(Route::Edge) => edge.map(edge_plan).ok_or_else(|| {
            NoRoute("the edge route needs a linked cloud that can hold a share edge (AWS)".into())
        }),
        RouteRequest::Exactly(Route::Relay) => {
            if !relay_ok || settings.landing == Landing::OnlyMyCloud {
                Err(NoRoute(no_relay().into()))
            } else {
                Ok(relay_plan())
            }
        }
        RouteRequest::Auto => {
            if settings.landing == Landing::AllowRelay
                && relay_ok
                && host.is_none()
                && edge.is_none()
            {
                return Ok(relay_plan());
            }
            if let Some(p) = host {
                return Ok(host_plan(p));
            }
            if let Some(c) = edge {
                return Ok(edge_plan(c));
            }
            if settings.landing == Landing::OnlyMyCloud || !relay_ok {
                return Err(NoRoute(no_relay().into()));
            }
            Ok(relay_plan())
        }
    }
}

/// Monthly cost of a share edge and the data price, for the consent card.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeCost {
    /// Dollars a month for the server and its address.
    pub monthly_usd: f64,
    /// Dollars per GiB sent to players.
    pub per_gib_usd: f64,
}

/// The cost of a share edge on `provider` (list prices; the cloud bills).
pub fn edge_cost(provider: &str) -> EdgeCost {
    match provider {
        // t4g.nano on demand (~$3.07) + a public IPv4 address ($3.65) + 8 GiB gp3 (~$0.64).
        "aws" => EdgeCost {
            monthly_usd: 7.4,
            per_gib_usd: 0.09,
        },
        // e2-micro (~$6.11) + an external IPv4 ($2.92) + 10 GiB pd-standard ($0.40).
        "gcp" => EdgeCost {
            monthly_usd: 9.4,
            per_gib_usd: 0.12,
        },
        // CAX11 (~€3.79) incl. IPv4 and 20 TB of traffic.
        "hetzner" => EdgeCost {
            monthly_usd: 4.5,
            per_gib_usd: 0.0,
        },
        _ => EdgeCost {
            monthly_usd: 10.0,
            per_gib_usd: 0.12,
        },
    }
}

/// The one-time consent text before a share edge is made.
pub fn consent_text(provider_title: &str, cost: &EdgeCost, daily_cap_mib: u64) -> String {
    let data = if cost.per_gib_usd > 0.0 {
        format!(" plus ${:.2} per GB players download", cost.per_gib_usd)
    } else {
        String::new()
    };
    let cap = if daily_cap_mib > 0 {
        format!(
            " Shares stop for the day after {} (change it in Settings).",
            human_mib(daily_cap_mib)
        )
    } else {
        String::new()
    };
    format!(
        "This creates a small server in your {provider_title} account (about ${:.0} a month{data}). Remove it any time.{cap}",
        cost.monthly_usd
    )
}

/// `2 GB`, `512 MB`.
pub fn human_mib(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        format!("{} GB", mib / 1024)
    } else {
        format!("{mib} MB")
    }
}

/// A day's data use against the cap.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyUse {
    /// The day (`YYYY-MM-DD`, UTC).
    pub day: String,
    /// Bytes moved so far today.
    pub bytes: u64,
}

impl DailyUse {
    /// Adds `bytes` on `day` (a new day starts at zero); whether the cap is
    /// now reached.
    pub fn add(&mut self, day: &str, bytes: u64, cap_mib: u64) -> bool {
        if self.day != day {
            self.day = day.to_owned();
            self.bytes = 0;
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.over(day, cap_mib)
    }

    /// Whether the cap is reached on `day`.
    pub fn over(&self, day: &str, cap_mib: u64) -> bool {
        cap_mib > 0 && self.day == day && self.bytes >= cap_mib.saturating_mul(1 << 20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aws() -> LinkedCloud {
        LinkedCloud {
            provider: "aws".into(),
            title: "AWS".into(),
            can_edge: true,
        }
    }

    fn in_aws() -> SpaceLocation {
        SpaceLocation::Cloud {
            provider: "aws".into(),
            can_publish: true,
        }
    }

    #[test]
    fn auto_prefers_the_spaces_own_cloud_then_an_edge_then_the_relay() {
        let s = ShareSettings::default();
        let p = choose(&in_aws(), &[aws()], &s, RouteRequest::Auto).unwrap();
        assert_eq!((p.route, p.cloud.as_deref()), (Route::Host, Some("aws")));
        assert_eq!(p.line(), "Through your AWS account");
        assert!(!p.needs_consent);

        let p = choose(&SpaceLocation::Local, &[aws()], &s, RouteRequest::Auto).unwrap();
        assert_eq!(p.route, Route::Edge);
        assert!(p.needs_consent, "the first edge asks first");

        let p = choose(&SpaceLocation::Local, &[], &s, RouteRequest::Auto).unwrap();
        assert_eq!(p.route, Route::Relay);
        assert_eq!(p.line(), "Through Cua");

        // A cloud Space whose machine cannot publish goes through the edge.
        let cannot = SpaceLocation::Cloud {
            provider: "aws".into(),
            can_publish: false,
        };
        assert_eq!(
            choose(&cannot, &[aws()], &s, RouteRequest::Auto)
                .unwrap()
                .route,
            Route::Edge
        );
    }

    #[test]
    fn consent_once_per_cloud() {
        let mut s = ShareSettings::default();
        s.edge_consent.push("aws".into());
        let p = choose(&SpaceLocation::Local, &[aws()], &s, RouteRequest::Auto).unwrap();
        assert!(!p.needs_consent);
    }

    #[test]
    fn settings_can_keep_shares_off_the_relay() {
        let only = ShareSettings {
            landing: Landing::OnlyMyCloud,
            ..ShareSettings::default()
        };
        assert!(choose(&SpaceLocation::Local, &[], &only, RouteRequest::Auto).is_err());
        assert!(
            choose(
                &SpaceLocation::Local,
                &[],
                &only,
                RouteRequest::Exactly(Route::Relay)
            )
            .is_err()
        );
        let off = ShareSettings {
            relay_fallback: false,
            ..ShareSettings::default()
        };
        let e = choose(&SpaceLocation::Local, &[], &off, RouteRequest::Auto).unwrap_err();
        assert!(e.0.contains("turned off"), "{e}");
    }

    #[test]
    fn an_exact_route_is_that_route_or_a_plain_refusal() {
        let s = ShareSettings::default();
        assert!(
            choose(
                &SpaceLocation::Local,
                &[aws()],
                &s,
                RouteRequest::Exactly(Route::Host)
            )
            .is_err()
        );
        assert!(choose(&in_aws(), &[], &s, RouteRequest::Exactly(Route::Edge)).is_err());
        assert_eq!(
            choose(&in_aws(), &[aws()], &s, RouteRequest::Exactly(Route::Relay))
                .unwrap()
                .route,
            Route::Relay
        );
        assert_eq!(RouteRequest::parse("").unwrap(), RouteRequest::Auto);
        assert_eq!(
            RouteRequest::parse("EDGE").unwrap(),
            RouteRequest::Exactly(Route::Edge)
        );
        assert!(RouteRequest::parse("cdn").is_none());
    }

    #[test]
    fn allowlists_match_addresses_and_ranges() {
        let list =
            ShareSettings::check_allow(&["203.0.113.0/24".into(), "198.51.100.7".into()]).unwrap();
        assert_eq!(list, vec!["203.0.113.0/24", "198.51.100.7/32"]);
        assert!(allowed(&list, "203.0.113.200".parse().unwrap()));
        assert!(allowed(&list, "198.51.100.7".parse().unwrap()));
        assert!(!allowed(&list, "198.51.100.8".parse().unwrap()));
        assert!(!allowed(&list, "2001:db8::1".parse().unwrap()));
        assert!(allowed(&[], "192.0.2.1".parse().unwrap()));
        assert!(ShareSettings::check_allow(&["10.0.0.0/33".into()]).is_err());
        assert!(ShareSettings::check_allow(&["example.com".into()]).is_err());
        let v6 = ShareSettings::check_allow(&["2001:db8::/32".into()]).unwrap();
        assert!(allowed(&v6, "2001:db8:1::5".parse().unwrap()));
    }

    #[test]
    fn approval_is_asked_unless_the_space_is_always_allowed() {
        let mut s = ShareSettings::default();
        assert!(s.needs_approval("local:mc"));
        s.always_allow_spaces.push("local:mc".into());
        assert!(!s.needs_approval("local:mc"));
        assert!(s.needs_approval("local:other"));
        s.approve_agent_shares = false;
        assert!(!s.needs_approval("local:other"));
    }

    #[test]
    fn the_daily_cap_resets_each_day() {
        let mut u = DailyUse::default();
        assert!(!u.add("2026-10-27", 1 << 20, 2));
        assert!(u.add("2026-10-27", 1 << 20, 2));
        assert!(u.over("2026-10-27", 2));
        assert!(!u.add("2026-10-28", 10, 2), "a new day starts at zero");
        assert!(!u.add("2026-10-28", u64::MAX, 0), "0 is no cap");
    }

    #[test]
    fn the_consent_card_names_the_cloud_the_cost_and_the_cap() {
        let t = consent_text("AWS", &edge_cost("aws"), DEFAULT_DAILY_CAP_MIB);
        assert!(t.starts_with("This creates a small server in your AWS account (about $7 a month"));
        assert!(t.contains("per GB"));
        assert!(t.contains("Remove it any time."));
        assert!(t.contains("after 2 GB"));
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(ShareSettings::load(home.path()), ShareSettings::default());
        let s = ShareSettings {
            landing: Landing::OnlyMyCloud,
            daily_cap_mib: 512,
            ..ShareSettings::default()
        };
        s.save(home.path()).unwrap();
        assert_eq!(ShareSettings::load(home.path()), s);
    }
}
