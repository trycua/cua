// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua network`: what a Space can reach, and changing it.
//!
//! - `show`: its preset, what it opens beyond Internet only, where the
//!   rules are enforced (outside the guest) or why it has no network, and
//!   what is always blocked;
//! - `set`: a new preset and lists (a child stays within its parent's
//!   unless `--widen`; `--tree` gives its children the same);
//! - `log`: what it tried to reach and was blocked.
//!
//! Also `cua hotspot` (this computer as a Space's exit for a while, as the
//! MCP `hotspot_*` tools do) and `cua mesh status` (the path traffic takes
//! to each other computer, and a pin).

use std::io::Write;
use std::sync::Arc;

use clap::Subcommand;
use cua_sdk::{Cua, CuaError, SpaceNetworkInfo};

use crate::util::line;

/// `cua network`.
#[derive(Subcommand, Debug, Clone)]
pub enum NetworkCmd {
    /// What a Space can reach: its preset, what it opens, where it is
    /// enforced, and what is always blocked.
    #[command(after_help = "Examples:
  cua network show studio
  cua network show local:studio --json")]
    Show {
        /// Space id or name.
        space: String,
    },
    /// Change what a Space can reach. A child Space stays within its
    /// parent's unless `--widen`.
    #[command(after_help = "Examples:
  # This computer's Postgres and dev server only
  cua network set studio --network localhost:5432,3000
  # Your local network too
  cua network set studio --network lan
  # Only these destinations
  cua network set studio --network custom:none --allow 140.82.112.0/20
  # Back to the default
  cua network set studio --network internet
  # Its internet traffic leaves through this computer (the hotspot)
  cua network set studio --via this-computer
  # ... or through another Space of its tree, or a SOCKS5 proxy
  cua network set worker --via space:gateway
  cua network set studio --via socks5://proxy.example:1080
  # Its own network again
  cua network set studio --via internet")]
    Set {
        /// Space id or name.
        space: String,
        /// `internet`, `none`, `localhost:<ports>` (or `localhost:all`),
        /// `lan`, `localhost+lan:<ports>` (both), `custom`, `custom:none` or
        /// `unprotected` (No protection).
        /// Left out: the preset it has (to change only `--via`).
        #[arg(long, value_name = "PRESET")]
        network: Option<String>,
        /// Where its internet traffic leaves: `internet` (its own network,
        /// the default), `this-computer` (this computer's network: the
        /// hotspot), `space:<name>` (a Space of its tree) or
        /// `socks5://host:port`. Its rules apply first; if the exit goes
        /// away, its internet stops (it never falls back). A child keeps
        /// its parent's. Left out: the exit it has.
        #[arg(long, value_name = "EXIT")]
        via: Option<String>,
        /// Allow this destination (IP, CIDR, or a name such as `pypi.org` or
        /// `*.github.com` on runtimes behind the Cua network gateway;
        /// repeat for more).
        #[arg(long, value_name = "ADDR|CIDR")]
        allow: Vec<String>,
        /// Block this destination (IP, CIDR, `all`, or a name on runtimes
        /// behind the Cua network gateway; repeat for more). A deny wins
        /// over the Space's own tree: it blocks a member of the tree too.
        #[arg(long, value_name = "ADDR|CIDR|all")]
        deny: Vec<String>,
        /// A child Space: let it reach more than its parent (recorded on
        /// the Space).
        #[arg(long)]
        widen: bool,
        /// Its child Spaces get the same settings (each within its parent's).
        #[arg(long)]
        tree: bool,
    },
    /// What a Space tried to reach and was blocked, most recent first.
    #[command(after_help = "Examples:
  cua network log studio")]
    Log {
        /// Space id or name.
        space: String,
    },
}

/// `cua hotspot`: this computer as a Space's exit for a while. For a
/// lasting choice use `cua network set <space> --via this-computer`.
#[derive(Subcommand, Debug, Clone)]
pub enum HotspotCmd {
    /// Send a Space's internet traffic out through this computer's network
    /// until `cua hotspot stop` (its own rules still apply).
    #[command(after_help = "Examples:
  cua hotspot start studio
  cua hotspot start studio --system-proxy --bypass 10.0.0.0/8")]
    Start {
        /// Space id or name.
        space: String,
        /// Point the Space's system proxy (and new processes' ALL_PROXY)
        /// at it. Otherwise apps use the SOCKS address it prints.
        #[arg(long)]
        system_proxy: bool,
        /// Destinations that keep the Space's own network (repeat).
        #[arg(long, value_name = "ADDR|CIDR|NAME")]
        bypass: Vec<String>,
    },
    /// Stop it.
    #[command(after_help = "Examples:
  cua hotspot stop studio")]
    Stop {
        /// Space id or name.
        space: String,
    },
    /// Whether it is on, and what it carried.
    #[command(after_help = "Examples:
  cua hotspot status studio --json")]
    Status {
        /// Space id or name.
        space: String,
    },
}

/// `cua mesh`: how traffic reaches your other computers.
#[derive(Subcommand, Debug, Clone)]
pub enum MeshCmd {
    /// The path to each other computer (local network, direct or relay),
    /// its round trip and why it was chosen. `--prefer` pins it.
    #[command(after_help = "Examples:
  cua mesh status
  cua mesh status --prefer lan
  cua mesh status --prefer auto")]
    Status {
        /// Pin the path: `lan` (the local network whenever it answers),
        /// `direct`, `relay`, or `auto` (fastest; lifts the pin). Traffic
        /// never stops: a pinned path that goes away falls back.
        #[arg(long, value_name = "lan|direct|relay|auto")]
        prefer: Option<String>,
    },
}

/// The one-time line about the new defaults, before a Space's first `show`
/// after the update.
const NOTICE: &str = "New: Spaces now reach the internet only. This Space can't reach this \
     computer, your local network or other Spaces unless you allow it \
     (cua network set <space> --network ...).";

/// Runs `cmd` against `cua`'s Spaces.
pub async fn run(
    cua: &Arc<Cua>,
    cmd: NetworkCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let spaces = cua.spaces();
    match cmd {
        NetworkCmd::Show { space } => {
            let info = spaces.network(space.clone()).await?;
            if json {
                line(out, info_json(&info).to_string());
            } else {
                if info.notice_pending {
                    line(out, NOTICE);
                }
                for l in show_lines(&space, &info) {
                    line(out, l);
                }
            }
            if info.notice_pending {
                // Shown once (in JSON too: `notice_pending` said it).
                spaces.ack_network_notice(space.clone()).await?;
            }
        }
        NetworkCmd::Set {
            space,
            network,
            via,
            allow,
            deny,
            widen,
            tree,
        } => {
            let current = spaces.network(space.clone()).await?.settings;
            let mut settings: cua_sdk::NetworkSettings = match &network {
                Some(network) => cua_spaces::network::parse_network(network, &allow, &deny)?.into(),
                None if via.is_some() && allow.is_empty() && deny.is_empty() => current.clone(),
                None if via.is_some() => {
                    return Err(CuaError::InvalidArgument(
                        "--allow and --deny go with --network".into(),
                    ));
                }
                None => {
                    return Err(CuaError::InvalidArgument(
                        "say what to change: --network, --via or both".into(),
                    ));
                }
            };
            settings.via = match via {
                Some(v) => Some(v),
                None => current.via.clone(),
            };
            let r = spaces
                .set_network(space.clone(), settings, widen, tree)
                .await?;
            if json {
                line(
                    out,
                    serde_json::json!({
                        "applied": r.applied,
                        "network": info_json(&r.network),
                    })
                    .to_string(),
                );
            } else {
                line(
                    out,
                    format!(
                        "{space}: {}. {}",
                        r.network.summary,
                        if r.applied == "now" {
                            "In effect now."
                        } else {
                            "Applies when it starts next (`cua spaces stop` then `start`)."
                        }
                    ),
                );
                for w in &r.network.widenings {
                    line(out, format!("  ⚠ {w}"));
                }
                if let Some(note) = &r.network.note {
                    line(out, format!("  {note}"));
                }
            }
        }
        NetworkCmd::Log { space } => {
            let events = spaces.network_log(space.clone()).await?;
            if json {
                line(
                    out,
                    serde_json::Value::Array(
                        events
                            .iter()
                            .map(|e| {
                                serde_json::json!({
                                    "reason": e.reason,
                                    "dst": e.dst,
                                    "proto": e.proto,
                                    "count": e.count,
                                    "last_unix": e.last_unix,
                                })
                            })
                            .collect(),
                    )
                    .to_string(),
                );
            } else if events.is_empty() {
                line(out, format!("{space}: nothing blocked"));
            } else {
                line(out, format!("{space}: blocked, most recent first"));
                for e in &events {
                    line(out, format!("  {}", event_line(e)));
                }
            }
        }
    }
    Ok(0)
}

/// Runs `cua hotspot`.
pub async fn run_hotspot(
    cua: &Arc<Cua>,
    cmd: HotspotCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let spaces = cua.spaces();
    match cmd {
        HotspotCmd::Start {
            space,
            system_proxy,
            bypass,
        } => {
            let s = spaces.space(space.clone()).await?;
            let st = s.start_hotspot(Some(system_proxy), Some(bypass)).await?;
            if json {
                line(out, hotspot_json(&st).to_string());
            } else {
                line(
                    out,
                    format!(
                        "{space}: internet traffic leaves through this computer's network \
                         (its own rules still apply). SOCKS at {} in the Space.",
                        st.socks_address
                    ),
                );
                line(
                    out,
                    "  To keep it across restarts: cua network set <space> --via this-computer",
                );
            }
        }
        HotspotCmd::Stop { space } => {
            let s = spaces.space(space.clone()).await?;
            let stopped = s.stop_hotspot().await?;
            if json {
                line(out, serde_json::json!({ "stopped": stopped }).to_string());
            } else if stopped.is_empty() {
                line(out, format!("{space}: no hotspot was on"));
            } else {
                line(out, format!("{space}: hotspot off"));
            }
        }
        HotspotCmd::Status { space } => {
            let s = spaces.space(space.clone()).await?;
            let all = s.hotspot_status().await?;
            if json {
                line(
                    out,
                    serde_json::Value::Array(all.iter().map(hotspot_json).collect()).to_string(),
                );
            } else if all.is_empty() || all.iter().all(|h| h.state == "stopped") {
                line(out, format!("{space}: hotspot off"));
            } else {
                for h in &all {
                    line(out, hotspot_line(&space, h));
                }
            }
        }
    }
    Ok(0)
}

/// One hotspot in plain words.
fn hotspot_line(space: &str, h: &cua_sdk::SpaceHotspotStatus) -> String {
    let state = match h.state.as_str() {
        "active" => "on",
        "waiting_for_peer" => "starting",
        other => other,
    };
    format!(
        "{space}: hotspot {state}, {} open connections, {} out, {} in",
        h.active_connections,
        human_bytes(h.bytes_out),
        human_bytes(h.bytes_in)
    )
}

fn hotspot_json(h: &cua_sdk::SpaceHotspotStatus) -> serde_json::Value {
    serde_json::json!({
        "space": h.space,
        "state": h.state,
        "hotspot_id": h.hotspot_id,
        "socks_address": h.socks_address,
        "active_connections": h.active_connections,
        "bytes_out": h.bytes_out,
        "bytes_in": h.bytes_in,
        "served_here": h.served_here,
    })
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Runs `cua mesh`.
pub async fn run_mesh(cmd: MeshCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let home = crate::util::cua_home();
    match cmd {
        MeshCmd::Status { prefer } => {
            if let Some(word) = prefer {
                let p = cua_spaces::paths::Prefer::parse(&word).ok_or_else(|| {
                    CuaError::InvalidArgument(format!(
                        "--prefer {word:?}: expected lan, direct, relay or auto"
                    ))
                })?;
                cua_spaces::paths::set_prefer(&home, p).await?;
            }
            let paths = cua_spaces::paths::machine_paths(&home).await;
            if json {
                line(
                    out,
                    serde_json::to_string(&paths).map_err(crate::util::internal)?,
                );
            } else {
                for l in mesh_lines(&paths) {
                    line(out, l);
                }
            }
        }
    }
    Ok(0)
}

/// `cua mesh status`'s lines.
fn mesh_lines(p: &cua_spaces::paths::Paths) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(n) = &p.note {
        out.push(n.clone());
    }
    if p.prefer != "auto" {
        out.push(format!(
            "Pinned: {} (cua mesh status --prefer auto lifts it)",
            p.prefer
        ));
    }
    for m in &p.machines {
        let kind = match m.kind.as_str() {
            "lan" => "Local network",
            "direct" => "Direct",
            _ => "Relay",
        };
        let off = if m.online { "" } else { " (offline)" };
        out.push(format!("{}{off}: {kind}. {}", m.name, m.why));
    }
    out
}

/// One blocked kind of connection, in plain words: "Blocked: this
/// computer's localhost (10.0.2.2:5432), 3 times".
fn event_line(e: &cua_sdk::NetworkEvent) -> String {
    let what = reason_words(&e.reason);
    let times = if e.count == 1 {
        "once".to_string()
    } else {
        format!("{} times", e.count)
    };
    format!("Blocked: {what} ({}), {times}", e.dst)
}

/// What a reason id means, in plain words.
pub(crate) fn reason_words(reason: &str) -> &'static str {
    match reason {
        "platform:metadata" => "cloud metadata",
        "platform:daemon" => "Cua on this computer",
        "platform:other-tree" => "a Space in another tree",
        "tree:tcp-only" => "its tree over UDP (VM Spaces of a tree talk over TCP only)",
        "platform:loopback" | "default:host-loopback" => "this computer's localhost",
        "default:host" | "default:host-address" => "this computer",
        "default:lan" => "your local network",
        "default:reserved" | "platform:multicast" => "a reserved address",
        "default:none" => "the network (this Space has none)",
        "user:deny" => "an address you blocked",
        "dns:stripped" | "dns_stripped" => "a name that pointed to a private address",
        _ => "a destination its settings don't allow",
    }
}

/// The JSON `show` prints.
fn info_json(i: &SpaceNetworkInfo) -> serde_json::Value {
    serde_json::json!({
        "settings": {
            "preset": i.settings.preset,
            "localhost_ports": i.settings.localhost_ports,
            "all_localhost_ports": i.settings.all_localhost_ports,
            "allow": i.settings.allow,
            "deny": i.settings.deny,
            "custom_base": i.settings.custom_base,
            "widened_by": i.settings.widened_by,
        },
        "summary": i.summary,
        "widenings": i.widenings,
        "enforcement": i.enforcement,
        "point": i.point,
        "note": i.note,
        "parent": i.parent,
        "root": i.root,
        "always_blocked": i.always_blocked,
        "notice_pending": i.notice_pending,
        "tree_address": i.tree_address,
        "via": i.settings.via,
        "topology": serde_json::from_str::<serde_json::Value>(&i.topology_json)
            .unwrap_or(serde_json::Value::Null),
    })
}

/// `show`'s lines, in plain words.
fn show_lines(space: &str, i: &SpaceNetworkInfo) -> Vec<String> {
    let mut out = vec![format!("{space}: {}", i.summary)];
    if !i.settings.allow.is_empty() {
        out.push(format!("  Allowed: {}", i.settings.allow.join(", ")));
    }
    if !i.settings.deny.is_empty() {
        out.push(format!("  Blocked: {}", i.settings.deny.join(", ")));
    }
    for w in &i.widenings {
        out.push(format!("  ⚠ {w}"));
    }
    if let Some(p) = &i.parent {
        out.push(format!("  Within its parent {p}"));
    }
    if let Some(a) = &i.tree_address {
        out.push(format!(
            "  Tree address: {a} (its tree's other Spaces reach it here)"
        ));
    }
    if let Some(w) = &i.settings.widened_by {
        out.push(format!("  Allowed more than its parent (by {w})"));
    }
    out.extend(topology_lines(&i.topology_json));
    match i.enforcement.as_str() {
        "enforced" => out.push(format!(
            "  Enforced by this computer, outside the Space: {}",
            i.point
        )),
        "no_network" => out.push("  No network: this runtime can't protect it yet".into()),
        "open" => out.push("  No protection: chosen by you".into()),
        "refused" => out.push(format!("  Can't start like this: {}", i.point)),
        _ if !i.point.is_empty() => out.push(format!("  {}", i.point)),
        _ => {}
    }
    if let Some(n) = &i.note {
        out.push(format!("  {n}"));
    }
    out.push("  Always blocked:".into());
    for b in &i.always_blocked {
        out.push(format!("    {b}"));
    }
    out
}

/// The topology's lines for `show`: its name, its exit and how it's
/// doing, the host's ports and the path to other computers.
fn topology_lines(topology_json: &str) -> Vec<String> {
    let Ok(t) = serde_json::from_str::<serde_json::Value>(topology_json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(name) = t["space"]["name"].as_str() {
        out.push(format!("  Name: {name} (its tree reaches it by this name)"));
    }
    if let Some(label) = t["via"]["label"].as_str() {
        let state = match t["exit"]["state"].as_str() {
            Some("active") => "working".to_string(),
            Some("unreachable") => format!(
                "not reachable, so its internet is off: {}",
                t["exit"]["note"].as_str().unwrap_or("the exit is gone")
            ),
            Some(s) => s.replace('_', " "),
            None => "starting".to_string(),
        };
        out.push(format!("  Exits through: {label} ({state})"));
    }
    if let Some(reach) = t["host"]["reach"].as_array().filter(|r| !r.is_empty()) {
        let list: Vec<&str> = reach.iter().filter_map(|r| r.as_str()).collect();
        out.push(format!("  This computer's ports: {}", list.join(", ")));
    }
    if let Some(skipped) = t["host_ports"]["at_host_only"].as_array() {
        for s in skipped {
            if let (Some(port), Some(why)) = (s[0].as_u64(), s[1].as_str()) {
                out.push(format!(
                    "    localhost:{port} is taken in the Space ({why}); use host:{port}"
                ));
            }
        }
    }
    // Members that run on other computers, and how traffic gets to them.
    if let Some(members) = t["tree"]["members"].as_array() {
        for m in members {
            if let (Some(name), Some(machine)) = (m["name"].as_str(), m["machine"].as_str()) {
                out.push(format!("  {name} runs on {machine}"));
            }
        }
    }
    if let Some(note) = t["overlay"]["note"].as_str() {
        out.push(format!("  Other computers: {note}"));
    }
    let mut shown = false;
    if let Some(machines) = t["overlay"]["machines"].as_array() {
        for m in machines {
            let Some(name) = m["name"].as_str() else {
                continue;
            };
            let kind = match m["path"].as_str() {
                Some("lan") => "local network",
                Some("direct") => "direct",
                Some("relay") => "relay",
                _ => "finding a path",
            };
            let why = m["why"].as_str().unwrap_or_default();
            out.push(format!("  To {name}: {kind}. {why}").trim_end().to_string());
            shown = true;
        }
    }
    if !shown && let Some(machines) = t["paths"]["machines"].as_array() {
        for m in machines {
            if let (Some(name), Some(why)) = (m["name"].as_str(), m["why"].as_str()) {
                out.push(format!("  To {name}: {why}"));
            }
        }
    }
    if let Some(refused) = t["overlay"]["refused"].as_array() {
        for r in refused.iter().filter_map(|r| r.as_str()) {
            out.push(format!("  Can't reach {r}"));
        }
    }
    if let Some(note) = t["tree"]["relay"]["note"].as_str() {
        out.push(format!("  Can't reach members that started later: {note}"));
    }
    if let Some(url) = t["tree_proxy"]["url"].as_str() {
        out.push(format!(
            "  Members it has no route to (VMs, other computers): connect through {url}"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_names_the_exit_the_host_and_the_paths() {
        let t = serde_json::json!({
            "space": {"name": "worker-1.swarm"},
            "via": {"kind": "this-computer", "label": "This computer"},
            "exit": {"state": "active"},
            "host": {"reach": ["localhost:3000"]},
            "host_ports": {"at_localhost": [3000], "at_host_only": [[5432, "in use"]]},
            "paths": {"machines": [{"name": "studio-mac", "why": "Fastest path: the local network, 2 ms"}]},
            "overlay": {"active": true, "machines": [
                {"name": "studio-mac", "path": "lan", "why": "Fastest path: the local network, 2 ms", "members": ["db.swarm"]}
            ], "refused": ["web.swarm: the tree address is taken"]},
            "tree": {"members": [{"name": "db.swarm", "machine": "studio-mac"}], "relay": {"state": "missing", "note": "this VM started alone, with no tree relay"}},
            "tree_proxy": {"url": "socks5h://127.0.0.1:41920"},
        })
        .to_string();
        let l = topology_lines(&t).join("\n");
        assert!(l.contains("db.swarm runs on studio-mac"), "{l}");
        assert!(
            l.contains("Can't reach web.swarm: the tree address is taken"),
            "{l}"
        );
        assert!(
            l.contains("connect through socks5h://127.0.0.1:41920"),
            "{l}"
        );
        assert!(
            l.contains("Can't reach members that started later: this VM started alone"),
            "{l}"
        );
        assert!(l.contains("Name: worker-1.swarm"), "{l}");
        assert!(l.contains("Exits through: This computer (working)"), "{l}");
        assert!(l.contains("This computer's ports: localhost:3000"), "{l}");
        assert!(l.contains("use host:5432"), "{l}");
        assert!(
            l.contains("To studio-mac: local network. Fastest path: the local network"),
            "{l}"
        );
        assert!(!l.contains('\u{b7}'), "no middle dots: {l}");
        let gone = serde_json::json!({
            "via": {"label": "gateway"},
            "exit": {"state": "unreachable", "note": "gateway is stopped"},
        })
        .to_string();
        let l = topology_lines(&gone).join("\n");
        assert!(l.contains("its internet is off: gateway is stopped"), "{l}");
        assert!(topology_lines("").is_empty());
    }

    #[test]
    fn mesh_status_says_why() {
        let p = cua_spaces::paths::Paths {
            mesh: true,
            prefer: "lan".into(),
            machines: vec![cua_spaces::paths::MachinePath {
                machine: "m1".into(),
                name: "studio-mac".into(),
                online: true,
                kind: "lan".into(),
                path: "direct 192.168.1.4:41641".into(),
                rtt_ms: Some(2.0),
                why: "Pinned to the local network, 2 ms".into(),
                switches: 1,
            }],
            note: None,
        };
        let l = mesh_lines(&p).join("\n");
        assert!(l.contains("Pinned: lan"), "{l}");
        assert!(
            l.contains("studio-mac: Local network. Pinned to the local network"),
            "{l}"
        );
    }

    #[test]
    fn show_says_it_in_plain_words() {
        let info = SpaceNetworkInfo {
            settings: cua_sdk::NetworkSettings {
                preset: "localhost".into(),
                localhost_ports: vec![5432],
                deny: vec!["8.8.8.8".into()],
                ..Default::default()
            },
            summary: "Allow this computer's localhost (ports 5432)".into(),
            widenings: vec!["Allows this computer's localhost (ports 5432)".into()],
            enforcement: "enforced".into(),
            point: "nftables".into(),
            always_blocked: vec!["Cloud metadata".into()],
            ..Default::default()
        };
        let l = show_lines("studio", &info).join("\n");
        assert!(
            l.starts_with("studio: Allow this computer's localhost"),
            "{l}"
        );
        assert!(l.contains("Blocked: 8.8.8.8"), "{l}");
        assert!(l.contains("⚠ Allows this computer's localhost"), "{l}");
        assert!(
            l.contains("Enforced by this computer, outside the Space"),
            "{l}"
        );
        assert!(l.contains("Always blocked:\n    Cloud metadata"), "{l}");
        assert!(!l.contains("Tree address"), "{l}");
        let vm = SpaceNetworkInfo {
            tree_address: Some("198.19.0.4".into()),
            ..info.clone()
        };
        let l = show_lines("studio", &vm).join("\n");
        assert!(l.contains("Tree address: 198.19.0.4"), "{l}");
        let j = info_json(&info);
        assert_eq!(j["settings"]["localhost_ports"][0], 5432);
        assert_eq!(j["enforcement"], "enforced");
    }
}
