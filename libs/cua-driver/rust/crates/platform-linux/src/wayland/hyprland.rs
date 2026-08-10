//! Hyprland-specific identity resolution for per-toplevel capture.
//!
//! Standard foreign-toplevel protocols do not expose process identity or an
//! opaque handle accepted by Hyprland's toplevel-export protocol. Hyprland's
//! user-owned IPC supplies that missing correlation. Capture is authorized only
//! when one mapped client owned by the requested PID matches the compositor
//! title and app-id observed on the Wayland connection.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::process::Command;

#[derive(Clone, Debug, Deserialize)]
struct Client {
    address: String,
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    pid: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    class: String,
}

pub fn is_session() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .is_some_and(|desktop| desktop.to_ascii_lowercase().contains("hyprland"))
}

/// Resolve one exact Hyprland compositor address for a Wayland observation.
/// Ambiguous title/app-id matches fail closed rather than selecting a sibling.
pub fn resolve_capture_address(
    window_id: u64,
    target_pid: Option<u32>,
    title: &str,
    app_id: &str,
) -> Result<u64> {
    if !is_session() {
        bail!("not a Hyprland session");
    }
    let target_pid = target_pid.context("Hyprland capture requires a verified target PID")?;
    resolve_from_clients(&clients()?, window_id, target_pid, title, app_id)
}

fn resolve_from_clients(
    clients: &[Client],
    window_id: u64,
    target_pid: u32,
    title: &str,
    app_id: &str,
) -> Result<u64> {
    let mut owned: Vec<(u64, &Client)> = clients
        .iter()
        .filter(|client| client.mapped && !client.hidden && client.pid == i64::from(target_pid))
        .filter_map(|client| parse_address(&client.address).map(|address| (address, client)))
        .collect();

    if let Some((address, _)) = owned.iter().find(|(address, _)| *address == window_id) {
        return Ok(*address);
    }

    owned.retain(|(_, client)| {
        let title_matches = !title.is_empty() && client.title == title;
        let app_matches = !app_id.is_empty() && client.class == app_id;
        if !title.is_empty() && !app_id.is_empty() {
            title_matches && app_matches
        } else {
            title_matches || app_matches
        }
    });

    match owned.as_slice() {
        [(address, _)] => Ok(*address),
        [] => bail!("no mapped Hyprland client owned by PID {target_pid} matched title/app-id"),
        matches => bail!(
            "Hyprland capture identity is ambiguous: {} PID-owned clients matched title/app-id",
            matches.len()
        ),
    }
}

fn clients() -> Result<Vec<Client>> {
    let binary = if std::path::Path::new("/usr/bin/hyprctl").is_file() {
        "/usr/bin/hyprctl"
    } else {
        "hyprctl"
    };
    let output = Command::new(binary)
        .args(["-j", "clients"])
        .output()
        .context("launch hyprctl for compositor identity")?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!("hyprctl clients failed");
    }
    serde_json::from_slice(&output.stdout).context("parse hyprctl clients JSON")
}

fn parse_address(address: &str) -> Option<u64> {
    u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(address: &str, pid: i64, title: &str, class: &str) -> Client {
        Client {
            address: address.to_owned(),
            mapped: true,
            hidden: false,
            pid,
            title: title.to_owned(),
            class: class.to_owned(),
        }
    }

    #[test]
    fn parses_full_hyprland_pointer_address() {
        assert_eq!(parse_address("0x55b5cd9af330"), Some(0x55b5cd9af330));
        assert_eq!(parse_address("invalid"), None);
    }

    #[test]
    fn identity_requires_pid_title_and_app_id() {
        let clients = [
            client("0x1111", 42, "Target", "fixture"),
            client("0x2222", 43, "Target", "fixture"),
            client("0x3333", 42, "Other", "fixture"),
        ];
        assert_eq!(
            resolve_from_clients(&clients, 0xff00, 42, "Target", "fixture").unwrap(),
            0x1111
        );
    }

    #[test]
    fn ambiguous_pid_owned_siblings_fail_closed() {
        let clients = [
            client("0x1111", 42, "Target", "fixture"),
            client("0x2222", 42, "Target", "fixture"),
        ];
        let error = resolve_from_clients(&clients, 0xff00, 42, "Target", "fixture")
            .expect_err("duplicate identities must not be guessed");
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn exact_hyprland_address_wins_within_verified_pid() {
        let clients = [
            client("0x1111", 42, "First", "fixture"),
            client("0x2222", 42, "Second", "fixture"),
        ];
        assert_eq!(
            resolve_from_clients(&clients, 0x2222, 42, "stale", "stale").unwrap(),
            0x2222
        );
    }
}
