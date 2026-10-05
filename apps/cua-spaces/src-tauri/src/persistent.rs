// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Persistent agents, the Cua Volume and the notifications feed for the
//! Agents, Drive and Notifications pages. The pages are the app core's
//! (`agents.page*`, `drive.*`, `notifications.*`, shared with the SwiftUI
//! app); these commands run their requests through the `cua daemon`, which
//! owns persistent agents (their supervisor and routine clock), keeps the
//! notifications feed and asks for presence before anything widens access.

use std::path::{Component, Path, PathBuf};

use cua_proto::daemon::v1 as dpb;
use serde_json::Value;

use crate::core::{AppCore, CmdResult};

/// The Spaces tools these pages may call (nothing that creates or deletes).
pub const TOOLS: &[&str] = &[
    "persistent_agent_list",
    "persistent_agent_save",
    "agent_pause",
    "agent_resume",
    "routine_add",
    "routine_list",
    "routine_remove",
    "routine_set_enabled",
    "notifications_list",
    "notifications_ack",
    "computer_access_grant",
    "computer_access_revoke",
    "computer_access_list",
    "volume_ls",
    "volume_read",
    "volume_history",
    "volume_restore",
    "volume_requests",
    "volume_approve",
    "volume_deny",
    "volume_grants",
    "volume_revoke",
    // The drive's storage, mount, sync and cache (Settings, the first run's
    // Cua Volume page, the Drive page). S3 keys only ever travel in
    // `volume_storage_set`'s arguments.
    "volume_storage",
    "volume_storage_set",
    "volume_mount_status",
    "volume_mount",
    "volume_unmount",
    "volume_sync_status",
    "volume_sync_resolve",
    "volume_cache_stats",
    "volume_cache_set",
    "volume_cache_clear",
];

/// The JSON a Spaces tool answered with, or its error message.
pub fn tool_result(content_json: &str, is_error: bool) -> CmdResult<Value> {
    let content: Value = serde_json::from_str(content_json).map_err(|e| e.to_string())?;
    let text = content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if is_error {
        return Err(text.trim_start_matches("error: ").to_string());
    }
    serde_json::from_str(&text).or(Ok(Value::String(text)))
}

impl AppCore {
    /// Runs one of [`TOOLS`] in the daemon (started when needed).
    pub async fn agents_tool(&self, tool: &str, args: Value) -> CmdResult<Value> {
        if !TOOLS.contains(&tool) {
            return Err(format!("{tool} is not available to this page"));
        }
        let daemon = self.daemon(true).await?;
        let r = daemon
            .spaces()
            .call_space_tool(dpb::CallSpaceToolRequest {
                name: tool.into(),
                arguments_json: args.to_string(),
            })
            .await
            .map_err(|e| e.message().to_string())?
            .into_inner();
        tool_result(&r.content_json, r.is_error)
    }
}

/// `path` with a leading `~/` (or a bare `~`) under `home`.
fn expand_home(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    match (path.strip_prefix('~'), home) {
        (Some(""), Some(h)) => Some(h.to_path_buf()),
        (Some(rest), Some(h)) if rest.starts_with('/') => {
            Some(h.join(rest.trim_start_matches('/')))
        }
        (Some(_), _) => None,
        (None, _) => Some(PathBuf::from(path)),
    }
}

/// An absolute path with no `.` or `..` parts (so a prefix check means
/// what it says).
fn plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|c| {
            matches!(
                c,
                Component::RootDir | Component::Prefix(_) | Component::Normal(_)
            )
        })
}

/// What "Show in Finder" may reveal: `path`, only when it lies inside the
/// drive's current mount point (`volume_mount_status` answered `mounted`
/// with that `path`). Anything else is refused. Lexical; the caller also
/// checks the resolved paths so a symlink cannot lead outside.
pub fn reveal_target(
    mount: &Value,
    path: &str,
    home: Option<&Path>,
) -> Result<(PathBuf, PathBuf), String> {
    let root = mount
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty() && mount.get("state").and_then(Value::as_str) == Some("mounted"))
        .ok_or("Cua Volume is not mounted")?;
    let root = expand_home(root, home)
        .filter(|r| plain_absolute(r))
        .ok_or("Cua Volume's mount point is not an absolute path")?;
    let target = expand_home(path, home)
        .filter(|t| plain_absolute(t) && t.starts_with(&root))
        .ok_or_else(|| format!("{path} is not inside Cua Volume"))?;
    Ok((root, target))
}

/// Shows `target` in the file manager: selected in Finder (`open -R`), or
/// its folder on Linux (`xdg-open`). One argv, no shell.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn show_in_file_manager(target: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("/usr/bin/open");
        c.arg("-R").arg(target);
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let folder = if target.is_dir() {
            target
        } else {
            target.parent().unwrap_or(target)
        };
        let mut c = std::process::Command::new("xdg-open");
        c.arg(folder);
        c
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not show {}: {e}", target.display()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn show_in_file_manager(_target: &Path) -> Result<(), String> {
    Err("Cua Volume does not mount on this system".into())
}

impl AppCore {
    /// Shows a path inside the mounted Cua Volume in the file manager. The
    /// daemon's `volume_mount_status` says where the drive is mounted right
    /// now; any path outside it is refused.
    pub async fn drive_reveal(&self, path: &str) -> CmdResult<()> {
        let mount = self
            .agents_tool("volume_mount_status", Value::Object(Default::default()))
            .await?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let (root, target) = reveal_target(&mount, path, home.as_deref())?;
        // Resolved, too: a symlink inside the drive must not lead out of it.
        let real_root =
            std::fs::canonicalize(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        let real = std::fs::canonicalize(&target).map_err(|e| format!("{path}: {e}"))?;
        if !real.starts_with(&real_root) {
            return Err(format!("{path} is not inside Cua Volume"));
        }
        show_in_file_manager(&real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mounted(path: &str) -> Value {
        json!({ "enabled": true, "state": "mounted", "method": "fskit", "path": path, "volume_name": "Cua Volume" })
    }

    // POSIX mount points: Cua Volume mounts on macOS and Linux only (Windows
    // has no reveal, see `show_in_file_manager`), and there `/Volumes/…` is
    // not an absolute path.
    #[cfg(unix)]
    #[test]
    fn reveal_only_inside_the_current_mount() {
        let m = mounted("/Volumes/Cua Volume");
        let ok = |p: &str| reveal_target(&m, p, None).map(|(_, t)| t);
        assert_eq!(
            ok("/Volumes/Cua Volume").unwrap(),
            PathBuf::from("/Volumes/Cua Volume")
        );
        assert_eq!(
            ok("/Volumes/Cua Volume/public/plan (conflict from maya-linux 2026-09-29 14.02.11).md")
                .unwrap(),
            PathBuf::from(
                "/Volumes/Cua Volume/public/plan (conflict from maya-linux 2026-09-29 14.02.11).md"
            )
        );
        for outside in [
            "/Volumes/Cua Volume/../Macintosh HD/etc/passwd",
            "/Volumes/Cua Volume2/x",
            "/Volumes/Cua",
            "/etc/passwd",
            "Cua Volume/x",
            "",
            "~/Cua Volume/x",
        ] {
            assert_eq!(
                ok(outside).unwrap_err(),
                format!("{outside} is not inside Cua Volume"),
                "{outside}"
            );
        }
    }

    #[test]
    fn reveal_needs_a_mounted_drive() {
        for status in [
            json!({ "enabled": true, "state": "needs_approval", "method": "fskit", "path": "/Volumes/Cua Volume" }),
            json!({ "enabled": false, "state": "off", "method": "fskit", "path": null }),
            json!({ "enabled": true, "state": "mounted", "method": "fskit", "path": "" }),
            json!("mounted"),
        ] {
            assert_eq!(
                reveal_target(&status, "/Volumes/Cua Volume/x", None).unwrap_err(),
                "Cua Volume is not mounted"
            );
        }
        assert_eq!(
            reveal_target(
                &mounted("relative/Cua Volume"),
                "relative/Cua Volume/x",
                None
            )
            .unwrap_err(),
            "Cua Volume's mount point is not an absolute path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reveal_expands_a_home_mount_point() {
        let home = Path::new("/home/maya");
        let m = mounted("~/Cua Volume");
        assert_eq!(
            reveal_target(&m, "/home/maya/Cua Volume/public/a.md", Some(home)).unwrap(),
            (
                PathBuf::from("/home/maya/Cua Volume"),
                PathBuf::from("/home/maya/Cua Volume/public/a.md")
            )
        );
        assert_eq!(
            reveal_target(&m, "~/Cua Volume/public", Some(home))
                .unwrap()
                .1,
            PathBuf::from("/home/maya/Cua Volume/public")
        );
        assert!(reveal_target(&m, "/home/maya/Documents", Some(home)).is_err());
        assert!(
            reveal_target(&m, "~/Cua Volume/x", None).is_err(),
            "no home, no ~"
        );
    }

    #[test]
    fn the_drive_tools_are_available_to_the_pages() {
        for tool in [
            "volume_storage",
            "volume_storage_set",
            "volume_mount_status",
            "volume_mount",
            "volume_unmount",
            "volume_sync_status",
            "volume_sync_resolve",
            "volume_cache_stats",
            "volume_cache_set",
            "volume_cache_clear",
        ] {
            assert!(TOOLS.contains(&tool), "{tool}");
        }
    }

    #[test]
    fn results_and_errors_are_unwrapped() {
        let ok = r#"[{"type":"text","text":"{\n  \"agents\": []\n}"}]"#;
        assert_eq!(
            tool_result(ok, false).unwrap()["agents"],
            serde_json::json!([])
        );
        let err =
            r#"[{"type":"text","text":"error: forbidden: agent:ada may not read agents/bob/x"}]"#;
        assert_eq!(
            tool_result(err, true).unwrap_err(),
            "forbidden: agent:ada may not read agents/bob/x"
        );
        assert!(!TOOLS.contains(&"delete_space"));
    }
}
