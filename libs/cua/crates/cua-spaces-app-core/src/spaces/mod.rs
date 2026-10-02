// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces: the SDK registry's rows as Spaces, their names, the MRU order,
//! the Space list state machine ([`roster`]) and the main window's sidebar
//! and detail ([`sidebar`]) with its Stream section ([`stream`]) and the
//! cover over its live desktop ([`cover`]).
//!
//! Every location (Cua Cloud, this Mac, added by address, relay) arrives
//! through one `list_spaces` roster; the SDK owns lifecycle and the spacesd
//! handshake, this module only turns its rows into what the shells draw.

pub mod cover;
pub mod creating;
pub mod roster;
pub mod sidebar;
pub mod stream;

use crate::model::{
    PowerControl, Space, SpaceOs, SpacePower, SpaceProvider, SpaceRow, SpaceSdkRef, SpaceStatus,
    ThumbnailScene, normalize_arch,
};
use crate::util::{collate, parse_rfc3339_ms};
use serde::{Deserialize, Serialize};

/// Thumbnail refresh cadence for visible running tiles.
pub const THUMBNAIL_INTERVAL_MS: u64 = 3_000;
/// Registry refresh cadence while a Spaces surface is open.
pub const REFRESH_MS: u64 = 10_000;
/// Long edge of tile thumbnails, in pixels.
pub const THUMBNAIL_MAX_DIMENSION: u32 = 480;

/// spacesd feature names the shells key on.
pub mod feature {
    /// The whole desktop streams.
    pub const DESKTOP_STREAM: &str = "desktop_stream";
    /// Single windows stream.
    pub const WINDOW_STREAM: &str = "window_stream";
    /// Desktop audio.
    pub const AUDIO_DESKTOP: &str = "audio.desktop";
    /// Network hotspot.
    pub const HOTSPOT: &str = "hotspot";
}

/// Whether the Space's spacesd reported `feature`.
pub fn has_feature(space: &Space, feature: &str) -> bool {
    space
        .sdk
        .as_ref()
        .is_some_and(|s| s.features.iter().any(|f| f == feature))
}

/// The location word a Space id names (`cloud:<name>`, `local:<name>`,
/// `direct:<host:port>`, `relay:<id>`, or the legacy `space://<loc>/...`).
pub fn provider_of_id(id: &str) -> Option<SpaceProvider> {
    let word = if let Some(rest) = id.strip_prefix("space://") {
        let end = rest.find('/')?;
        &rest[..end]
    } else {
        let end = id.find(':')?;
        &id[..end]
    };
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    SpaceProvider::parse(word)
}

/// A row's location word as a provider (`direct` for unknown words).
pub fn normalize_provider(word: &str) -> SpaceProvider {
    SpaceProvider::parse(word).unwrap_or(SpaceProvider::Direct)
}

/// The cloud namespace of a legacy `space://cloud/<ns>/<name>` id.
pub fn cloud_namespace_of(id: &str) -> Option<String> {
    let rest = id.strip_prefix("space://cloud/")?;
    let end = rest.find('/')?;
    (end > 0).then(|| rest[..end].to_string())
}

/// The placeholder scene for an OS.
pub fn scene_for_os(os: SpaceOs) -> ThumbnailScene {
    match os {
        SpaceOs::Macos => ThumbnailScene::MacDesktop,
        SpaceOs::Windows => ThumbnailScene::WindowsDesktop,
        SpaceOs::Linux => ThumbnailScene::LinuxTerminal,
    }
}

/// "brave-otter" to "Brave Otter"; a `host:port` stays as it is.
pub fn display_name(name: &str) -> String {
    let words: Vec<&str> = name.split(['-', '_']).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return name.to_string();
    }
    if words.len() == 1 && (name.contains('.') || name.contains(':')) {
        return name.to_string();
    }
    words
        .iter()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn is_container_hostname(name: &str) -> bool {
    name.len() == 12
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The name to show. A container's spacesd reports its hostname (12 hex
/// digits), so the id's name (`local:<name>`, `cloud:<name>`) wins then.
pub fn name_of(id: &str, name: &str) -> String {
    let from_id = id
        .strip_prefix("local:")
        .or_else(|| id.strip_prefix("cloud:"))
        .filter(|rest| !rest.is_empty());
    if let Some(from_id) = from_id
        && (name.is_empty() || is_container_hostname(name))
    {
        return from_id.to_string();
    }
    if name.is_empty() {
        id.to_string()
    } else {
        name.to_string()
    }
}

fn provider_label(p: SpaceProvider) -> &'static str {
    match p {
        SpaceProvider::Cloud => "Cua Cloud",
        SpaceProvider::Local => "This Mac",
        SpaceProvider::Direct => "Direct",
        SpaceProvider::Relay => "My machines",
    }
}

fn detail_for(row: &SpaceRow, status: SpaceStatus) -> String {
    if status == SpaceStatus::Suspended {
        // Turned off on purpose: said so, not "Unreachable".
        if let Some(control) = row.power.as_deref().and_then(PowerControl::parse)
            && matches!(
                row.power_state.as_deref(),
                Some("suspended") | Some("stopped")
            )
        {
            return off_label(control).into();
        }
        return match &row.error {
            Some(e) if !e.is_empty() => format!("Unreachable \u{b7} {e}"),
            _ => "Unreachable".into(),
        };
    }
    match normalize_provider(&row.provider) {
        SpaceProvider::Cloud => cloud_namespace_of(&row.id)
            .unwrap_or_else(|| provider_label(SpaceProvider::Cloud).into()),
        SpaceProvider::Direct => row
            .id
            .strip_prefix("space://direct/")
            .or_else(|| row.id.strip_prefix("direct:"))
            .unwrap_or(&row.id)
            .to_string(),
        // A Space in your cloud: where it runs ("AWS · us-west-2").
        SpaceProvider::Relay if cloud_place_of(row).is_some() => {
            cloud_place_of(row).unwrap_or_default().to_string()
        }
        SpaceProvider::Relay => match row.host.as_deref().filter(|h| !h.is_empty()) {
            Some(host) => format!(
                "On {}",
                row.host_name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .unwrap_or(host)
            ),
            None => "My machines \u{b7} via relay".into(),
        },
        p => provider_label(p).into(),
    }
}

/// A Space in your cloud's account and region in words, when it is one.
fn cloud_place_of(row: &SpaceRow) -> Option<&str> {
    row.cloud
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(|c| {
            row.cloud_place
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .unwrap_or(c)
        })
}

/// Maps one registry row onto a Space. `now` stands in for a missing
/// `added_at`.
pub fn row_to_space(row: &SpaceRow, now: i64) -> Space {
    let os = row.os.unwrap_or(SpaceOs::Linux);
    let power = power_of_row(row);
    // Turned off as the SDK recorded it: not running, whatever a probe
    // that raced the turn-off said.
    let recorded_off = matches!(
        row.power_state.as_deref(),
        Some("suspended") | Some("stopped")
    );
    let status = if row.reachable && !(power.is_some() && recorded_off) {
        SpaceStatus::Running
    } else {
        SpaceStatus::Suspended
    };
    let added = row.added_at.as_deref().and_then(parse_rfc3339_ms);
    let provider = normalize_provider(&row.provider);
    let namespace = cloud_namespace_of(&row.id);
    let text = |v: &Option<String>| v.clone().filter(|v| !v.trim().is_empty());
    let (os_name, os_pretty_name, image) = (
        text(&row.os_name),
        text(&row.os_pretty_name),
        text(&row.image),
    );
    // Until the Space reports its OS, the catalog's distribution for its
    // image names it ("Omarchy", not "Linux").
    let distro = match (&os_name, &os_pretty_name) {
        (None, None) => image.as_deref().and_then(crate::wizard::image_distro),
        _ => None,
    };
    Space {
        id: row.id.clone(),
        name: display_name(&name_of(&row.id, &row.name)),
        os,
        status,
        detail: detail_for(row, status),
        last_used_at: added.unwrap_or(now),
        started_at: added,
        scene: scene_for_os(os),
        fleet_id: Some(namespace.unwrap_or_else(|| provider.as_str().to_string())),
        size: None,
        region: None,
        provider: Some(provider),
        sdk: Some(SpaceSdkRef {
            features: row.features.clone(),
            spacesd_version: row.spacesd_version.clone(),
            reachable: row.reachable,
            error: row.error.clone(),
        }),
        os_name: os_name.or_else(|| distro.as_ref().map(|d| d.name.clone())),
        progress: None,
        os_pretty_name: os_pretty_name.or_else(|| distro.map(|d| d.name)),
        kind: row
            .kind
            .or_else(|| image.as_deref().and_then(crate::wizard::image_kind)),
        arch: row
            .arch
            .as_deref()
            .and_then(normalize_arch)
            .map(str::to_string)
            .or_else(|| image.as_deref().and_then(single_arch)),
        image,
        image_digest: row.image_digest.clone().filter(|v| !v.trim().is_empty()),
        host: text(&row.host),
        host_name: text(&row.host_name),
        power,
        cloud: text(&row.cloud),
        cloud_place: cloud_place_of(row).map(str::to_string),
        cloud_delete: text(&row.cloud).and(text(&row.cloud_delete)),
    }
}

/// A row's power: its control, and off when the SDK recorded it suspended
/// or stopped, or when it does not answer and no state was recorded (a
/// Space one of your machines provides is unreachable while it is off).
fn power_of_row(row: &SpaceRow) -> Option<SpacePower> {
    let control = row.power.as_deref().and_then(PowerControl::parse)?;
    let state = row.power_state.as_deref().filter(|s| !s.trim().is_empty());
    Some(SpacePower {
        control,
        off: matches!(state, Some("suspended") | Some("stopped"))
            || (state.is_none() && !row.reachable),
        turning_on: None,
        error: None,
    })
}

/// "Suspended" or "Off": what an off Space is called, by how it turned
/// off.
pub fn off_label(control: PowerControl) -> &'static str {
    match control {
        PowerControl::Suspend => "Suspended",
        PowerControl::Stop => "Off",
    }
}

/// The one platform the catalog lists for `image`, if it lists one.
fn single_arch(image: &str) -> Option<String> {
    match crate::wizard::image_arch(image).as_slice() {
        [one] => normalize_arch(one).map(str::to_string),
        _ => None,
    }
}

/// Maps a whole roster.
pub fn rows_to_spaces(rows: &[SpaceRow], now: i64) -> Vec<Space> {
    rows.iter().map(|r| row_to_space(r, now)).collect()
}

/// A Space on the Cua Spaces pool (the account's `cua-spaces*` namespace or
/// the shared `spaces-linux` pool): listed first.
pub fn is_spaces_pool(space: &Space) -> bool {
    cloud_namespace_of(&space.id)
        .is_some_and(|ns| ns.starts_with("cua-spaces") || ns == "spaces-linux")
}

/// Pool Spaces first, then most recently used, then by name.
pub fn sort_by_mru(spaces: &[Space]) -> Vec<Space> {
    let mut out = spaces.to_vec();
    out.sort_by(|a, b| {
        let (pa, pb) = (is_spaces_pool(a), is_spaces_pool(b));
        pb.cmp(&pa)
            .then_with(|| b.last_used_at.cmp(&a.last_used_at))
            .then_with(|| collate(&a.name, &b.name))
    });
    out
}

/// Marks `id` used at `now`. Unknown ids are a no-op.
pub fn touch_space(spaces: &[Space], id: &str, now: i64) -> Vec<Space> {
    spaces
        .iter()
        .map(|s| {
            if s.id == id {
                Space {
                    last_used_at: now,
                    ..s.clone()
                }
            } else {
                s.clone()
            }
        })
        .collect()
}

/// Spaces that count as active (running, waiting for approval, starting).
pub fn count_active(spaces: &[Space]) -> u32 {
    spaces
        .iter()
        .filter(|s| {
            matches!(
                s.status,
                SpaceStatus::Running | SpaceStatus::Approval | SpaceStatus::Provisioning
            )
        })
        .count() as u32
}

/// The Spaces the user can open: every Space but "This machine", which
/// counts only while it is shared as a Space and reachable (its status is
/// `Local`). A Space a provider hosts on another machine counts. The notch
/// tab and the menu bar item both show this number.
pub fn openable_count(spaces: &[Space]) -> u32 {
    spaces
        .iter()
        .filter(|s| {
            if s.id == crate::host::THIS_MACHINE_ID {
                s.status == SpaceStatus::Local
            } else {
                s.status != SpaceStatus::Local
            }
        })
        .count() as u32
}

/// The three ambient status dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmbientDots {
    /// Any Space running or starting.
    pub running: bool,
    /// Any Space waiting for approval.
    pub approval: bool,
    /// Any Space suspended.
    pub suspended: bool,
    /// Any agent actively working.
    pub agent_active: bool,
}

/// The ambient dots for a list.
pub fn ambient_dots(spaces: &[Space]) -> AmbientDots {
    let has = |st: SpaceStatus| spaces.iter().any(|s| s.status == st);
    AmbientDots {
        running: has(SpaceStatus::Running) || has(SpaceStatus::Provisioning),
        approval: has(SpaceStatus::Approval),
        suspended: has(SpaceStatus::Suspended),
        agent_active: has(SpaceStatus::Running),
    }
}

/// The menu bar item's status line ("No Spaces", "1 Space", "3 Spaces").
pub fn status_line(spaces: u32) -> String {
    match spaces {
        0 => "No Spaces".into(),
        1 => "1 Space".into(),
        n => format!("{n} Spaces"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> SpaceRow {
        SpaceRow {
            id: "space://cloud/cua-spaces-abc/brave-otter".into(),
            name: "brave-otter".into(),
            provider: "cloud".into(),
            spacesd_version: "0.4.0".into(),
            features: vec!["desktop_stream".into(), "window_stream".into()],
            added_at: Some("2026-08-30T12:00:00Z".into()),
            os: Some(SpaceOs::Linux),
            os_name: Some("Ubuntu".into()),
            reachable: true,
            error: None,
            os_pretty_name: None,
            image: None,
            image_digest: None,
            kind: None,
            arch: None,
            host: None,
            host_name: None,
            power: None,
            power_state: None,
            cloud: None,
            cloud_place: None,
            cloud_delete: None,
        }
    }

    #[test]
    fn maps_a_reachable_cloud_row() {
        let s = row_to_space(&row(), 1_000);
        assert_eq!(s.name, "Brave Otter");
        assert_eq!(s.status, SpaceStatus::Running);
        assert_eq!(s.fleet_id.as_deref(), Some("cua-spaces-abc"));
        assert_eq!(s.detail, "cua-spaces-abc");
        assert_eq!(s.last_used_at, 1_788_091_200_000);
        assert!(has_feature(&s, feature::DESKTOP_STREAM));
    }

    #[test]
    fn unreachable_is_suspended_with_reason() {
        let s = row_to_space(
            &SpaceRow {
                reachable: false,
                error: Some("timed out".into()),
                ..row()
            },
            5,
        );
        assert_eq!(s.status, SpaceStatus::Suspended);
        assert!(s.detail.contains("timed out"));
    }

    #[test]
    fn direct_and_local_rows() {
        let d = row_to_space(
            &SpaceRow {
                id: "space://direct/10.0.0.5:3211".into(),
                name: "10.0.0.5:3211".into(),
                provider: "direct".into(),
                os: None,
                added_at: None,
                ..row()
            },
            42,
        );
        assert_eq!(d.os, SpaceOs::Linux);
        assert_eq!(d.detail, "10.0.0.5:3211");
        assert_eq!(d.name, "10.0.0.5:3211");
        assert_eq!(d.last_used_at, 42);
        assert_eq!(d.fleet_id.as_deref(), Some("direct"));
        let hex = row_to_space(
            &SpaceRow {
                id: "local:demo-box".into(),
                name: "0123456789ab".into(),
                provider: "local".into(),
                ..row()
            },
            0,
        );
        assert_eq!(hex.name, "Demo Box");
        assert_eq!(hex.detail, "This Mac");
    }

    #[test]
    fn ids_name_their_provider() {
        assert_eq!(provider_of_id("cloud:x"), Some(SpaceProvider::Cloud));
        assert_eq!(
            provider_of_id("space://relay/abc"),
            Some(SpaceProvider::Relay)
        );
        assert_eq!(provider_of_id("weird:x"), None);
        assert_eq!(provider_of_id("nocolon"), None);
    }

    #[test]
    fn mru_puts_pool_first_then_recent() {
        let mk = |id: &str, t: i64| Space {
            id: id.into(),
            name: id.into(),
            os: SpaceOs::Linux,
            status: SpaceStatus::Running,
            detail: String::new(),
            last_used_at: t,
            started_at: None,
            scene: ThumbnailScene::LinuxTerminal,
            fleet_id: None,
            size: None,
            region: None,
            provider: None,
            sdk: None,
            os_name: None,
            progress: None,
            os_pretty_name: None,
            image: None,
            image_digest: None,
            kind: None,
            arch: None,
            host: None,
            host_name: None,
            power: None,
            cloud: None,
            cloud_place: None,
            cloud_delete: None,
        };
        let sorted = sort_by_mru(&[
            mk("a", 10),
            mk("space://cloud/cua-spaces-x/p", 1),
            mk("b", 20),
        ]);
        let ids: Vec<_> = sorted.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["space://cloud/cua-spaces-x/p", "b", "a"]);
    }
}
