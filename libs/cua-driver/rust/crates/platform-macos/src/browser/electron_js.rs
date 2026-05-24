//! ElectronJS: detect Electron apps and evaluate JS via CDP inspector.

use std::time::Duration;
use std::collections::HashSet;

use super::cdp_client::CdpClient;

const DEFAULT_PORTS: &[u16] = &[9222, 9223, 9224, 9225, 9230];

pub struct ElectronJs;

impl ElectronJs {
    /// Returns true if the process at `pid` is an Electron app.
    pub fn is_electron(pid: i32) -> bool {
        // Look up the bundle path from list_running_apps.
        let apps = crate::apps::list_running_apps();
        let app = apps.iter().find(|a| a.pid == pid);

        if let Some(app) = app {
            // Try to infer bundle path from app name.
            let bundle_path = find_bundle_path_for_app(&app.name);
            if let Some(bp) = bundle_path {
                let electron_fw = format!("{bp}/Contents/Frameworks/Electron Framework.framework");
                if std::path::Path::new(&electron_fw).exists() {
                    return true;
                }
            }
        }

        // Fallback: check executable via lsof or ps.
        // Check if the executable itself links Electron.
        if let Ok(exe) = get_exe_path(pid) {
            // Check parent dir for Electron Framework.
            let exe_path = std::path::Path::new(&exe);
            if let Some(parent) = exe_path.parent() {
                // e.g. /Applications/Slack.app/Contents/MacOS/Slack → look for Frameworks sibling
                if let Some(macos_dir) = parent.parent() {
                    let electron_fw = macos_dir.join("Frameworks/Electron Framework.framework");
                    if electron_fw.exists() {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Evaluate JavaScript in the Electron app at `pid` via CDP.
    pub async fn execute(javascript: &str, pid: i32) -> anyhow::Result<String> {
        // 1. Check default ports for an existing CDP page target.
        if let Some(port) = CdpClient::find_page_target(DEFAULT_PORTS).await {
            return CdpClient::evaluate(javascript, port).await;
        }

        // 2. Check listening ports of this process for a CDP endpoint.
        let existing_ports = listening_ports(pid).await;
        for &port in &existing_ports {
            if CdpClient::is_available(port).await {
                return CdpClient::evaluate(javascript, port).await;
            }
        }

        // 3. Snapshot current ports, then send SIGUSR1 to open the inspector.
        let before_ports: HashSet<u16> = existing_ports.iter().cloned().collect();

        unsafe {
            libc::kill(pid, libc::SIGUSR1);
        }

        // 4. Poll for new port.
        let mut new_port: Option<u16> = None;
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let current = listening_ports(pid).await;
            for &p in &current {
                if !before_ports.contains(&p) && CdpClient::is_available(p).await {
                    new_port = Some(p);
                    break;
                }
            }
            if new_port.is_some() { break; }
        }

        let port = new_port.ok_or_else(|| {
            anyhow::anyhow!("Could not find Electron inspector port for pid {pid}. \
                             Try launching with --inspect or --remote-debugging-port.")
        })?;

        CdpClient::evaluate(javascript, port).await
    }
}

/// Get the executable path of a process using `ps`.
fn get_exe_path(pid: i32) -> anyhow::Result<String> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if s.is_empty() {
        anyhow::bail!("No exe for pid {pid}");
    }
    Ok(s)
}

/// Find the .app bundle path for an app by name (searches common locations).
fn find_bundle_path_for_app(name: &str) -> Option<String> {
    let dirs = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    let user_apps = format!("{home}/Applications");

    let mut all_dirs: Vec<&str> = dirs.to_vec();
    let user_str: &str = &user_apps;
    all_dirs.push(user_str);

    for dir in all_dirs {
        let candidate = format!("{dir}/{name}.app");
        if std::path::Path::new(&candidate).exists() {
            return Some(candidate);
        }
    }
    None
}

/// Return TCP listening ports for a process via `lsof`.
async fn listening_ports(pid: i32) -> Vec<u16> {
    let out = tokio::process::Command::new("lsof")
        .args([
            "-p", &pid.to_string(),
            "-iTCP", "-sTCP:LISTEN",
            "-Fn", "-P",
        ])
        .output()
        .await;

    let Ok(out) = out else { return vec![] };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut ports = Vec::new();
    for line in text.lines() {
        // Lines starting with 'n' contain the address e.g. n*:9222 or n127.0.0.1:9222
        let trimmed = line.trim();
        if !trimmed.starts_with('n') { continue; }
        if let Some(colon_pos) = trimmed.rfind(':') {
            let port_str = &trimmed[colon_pos + 1..];
            if let Ok(p) = port_str.parse::<u16>() {
                ports.push(p);
            }
        }
    }
    ports
}
