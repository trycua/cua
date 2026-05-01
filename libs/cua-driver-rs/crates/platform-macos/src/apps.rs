//! macOS app enumeration via NSWorkspace and NSRunningApplication.

use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub running: bool,
    pub active: bool,
}

/// Enumerate running apps with NSApplicationActivationPolicyRegular.
/// Uses `osascript` to query System Events — no Accessibility permission needed.
pub fn list_running_apps() -> Vec<AppInfo> {
    // Use `ps` + NSWorkspace via a brief Swift one-liner via swift-sh is heavy;
    // instead use the Obj-C bridge via `osascript`.
    // Fallback: parse `ps -axo pid,command` and cross-reference with bundle IDs.
    // Better: use a native call via core-foundation bindings.
    list_running_apps_native()
}

fn list_running_apps_native() -> Vec<AppInfo> {
    // Use `lsappinfo list` which is a macOS system tool available on all versions.
    // Alternatively we can use NSWorkspace via objc2 — but for simplicity and
    // to match the Swift reference, we use a subprocess call to `osascript`.
    let script = r#"
set output to ""
tell application "System Events"
    set appList to every application process whose background only is false
    repeat with proc in appList
        set procName to name of proc
        set procPid to unix id of proc
        set bundleId to bundle identifier of proc
        if bundleId is missing value then set bundleId to ""
        set isFront to "0"
        if frontmost of proc then set isFront to "1"
        set output to output & procName & "|" & procPid & "|" & bundleId & "|" & isFront & linefeed
    end repeat
end tell
return output
"#;

    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();

    match out {
        Err(_) => vec![],
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            parse_osascript_app_list(&text)
        }
    }
}

fn parse_osascript_app_list(text: &str) -> Vec<AppInfo> {
    let mut apps = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() < 2 { continue; }
        let name = parts[0].trim().to_owned();
        let pid: i32 = parts[1].trim().parse().unwrap_or(0);
        let bundle_id = if parts.len() > 2 && !parts[2].trim().is_empty() {
            Some(parts[2].trim().to_owned())
        } else {
            None
        };
        let active = parts.get(3).map(|s| s.trim() == "1").unwrap_or(false);
        if name.is_empty() || pid == 0 { continue; }
        apps.push(AppInfo { name, pid, bundle_id, running: true, active });
    }
    apps
}

/// Launch an app by bundle ID using `open -g -b` (background, no activation).
/// Returns the pid on success.
pub fn launch_app(bundle_id: &str) -> anyhow::Result<i32> {
    let status = Command::new("open")
        .args(["-g", "-b", bundle_id])
        .status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch {bundle_id}");
    }
    // Give the app a moment to start.
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Find its pid.
    let apps = list_running_apps();
    for app in &apps {
        if app.bundle_id.as_deref() == Some(bundle_id) {
            return Ok(app.pid);
        }
    }
    anyhow::bail!("Launched {bundle_id} but could not find its pid")
}

/// Launch an app by display name using `open -g -a AppName` (background, no activation).
/// Returns the pid on success.
pub fn launch_app_by_name(name: &str) -> anyhow::Result<i32> {
    let status = Command::new("open")
        .args(["-g", "-a", name])
        .status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch app '{name}'");
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let apps = list_running_apps();
    for app in &apps {
        if app.name.eq_ignore_ascii_case(name) {
            return Ok(app.pid);
        }
    }
    anyhow::bail!("Launched '{name}' but could not find its pid")
}

/// Return the localized application name for a running process by PID.
/// Uses `ps -p {pid} -o comm=` which gives the command name without path.
/// Returns `None` if the PID is unknown or the command fails.
pub fn get_app_name_for_pid(pid: i32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // Strip path prefix: "/Applications/Safari.app/Contents/MacOS/Safari" → "Safari"
    Some(
        std::path::Path::new(&raw)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&raw)
            .to_string(),
    )
}

/// Format the app list in the same text style as libs/cua-driver.
pub fn format_app_list(apps: &[AppInfo]) -> String {
    let running: Vec<&AppInfo> = apps.iter().filter(|a| a.running).collect();
    let total = apps.len();
    let mut lines = vec![format!(
        "✅ Found {} app(s): {} running, {} installed-not-running.",
        total,
        running.len(),
        total - running.len()
    )];
    for app in &running {
        let bundle = app.bundle_id.as_deref().map(|b| format!(" [{b}]")).unwrap_or_default();
        lines.push(format!("- {} (pid {}){}", app.name, app.pid, bundle));
    }
    lines.join("\n")
}
