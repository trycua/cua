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
  cua network set studio --network internet")]
    Set {
        /// Space id or name.
        space: String,
        /// `internet`, `none`, `localhost:<ports>` (or `localhost:all`),
        /// `lan`, `custom`, `custom:none` or `unprotected` (No protection).
        #[arg(long, value_name = "PRESET")]
        network: String,
        /// Allow this destination (IP or CIDR; repeat for more).
        #[arg(long, value_name = "ADDR|CIDR")]
        allow: Vec<String>,
        /// Block this destination (IP, CIDR or `all`; repeat for more).
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
            allow,
            deny,
            widen,
            tree,
        } => {
            let settings = cua_spaces::network::parse_network(&network, &allow, &deny)?;
            let r = spaces
                .set_network(space.clone(), settings.into(), widen, tree)
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
            let _ = space;
            line(
                out,
                "Blocked connections are not reported yet. `cua network show` says what this \
                 Space can reach.",
            );
        }
    }
    Ok(0)
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
    if let Some(w) = &i.settings.widened_by {
        out.push(format!("  Allowed more than its parent (by {w})"));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let j = info_json(&info);
        assert_eq!(j["settings"]["localhost_ports"][0], 5432);
        assert_eq!(j["enforcement"], "enforced");
    }
}
