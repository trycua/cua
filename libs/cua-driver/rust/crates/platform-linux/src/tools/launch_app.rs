// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

use super::impl_::{chromium_family_program, exec_basename, window_record_json};

// ── launch_app ───────────────────────────────────────────────────────────────

pub struct LaunchAppTool;
static LAUNCH_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// The command that starts `prog` with `rest`. With `agent_seat_lane` (1 or 2)
/// it runs inside a transient systemd user scope named
/// `cua-agent-seat-<lane>-<token>.scope`, which tells the Hyprland plugin to give
/// the app that lane's agent seat as its only `wl_seat` (see
/// `wayland::hyprland_input::agent_seat_unit`). `systemd-run --scope` moves
/// itself into the scope and then execs the program, so the child pid is the
/// program's.
fn build_launch_command(
    prog: &str,
    rest: &[String],
    agent_seat_lane: Option<u8>,
    token: &str,
    scoped: bool,
) -> std::process::Command {
    let mut launch = match agent_seat_lane.filter(|_| scoped) {
        Some(lane) => {
            let mut scoped = std::process::Command::new("systemd-run");
            scoped
                .args(["--user", "--scope", "--quiet", "--collect", "--unit"])
                .arg(crate::wayland::hyprland_input::agent_seat_unit(lane, token))
                .arg("--")
                .arg(prog);
            scoped
        }
        None => std::process::Command::new(prog),
    };
    launch
        .args(rest)
        // The child must not inherit the driver's stdio: in private-worker mode
        // stdout is the JSON-RPC stream to the SDK, and a chatty app (Chromium's
        // zygote logs, GTK warnings) writing there corrupts a response and shuts
        // the worker down.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Enable accessibility for this child without toggling GNOME's global
        // ScreenReaderEnabled setting (which can launch Orca). Native
        // toolkits ignore these when they do not need them.
        .env("ACCESSIBILITY_ENABLED", "1")
        .env("NO_AT_BRIDGE", "0");
    // The environment tag is the fallback where there is no systemd (apps that
    // keep their environment); a launch that did not ask never inherits one.
    match agent_seat_lane {
        Some(lane) => launch.env(
            crate::wayland::hyprland_input::AGENT_SEAT_ENV,
            lane.to_string(),
        ),
        None => launch.env_remove(crate::wayland::hyprland_input::AGENT_SEAT_ENV),
    };
    launch
}

/// A scope-name token unique to this launch: 16 hex digits.
fn launch_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!("{:016x}", nanos ^ (u64::from(std::process::id()) << 40))
}

/// Spawn a launcher command line (an executable plus arguments, e.g. an XDG
/// `Exec=` value with field codes stripped) in the background and return the
/// child pid. With `agent_seat_lane`, see [`build_launch_command`].
fn spawn_launch_command(
    cmd: &str,
    additional_arguments: &[String],
    agent_seat_lane: Option<u8>,
) -> std::io::Result<u32> {
    let mut parts = cmd.split_whitespace();
    let prog = parts.next().unwrap_or(cmd);
    let mut rest: Vec<String> = parts.map(str::to_owned).collect();
    rest.extend(additional_arguments.iter().cloned());
    if chromium_family_program(prog)
        && !rest
            .iter()
            .any(|arg| arg == "--force-renderer-accessibility")
    {
        rest.push("--force-renderer-accessibility".to_owned());
    }
    let child = build_launch_command(
        prog,
        &rest,
        agent_seat_lane,
        &launch_token(),
        systemd_run_available(),
    )
    .spawn()?;
    let pid = child.id();
    if let Some(lane) = agent_seat_lane {
        crate::wayland::hyprland_input::remember_agent_seat_launch(pid, usize::from(lane) - 1);
    }
    reap_in_background(child);
    Ok(pid)
}

/// Collect a launched child's exit status in the background.
///
/// `std::process::Child` does not reap on drop and the driver never waits on
/// an app it launches, so every launched app used to linger as a zombie for
/// the daemon's lifetime: it held a pid slot, and — because the pid stays
/// present — made "does this pid exist" liveness checks report a terminated
/// app as still running.
///
/// Reaping is scoped to one thread per child rather than setting
/// `SIGCHLD` to `SIG_IGN`, which is process-global and would break every
/// caller that needs an exit status, including the `xdg-open` check below
/// and any `Command::status()`/`output()` elsewhere in the daemon. The
/// thread blocks in `wait` until the app exits, so it costs nothing while
/// the app runs and never delays the launch itself.
fn reap_in_background(mut child: std::process::Child) {
    let pid = child.id();
    let reaper = std::thread::Builder::new()
        .name(format!("cua-reap-{pid}"))
        .stack_size(64 * 1024)
        .spawn(move || {
            if let Err(e) = child.wait() {
                tracing::debug!(pid, "launched app could not be reaped: {e}");
            }
        });
    if let Err(e) = reaper {
        tracing::debug!(pid, "could not start reaper thread for launched app: {e}");
    }
}

/// Match a launch_app `name` that failed direct exec against installed XDG
/// .desktop applications: exact display name, desktop-file id, or `Exec=`
/// basename first, then a display-name substring when it is unambiguous.
fn match_installed_app<'a>(
    apps: &'a [crate::installed_apps::InstalledApp],
    query: &str,
) -> Option<&'a crate::installed_apps::InstalledApp> {
    let q = query.to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    apps.iter()
        .find(|a| {
            a.name.to_ascii_lowercase() == q
                || a.bundle_id.to_ascii_lowercase() == q
                || exec_basename(&a.launch_path) == q
        })
        .or_else(|| {
            let mut matches = apps
                .iter()
                .filter(|a| a.name.to_ascii_lowercase().contains(&q));
            match (matches.next(), matches.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        })
}

/// Watch a just-spawned `xdg-open` long enough to catch a fast failure.
/// xdg-open's generic fallback can `exec` the target app and stay alive for
/// its whole lifetime, so a child still running after the grace period counts
/// as success; a quick non-zero exit (2 = file not found, 3 = no handler
/// tool, 4 = action failed) is the only reliable failure signal.
fn xdg_open_failure(mut child: std::process::Child) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        // A branch that observes an exit status has already reaped the child;
        // one that gives up on it still owes that, so hand it to the reaper
        // instead of dropping it and leaving a zombie behind.
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return None,
            Ok(Some(status)) => return Some(format!("xdg-open failed ({status})")),
            Ok(None) if std::time::Instant::now() >= deadline => {
                reap_in_background(child);
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => {
                reap_in_background(child);
                return Some(format!("could not observe xdg-open: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod launch_app_tests {
    use super::*;
    use crate::installed_apps::InstalledApp;

    fn app(name: &str, bundle_id: &str, launch_path: &str) -> InstalledApp {
        InstalledApp {
            name: name.to_owned(),
            bundle_id: bundle_id.to_owned(),
            launch_path: launch_path.to_owned(),
            last_used: None,
            startup_wm_class: None,
        }
    }

    fn fixture() -> Vec<InstalledApp> {
        vec![
            app("Galculator", "galculator", "galculator"),
            app(
                "Google Chrome",
                "google-chrome",
                "/usr/bin/google-chrome-stable",
            ),
            app("File Manager", "thunar", "thunar"),
            app(
                "File Manager Settings",
                "thunar-settings",
                "thunar-settings",
            ),
        ]
    }

    /// Process state letter from `/proc/<pid>/stat`, or `None` once the entry
    /// is gone. `comm` may itself contain spaces and parentheses, so the state
    /// is read after the final `)` rather than by splitting from the left.
    fn proc_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(')')?.1;
        after_comm.split_whitespace().next()?.chars().next()
    }

    #[test]
    fn launched_children_are_reaped_instead_of_lingering_as_zombies() {
        // A launched app that exits must not stay in the process table. The
        // driver never waits on what it launches, so without an explicit
        // reaper every launch leaked a pid slot for the daemon's lifetime and
        // left "does this pid exist" liveness checks reporting a terminated
        // app as still running.
        // `/bin/sh` rather than a richer coreutils binary: it is the one
        // executable POSIX and the Nix build sandbox both guarantee, and the
        // sandbox has no `/bin/true`.
        let pid = spawn_launch_command("/bin/sh -c exit", &[], None).expect("/bin/sh should spawn");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match proc_state(pid) {
                None => break,                    // reaped, entry gone
                Some(state) if state != 'Z' => {} // still running or exiting
                Some(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "pid {pid} was still a zombie after the reaper deadline"
                    );
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} was never reaped"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    #[test]
    fn agent_seat_request_validates_before_anything_is_launched() {
        assert_eq!(agent_seat_request(&json!({"name": "x"})), Ok(None));
        assert_eq!(agent_seat_request(&json!({"agent_seat": false})), Ok(None));
        assert!(agent_seat_request(&json!({"agent_seat_lane": 2}))
            .unwrap_err()
            .contains("only applies"));
        assert!(
            agent_seat_request(&json!({"agent_seat": true, "agent_seat_lane": 3}))
                .unwrap_err()
                .contains("1 or 2")
        );
        assert!(
            agent_seat_request(&json!({"agent_seat": true, "agent_seat_lane": "1"}))
                .unwrap_err()
                .contains("1 or 2")
        );
        // Valid lanes still need the plugin: a plain session has no agent seats.
        if !crate::wayland::hyprland_input::enabled() {
            assert!(agent_seat_request(&json!({"agent_seat": true}))
                .unwrap_err()
                .contains("Hyprland"));
        }
    }

    #[test]
    fn agent_seat_launch_runs_in_a_lane_scope_and_a_plain_launch_does_not() {
        let args = |c: &std::process::Command| -> Vec<String> {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let rest = vec!["--flag".to_owned(), "x".to_owned()];
        let plain = build_launch_command("chromium", &rest, None, "tok", true);
        assert_eq!(plain.get_program(), "chromium");
        assert_eq!(args(&plain), ["--flag", "x"]);
        let tagged = build_launch_command("chromium", &rest, Some(2), "tok123", true);
        assert_eq!(tagged.get_program(), "systemd-run");
        assert_eq!(
            args(&tagged),
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--unit",
                "cua-agent-seat-2-tok123.scope",
                "--",
                "chromium",
                "--flag",
                "x"
            ]
        );
        // Without systemd the app is tagged by its environment only.
        let unscoped = build_launch_command("chromium", &rest, Some(2), "tok", false);
        assert_eq!(unscoped.get_program(), "chromium");
        assert!(unscoped
            .get_envs()
            .any(|(k, v)| k == "CUA_AGENT_SEAT" && v.is_some_and(|v| v == "2")));
        assert!(plain
            .get_envs()
            .any(|(k, v)| k == "CUA_AGENT_SEAT" && v.is_none()));
        // The unit name is what the plugin and the driver read back from the cgroup.
        let listing = "0::/user.slice/app.slice/cua-agent-seat-2-tok123.scope\n";
        assert_eq!(
            crate::wayland::hyprland_input::agent_seat_lane_from_cgroup(listing),
            Some(1)
        );
        let token = launch_token();
        assert_eq!(token.len(), 16);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn matches_exact_display_name_case_insensitively() {
        let apps = fixture();
        let hit = match_installed_app(&apps, "google chrome").expect("should match");
        assert_eq!(hit.bundle_id, "google-chrome");
    }

    #[test]
    fn matches_desktop_file_id_and_exec_basename() {
        let apps = fixture();
        assert_eq!(
            match_installed_app(&apps, "galculator").unwrap().name,
            "Galculator"
        );
        assert_eq!(
            match_installed_app(&apps, "google-chrome-stable")
                .unwrap()
                .name,
            "Google Chrome"
        );
    }

    #[test]
    fn matches_unambiguous_display_name_substring() {
        let apps = fixture();
        assert_eq!(
            match_installed_app(&apps, "chrome").unwrap().name,
            "Google Chrome"
        );
    }

    #[test]
    fn refuses_ambiguous_substring_and_unknown_names() {
        let apps = fixture();
        // "file manager" is a substring of two entries — refuse to guess.
        // ("File Manager" itself still resolves via the exact-name rung.)
        assert!(match_installed_app(&apps, "file man").is_none());
        assert_eq!(
            match_installed_app(&apps, "file manager")
                .unwrap()
                .bundle_id,
            "thunar"
        );
        assert!(match_installed_app(&apps, "gnome-calculator").is_none());
        assert!(match_installed_app(&apps, "").is_none());
    }
}

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        LAUNCH_DEF.get_or_init(|| ToolDef {
            name: "launch_app".into(),
            description: "Launch a Linux app in the background. Provide launch_path (preferred — \
                round-trip the value from list_apps), name (tried as a direct command, then \
                matched against installed .desktop applications, then handed to xdg-open if it \
                is a URL or existing file path), bundle_id (ignored on Linux), or urls (list of \
                URLs to open). Resolution precedence: launch_path > name > bundle_id. Errors \
                when the name resolves to nothing launchable.".into(),
            input_schema: json!({"type":"object","properties":{
                "launch_path":{"type":"string","description":"Round-trip the `launch_path` returned by `list_apps` — the Exec= command from the .desktop file with XDG field codes already stripped. Highest precedence on Linux; spawned directly via the system shell."},
                "name":{"type":"string","description":"App name or command to launch. Tried as a direct command first, then matched against installed .desktop applications (exact display name, desktop-file id, or Exec basename; else an unambiguous display-name substring)."},
                "bundle_id":{"type":"string","description":"Ignored on Linux (macOS/Windows concept)."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open via xdg-open."},
                "additional_arguments":{"type":"array","items":{"type":"string"},"description":"Extra command-line arguments passed to the launched process."},
                "agent_seat":{"type":"boolean","description":"Hyprland with the Cua plugin only. Start the app on the agent seat so it accepts background clicks and typing even if it binds only its first wl_seat (Chromium, Electron). Its only seat is then the agent's: the person's own mouse and keyboard do not reach it while it runs. A browser or Electron app already running on the same profile takes the request over and is not tagged: quit it or use a separate profile first."},
                "agent_seat_lane":{"type":"integer","enum":[1,2],"description":"Which agent seat lane the app is bound to (default 1). Two agents driving apps at once use different lanes."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let launch_path_opt = args.opt_str("launch_path");
        let name_opt = args.opt_str("name");
        let urls: Vec<String> = args.str_array("urls");
        let additional_arguments: Vec<String> = args.str_array("additional_arguments");
        let agent_seat_lane = match agent_seat_request(&args) {
            Ok(lane) => lane,
            Err(refusal) => return ToolResult::error(refusal),
        };
        if args.get("cdp_debugging_port").is_some() {
            return ToolResult::error(
                "cdp_debugging_port moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }
        if launch_path_opt
            .as_deref()
            .into_iter()
            .chain(name_opt.as_deref())
            .chain(additional_arguments.iter().map(String::as_str))
            .any(cua_driver_core::launch_guard::contains_remote_debugging_flag)
        {
            return ToolResult::error(
                cua_driver_core::launch_guard::REMOTE_DEBUGGING_LAUNCH_REFUSAL,
            );
        }

        if launch_path_opt.is_none() && name_opt.is_none() && urls.is_empty() {
            return ToolResult::error("Provide at least one of: launch_path, name, or urls.");
        }
        if agent_seat_lane.is_some() && !urls.is_empty() {
            return ToolResult::error(
                "agent_seat needs launch_path or name: urls open in whatever instance xdg-open \
                 reaches, which the agent seat cannot be attached to.",
            );
        }

        // Snapshot the top-levels before spawning so a window created by an
        // already-running service (D-Bus activation) can be told apart.
        let windows_before: std::collections::HashSet<u64> =
            tokio::task::spawn_blocking(|| crate::wayland::list_windows_dispatch(None))
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|w| w.xid)
                .collect();
        let result = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(String, Option<u32>, String)> {
                // Open URLs via xdg-open.
                if !urls.is_empty() {
                    let mut children = Vec::new();
                    for url in &urls {
                        children.push((
                            url.clone(),
                            std::process::Command::new("xdg-open").arg(url).spawn()?,
                        ));
                    }
                    for (url, child) in children {
                        if let Some(reason) = xdg_open_failure(child) {
                            anyhow::bail!("could not open '{url}': {reason}");
                        }
                    }
                    return Ok((
                        format!("Opened {} URL(s) via xdg-open.", urls.len()),
                        None,
                        "xdg-open".to_owned(),
                    ));
                }
                // launch_path > name. Both go through the same direct-exec path
                // (so XDG `Exec=` commands round-trip), but launch_path is the
                // canonical form preferred by list_apps callers.
                let command = launch_path_opt.as_deref().or(name_opt.as_deref());
                if let Some(cmd) = command {
                    match spawn_launch_command(cmd, &additional_arguments, agent_seat_lane) {
                        Ok(pid) => {
                            return Ok((
                                format!("✅ Launched {cmd} (pid {pid}) in background."),
                                Some(pid),
                                cmd.to_owned(),
                            ));
                        }
                        Err(_) => {
                            // Not an executable on PATH. Resolve the name against
                            // installed XDG .desktop applications — the same
                            // source list_apps reads — and run the match's Exec=.
                            let installed = crate::installed_apps::list_installed_apps();
                            if let Some(app) = match_installed_app(&installed, cmd) {
                                let pid = spawn_launch_command(
                                    &app.launch_path,
                                    &additional_arguments,
                                    agent_seat_lane,
                                )
                                .map_err(|e| {
                                    anyhow::anyhow!(
                                        "'{cmd}' matched installed app '{}' but its launcher \
                                         `{}` failed to start: {e}",
                                        app.name,
                                        app.launch_path
                                    )
                                })?;
                                return Ok((
                                    format!(
                                        "✅ Launched {} (`{}`, pid {pid}) in background — \
                                         resolved '{cmd}' via its desktop entry.",
                                        app.name, app.launch_path
                                    ),
                                    Some(pid),
                                    format!("{}\u{0}{cmd}\u{0}{}", app.name, app.launch_path),
                                ));
                            }
                            // xdg-open handles URLs and file paths, not app
                            // names — only fall through for something it can
                            // plausibly open, and surface its fast non-zero
                            // exit instead of reporting a launch that never
                            // happened.
                            if !cmd.contains("://") && !std::path::Path::new(cmd).exists() {
                                anyhow::bail!(
                                    "'{cmd}' is not an executable on PATH and matches no \
                                     installed .desktop application. Call list_apps and \
                                     round-trip its launch_path."
                                );
                            }
                            let child = std::process::Command::new("xdg-open").arg(cmd).spawn()?;
                            if let Some(reason) = xdg_open_failure(child) {
                                anyhow::bail!("could not open '{cmd}': {reason}");
                            }
                            // xdg-open may spawn a helper and exit, so do not
                            // claim its pid is the app pid.
                            return Ok((
                                format!("Opened '{cmd}' via xdg-open."),
                                None,
                                cmd.to_owned(),
                            ));
                        }
                    }
                }
                unreachable!()
            },
        )
        .await;

        match result {
            Ok(Ok((message, pid_opt, name))) => {
                if let Some(launcher_pid) = pid_opt {
                    // `name` may carry NUL-separated match keys (display name,
                    // query, Exec=) for a desktop-entry launch.
                    let query = name.clone();
                    let name = name.split('\u{0}').next().unwrap_or("").to_owned();
                    let resolved = tokio::task::spawn_blocking(move || {
                        resolve_launched_windows(launcher_pid, &query, &windows_before)
                    })
                    .await
                    .unwrap_or_else(|_| LaunchedWindows::default());
                    let windows: Vec<Value> =
                        resolved.windows.iter().map(window_record_json).collect();
                    let mut text = message;
                    let mut structured = json!({
                        "pid": resolved.pid,
                        "bundle_id": Value::Null,
                        "name": name,
                        "running": resolved.pid.is_some(),
                        "active": false,
                        "windows": windows,
                        "launcher_pid": launcher_pid,
                    });
                    if let Some(lane) = agent_seat_lane {
                        // Chromium and Electron move themselves into their own
                        // systemd scope after startup, so the scope cannot be read
                        // back here; remember the lane for this process instead, and
                        // for the window's owner when a wrapper handed off.
                        let lane_index = usize::from(lane) - 1;
                        if let Some(pid) = resolved.pid {
                            crate::wayland::hyprland_input::remember_agent_seat_launch(
                                pid, lane_index,
                            );
                        }
                        let handed_off = resolved.handed_off;
                        structured["agent_seat"] = json!({"lane": lane});
                        text.push_str(&format!(
                            " Started on agent seat lane {lane}: it takes background input; \
                             the person's own mouse and keyboard do not reach it while it runs."
                        ));
                        if handed_off {
                            text.push_str(
                                " Warning: the launcher handed the request to another process. If \
                                 that process was already running it is on the user's seat and \
                                 its actions refuse with client_not_agent_seat_bound: quit it, or \
                                 start the app with a separate profile, and launch it again.",
                            );
                        }
                    }
                    if resolved.handed_off {
                        match resolved.pid {
                            Some(pid) => {
                                structured["handoff"] = json!("dbus_activation");
                                text.push_str(&format!(
                                    " The launcher handed the request to another process \
                                     (a wrapper or D-Bus activation); the window belongs to pid {pid}."
                                ));
                            }
                            None => {
                                // The launcher exited (D-Bus activation or a failed
                                // start) and the hand-off budget in
                                // `resolve_launched_windows` passed with no new
                                // window ever appearing. This is not a success: we
                                // have no pid, no window, and no way to know the app
                                // actually started. Report a typed refusal instead
                                // of a `pid: null` shape that reads as success.
                                return launch_handoff_timeout_result(
                                    &name,
                                    launcher_pid,
                                    LAUNCH_HANDOFF_BUDGET.as_secs(),
                                );
                            }
                        }
                    }
                    ToolResult::text(text).with_structured(structured)
                } else {
                    ToolResult::text(message).with_structured(json!({
                        "pid": Value::Null,
                        "bundle_id": Value::Null,
                        "name": name,
                        "running": Value::Null,
                        "active": false,
                        "windows": [],
                    }))
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Failed to launch: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// The agent seat lane (1 or 2) a launch asks for: `agent_seat: true`, with
/// `agent_seat_lane` choosing the lane (default 1). `Err` is the refusal text.
/// The seat needs Hyprland with the Cua plugin; elsewhere the request would
/// start an app nothing can drive.
fn agent_seat_request(args: &Value) -> Result<Option<u8>, String> {
    let requested = args
        .get("agent_seat")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let lane = args.get("agent_seat_lane");
    if !requested {
        return match lane {
            Some(_) => Err("agent_seat_lane only applies with agent_seat:true.".to_owned()),
            None => Ok(None),
        };
    }
    let lane = match lane {
        None => 1,
        Some(value) => match value.as_u64() {
            Some(lane @ (1 | 2)) => lane as u8,
            _ => return Err("agent_seat_lane must be 1 or 2.".to_owned()),
        },
    };
    if !crate::wayland::hyprland_input::enabled() {
        return Err(
            "agent_seat needs Hyprland with the Cua plugin loaded and input enabled \
             (see `hyprctl -j cua:status`); this session has no agent seats."
                .to_owned(),
        );
    }
    Ok(Some(lane))
}

/// Whether `systemd-run` is on PATH.
pub(super) fn systemd_run_available() -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join("systemd-run").is_file())
    })
}

/// The `launch_app` D-Bus-activation hand-off timed out: the launcher exited
/// (handing the request to a running service) but no new window of the app
/// ever appeared within the deadline. This has no pid and no confirmed
/// window, so it is a refusal (`code: "launch_handoff_timeout"`), never a
/// `pid: null` shape that a caller could mistake for success.
fn launch_handoff_timeout_result(name: &str, launcher_pid: u32, waited_secs: u64) -> ToolResult {
    ToolResult::error(format!(
        "'{name}' was handed off for D-Bus activation but no new window appeared within \
         {waited_secs}s; the launch could not be confirmed. Call list_windows to check \
         whether it started anyway, or retry."
    ))
    .with_structured(json!({
        "code": "launch_handoff_timeout",
        "effect": "refused",
        "handoff": "dbus_activation",
        "name": name,
        "launcher_pid": launcher_pid,
        "pid": Value::Null,
        "running": Value::Null,
        "waited_secs": waited_secs,
    }))
}

/// What `launch_app` could attribute to the launch after the spawn.
#[derive(Default)]
struct LaunchedWindows {
    /// The process that owns the app's window: the launcher itself, or the
    /// wrapper target / running service it handed off to.
    pid: Option<u32>,
    windows: Vec<crate::x11::WindowInfo>,
    /// The window belongs to another process than the launcher, or the
    /// launcher exited without one.
    handed_off: bool,
}

/// Whether the launched process is gone or a zombie (already reaped or being
/// reaped by the launch thread).
fn process_exited(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state == "Z" || state == "X"),
        Err(_) => true,
    }
}

/// Case-insensitive identity match between a launch query
/// (`nautilus`, `org.gnome.Nautilus`, `gnome-control-center`, a display
/// name) and a window's WM_CLASS / app id.
fn window_matches_launch(window: &crate::x11::WindowInfo, query: &str) -> bool {
    query
        .split('\u{0}')
        .filter(|key| !key.trim().is_empty())
        .any(|key| window_matches_launch_key(window, key))
}

fn window_matches_launch_key(window: &crate::x11::WindowInfo, query: &str) -> bool {
    let class = window.app_name.to_ascii_lowercase();
    if class.is_empty() {
        return false;
    }
    let query = query.to_ascii_lowercase();
    let stem = query
        .rsplit('/')
        .next()
        .unwrap_or(&query)
        .trim_end_matches(".desktop")
        .to_owned();
    let last = stem.rsplit('.').next().unwrap_or(&stem).to_owned();
    let class_last = class.rsplit('.').next().unwrap_or(&class).to_owned();
    class == stem
        || class_last == last
        || class.contains(&last)
        || last.contains(&class_last)
        || stem
            .split_whitespace()
            .next()
            .is_some_and(|word| class.contains(word))
}

/// How long a launch may take to show a window while the launcher process is
/// still alive (the app is starting; LibreOffice on a small VM needs well over
/// ten seconds), and once it has exited (a wrapper or D-Bus activation has
/// taken over, so the window is either imminent or never coming).
const LAUNCH_STARTING_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
const LAUNCH_HANDOFF_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);

/// Resolve the window(s) of a launch. Prefers the launcher's own window;
/// otherwise adopts a newly mapped top-level whose WM_CLASS matches the
/// launch, whoever owns it: a wrapper script that exec'd the real binary
/// (`libreoffice` → `soffice.bin`), or the running D-Bus service GNOME apps
/// hand their request to.
fn resolve_launched_windows(
    launcher_pid: u32,
    query: &str,
    before: &std::collections::HashSet<u64>,
) -> LaunchedWindows {
    let started = std::time::Instant::now();
    loop {
        let all = crate::wayland::list_windows_dispatch(None);
        if let Some(launched) = settle_launch(&all, launcher_pid, query, before, |w| {
            crate::x11::window_is_splash(w.xid)
        }) {
            return launched;
        }
        let launcher_exited = process_exited(launcher_pid);
        let budget = if launcher_exited {
            LAUNCH_HANDOFF_BUDGET
        } else {
            LAUNCH_STARTING_BUDGET
        };
        if started.elapsed() >= budget {
            return LaunchedWindows {
                pid: (!launcher_exited).then_some(launcher_pid),
                windows: Vec::new(),
                handed_off: launcher_exited,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn settle_launch(
    all: &[crate::x11::WindowInfo],
    launcher_pid: u32,
    query: &str,
    before: &std::collections::HashSet<u64>,
    is_splash: impl Fn(&crate::x11::WindowInfo) -> bool,
) -> Option<LaunchedWindows> {
    let own: Vec<_> = all
        .iter()
        .filter(|w| w.pid == Some(launcher_pid) && !is_splash(w))
        .cloned()
        .collect();
    if !own.is_empty() {
        return Some(LaunchedWindows {
            pid: Some(launcher_pid),
            windows: own,
            handed_off: false,
        });
    }
    let fresh: Vec<_> = all
        .iter()
        .filter(|w| !before.contains(&w.xid) && w.pid.is_some())
        .filter(|w| window_matches_launch(w, query) && !is_splash(w))
        .cloned()
        .collect();
    let pid = fresh.first()?.pid?;
    Some(LaunchedWindows {
        pid: Some(pid),
        windows: fresh.into_iter().filter(|w| w.pid == Some(pid)).collect(),
        handed_off: true,
    })
}

#[cfg(test)]
mod launch_resolution_tests {
    use super::*;

    fn window(class: &str) -> crate::x11::WindowInfo {
        crate::x11::WindowInfo {
            xid: 1,
            pid: Some(7),
            app_name: class.into(),
            title: "t".into(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn launch_queries_match_window_classes() {
        assert!(window_matches_launch(
            &window("org.gnome.Nautilus"),
            "nautilus"
        ));
        assert!(window_matches_launch(
            &window("org.gnome.Nautilus"),
            "org.gnome.Nautilus"
        ));
        assert!(window_matches_launch(
            &window("Gnome-terminal"),
            "gnome-terminal"
        ));
        assert!(window_matches_launch(
            &window("Gnome-control-center"),
            "gnome-control-center"
        ));
        assert!(window_matches_launch(
            &window("Org.gnome.Nautilus"),
            "org.gnome.Nautilus.desktop"
        ));
        assert!(window_matches_launch(
            &window("Org.gnome.Nautilus"),
            "Files\u{0}org.gnome.Nautilus\u{0}nautilus --new-window"
        ));
        assert!(window_matches_launch(
            &window("Gnome-control-center"),
            "Settings\u{0}Settings\u{0}gnome-control-center"
        ));
        assert!(!window_matches_launch(&window("Gedit"), "nautilus"));
        assert!(!window_matches_launch(&window(""), "nautilus"));
    }

    fn owned(xid: u64, pid: u32, class: &str) -> crate::x11::WindowInfo {
        crate::x11::WindowInfo {
            xid,
            pid: Some(pid),
            ..window(class)
        }
    }

    #[test]
    fn a_launcher_splash_waits_for_the_app_window() {
        let splash = owned(10, 377, "libreoffice");
        let calc = owned(20, 398, "libreoffice-calc");
        let none = std::collections::HashSet::new();
        let is_splash = |w: &crate::x11::WindowInfo| w.xid == 10;

        let splash_only = std::slice::from_ref(&splash);
        assert!(settle_launch(splash_only, 377, "libreoffice", &none, is_splash).is_none());

        let launched =
            settle_launch(&[splash, calc], 377, "libreoffice", &none, is_splash).unwrap();
        assert_eq!(launched.pid, Some(398));
        assert_eq!(
            launched.windows.iter().map(|w| w.xid).collect::<Vec<_>>(),
            [20]
        );

        let gimp = owned(30, 377, "gimp");
        let launched = settle_launch(&[gimp], 377, "gimp", &none, is_splash).unwrap();
        assert_eq!((launched.pid, launched.handed_off), (Some(377), false));
    }

    #[test]
    fn a_missing_process_counts_as_exited() {
        assert!(process_exited(u32::MAX - 1));
        assert!(!process_exited(std::process::id()));
    }

    #[test]
    fn a_handoff_timeout_is_a_refusal_not_a_null_pid_success() {
        let result = launch_handoff_timeout_result("Nautilus", 4242, 8);
        assert_eq!(
            result.is_error,
            Some(true),
            "a hand-off timeout must be reported as an error, not success"
        );
        let s = result.structured_content.unwrap();
        assert_eq!(s["code"], "launch_handoff_timeout");
        assert_eq!(s["effect"], "refused");
        assert_eq!(s["pid"], Value::Null);
        assert_eq!(s["running"], Value::Null);
        assert_eq!(s["launcher_pid"], 4242);
        assert_eq!(s["waited_secs"], 8);
    }
}
