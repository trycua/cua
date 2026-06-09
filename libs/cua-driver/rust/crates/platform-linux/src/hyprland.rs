//! Hyprland window discovery and capture helpers.
//!
//! The primary Linux backend is X11/XWayland. On Hyprland, native Wayland
//! clients never appear in _NET_CLIENT_LIST, but hyprctl clients -j exposes
//! enough read-only metadata for list_windows and grim-based screenshots.

use anyhow::{bail, Result};
use serde::Deserialize;
use std::process::Command;

use crate::x11::WindowInfo;

#[derive(Debug, Deserialize)]
struct HyprClient {
    address: String,
    mapped: bool,
    hidden: bool,
    pid: i64,
    title: String,
    class: String,
    at: [i32; 2],
    size: [i32; 2],
}

pub fn list_windows(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    match list_windows_inner(filter_pid) {
        Ok(windows) => windows,
        Err(_) => Vec::new(),
    }
}

pub fn screenshot_window_bytes(window_id: u64) -> Result<Vec<u8>> {
    let client = clients()?
        .into_iter()
        .find(|c| parse_address(&c.address) == Some(window_id))
        .ok_or_else(|| anyhow::anyhow!("Hyprland client 0x{window_id:x} not found"))?;
    if client.size[0] <= 1 || client.size[1] <= 1 {
        bail!("Hyprland client 0x{window_id:x} has invalid geometry");
    }

    let geometry = format!(
        "{},{} {}x{}",
        client.at[0], client.at[1], client.size[0], client.size[1]
    );
    let out = Command::new("grim")
        .args(["-g", &geometry, "-t", "png", "-"])
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("grim failed for Hyprland geometry {geometry}");
    }
    Ok(out.stdout)
}

fn list_windows_inner(filter_pid: Option<u32>) -> Result<Vec<WindowInfo>> {
    let mut out = Vec::new();
    for client in clients()? {
        if !client.mapped || client.hidden || client.size[0] <= 1 || client.size[1] <= 1 {
            continue;
        }
        let pid = u32::try_from(client.pid).ok();
        if let Some(filter_pid) = filter_pid {
            if pid != Some(filter_pid) {
                continue;
            }
        }
        let Some(window_id) = parse_address(&client.address) else {
            continue;
        };
        let title = if client.title.trim().is_empty() {
            client.class
        } else {
            client.title
        };
        if title.trim().is_empty() {
            continue;
        }
        out.push(WindowInfo {
            xid: window_id,
            pid,
            title,
            x: client.at[0],
            y: client.at[1],
            width: client.size[0] as u32,
            height: client.size[1] as u32,
        });
    }
    Ok(out)
}

fn clients() -> Result<Vec<HyprClient>> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return Ok(Vec::new());
    }
    let out = Command::new("hyprctl").args(["clients", "-j"]).output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("hyprctl clients -j failed");
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn parse_address(address: &str) -> Option<u64> {
    u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()
}
