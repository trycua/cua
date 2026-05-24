//! `cua-driver doctor` — environment + install diagnostic probes.
//!
//! The doctor subcommand runs a battery of platform-aware probes and emits
//! a structured report (plain text by default, JSON via `--json`). Each probe
//! produces one line tagged `[ok]`, `[warn]`, or `[err]` so the output is
//! grep-friendly without losing detail.
//!
//! Exit code: `0` when every probe is `[ok]` or `[warn]`. Non-zero only when
//! at least one `[err]` probe failed — e.g. the binary cannot read its own
//! install dir. Warnings (e.g. running outside an interactive desktop on
//! Windows) do not fail because they are sometimes the expected state (CI
//! invocations of `doctor` to render the report).
//!
//! ## Probe categories
//!
//! - **Cross-platform**: version + arch, install layout, home dir,
//!   telemetry state.
//! - **Windows**: interactive desktop session detection (Session 0 warning),
//!   UI Automation COM availability, top-level window enumeration count.
//! - **Linux**: `DISPLAY` / `WAYLAND_DISPLAY` presence, X11 connection
//!   reachability, AT-SPI bus availability hint.
//! - **macOS**: existing legacy-cleanup steps (LaunchAgent plist + update
//!   script), plus a hint to run `cua-driver diagnose` for a full
//!   TCC / cdhash / install layout dump.

use std::path::PathBuf;

/// Probe outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Err,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Err => "err",
        }
    }
}

/// One probe result.
#[derive(Debug, Clone)]
pub struct Probe {
    /// Short stable label (e.g. `"binary"`, `"home dir"`).
    pub label: String,
    pub status: Status,
    /// One-line summary printed next to the tag.
    pub message: String,
    /// Optional multi-line detail (continuation lines indented under the
    /// summary in text mode, separate JSON field in `--json` mode).
    pub detail: Option<String>,
}

impl Probe {
    pub fn ok(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self { label: label.into(), status: Status::Ok, message: message.into(), detail: None }
    }
    pub fn warn(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self { label: label.into(), status: Status::Warn, message: message.into(), detail: None }
    }
    pub fn err(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self { label: label.into(), status: Status::Err, message: message.into(), detail: None }
    }
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Aggregated probe results for one `doctor` run.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub probes: Vec<Probe>,
}

impl Report {
    pub fn push(&mut self, probe: Probe) {
        self.probes.push(probe);
    }

    /// True iff at least one probe is `Status::Err`. Drives the process
    /// exit code — warnings never fail the run.
    pub fn has_errors(&self) -> bool {
        self.probes.iter().any(|p| p.status == Status::Err)
    }

    /// Plain-text rendering. Each probe is one summary line; multi-line
    /// `detail` blocks are indented underneath.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for probe in &self.probes {
            out.push_str(&format!(
                "[{tag:<4}] {label}: {msg}\n",
                tag = probe.status.tag(),
                label = probe.label,
                msg = probe.message,
            ));
            if let Some(detail) = &probe.detail {
                for line in detail.lines() {
                    out.push_str("         ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// JSON rendering: `{ "probes": [...], "ok": bool }`. Each probe is
    /// `{ "label", "status", "message", "detail" }`.
    pub fn to_json(&self) -> serde_json::Value {
        let probes: Vec<serde_json::Value> = self
            .probes
            .iter()
            .map(|p| {
                let mut obj = serde_json::Map::new();
                obj.insert("label".into(), serde_json::Value::String(p.label.clone()));
                obj.insert("status".into(), serde_json::Value::String(p.status.tag().into()));
                obj.insert("message".into(), serde_json::Value::String(p.message.clone()));
                if let Some(d) = &p.detail {
                    obj.insert("detail".into(), serde_json::Value::String(d.clone()));
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        serde_json::json!({
            "ok": !self.has_errors(),
            "probes": probes,
        })
    }
}

// ── Cross-platform probes ─────────────────────────────────────────────────

/// Probe: version + target triple — the same string `cua-driver --version`
/// returns, plus the build-time target so the user can sanity-check arch.
fn probe_version() -> Probe {
    let version = env!("CARGO_PKG_VERSION");
    let target = build_target_triple();
    Probe::ok("binary", format!("cua-driver {version} ({target})"))
}

/// Build-time target triple. We don't have a `built` crate dependency, so
/// we synthesise `<arch>-<os>` which is enough to disambiguate the host
/// without adding a new transitive dep.
fn build_target_triple() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// Probe: where the binary lives on disk. Resolves symlinks (e.g. the
/// `~/.local/bin/cua-driver -> packages/current/cua-driver` chain) so the
/// user sees the actual versioned release dir.
fn probe_install_layout() -> Probe {
    let exe = std::env::current_exe();
    match exe {
        Err(e) => Probe::err("install dir", format!("could not resolve current_exe: {e}")),
        Ok(path) => {
            let canonical = std::fs::canonicalize(&path)
                .unwrap_or_else(|_| path.clone());
            let mut detail = String::new();
            if path != canonical {
                detail.push_str(&format!("argv exe:  {}\n", path.display()));
                detail.push_str(&format!("resolved:  {}", canonical.display()));
            } else {
                detail.push_str(&format!("path: {}", canonical.display()));
            }
            Probe::ok("install dir", canonical.display().to_string()).with_detail(detail)
        }
    }
}

/// Probe: package-home directory (`~/.cua-driver`) — counts cached
/// release dirs so the user can sanity-check
/// `CUA_DRIVER_RS_KEEP_VERSIONS` is doing the right thing. Renamed from
/// `.cua-driver-rs/` in v0.2.16 to match the install-path rename (PR #1644).
fn probe_home_dir() -> Probe {
    let home = match home_dir() {
        Some(h) => h,
        None => return Probe::warn("home dir", "neither HOME nor USERPROFILE set"),
    };
    let cua_home = home.join(".cua-driver");
    if !cua_home.exists() {
        return Probe::warn(
            "home dir",
            format!("{} does not exist yet (created on first run)", cua_home.display()),
        );
    }
    let releases = cua_home.join("packages").join("releases");
    // Only count subdirectories — bare files inside `releases/` (stray
    // download artifacts, lock files, .DS_Store) are not actual cached
    // versions and shouldn't inflate the "release dir(s) cached" metric.
    let release_count = std::fs::read_dir(&releases)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.metadata().map(|m| m.is_dir()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    Probe::ok(
        "home dir",
        format!(
            "{} ({release_count} release dir{} cached)",
            cua_home.display(),
            if release_count == 1 { "" } else { "s" },
        ),
    )
}

/// Probe: telemetry state — env-var opt-out + install-id file presence.
/// Reports without reading the UUID itself (privacy-preserving — only
/// presence, not value).
fn probe_telemetry() -> Probe {
    let env_disabled = std::env::var("CUA_DRIVER_RS_TELEMETRY_ENABLED")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(false);
    let install_id_present = home_dir()
        .map(|h| h.join(".cua-driver").join(".telemetry_id").exists())
        .unwrap_or(false);
    if env_disabled {
        Probe::ok(
            "telemetry",
            "disabled via CUA_DRIVER_RS_TELEMETRY_ENABLED",
        )
    } else {
        let id_state = if install_id_present { "install-id present" } else { "install-id not yet generated" };
        Probe::ok("telemetry", format!("enabled ({id_state})"))
    }
}

/// `$HOME` on Unix, `%USERPROFILE%` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

// ── Platform-specific probe entry-points ──────────────────────────────────

/// Append every platform-specific probe to the report.
#[cfg(target_os = "windows")]
fn append_platform_probes(report: &mut Report) {
    use platform_windows::diagnostics as diag;

    // Interactive desktop session probe — the critical Windows check. SSH
    // (and most service contexts) land processes in Session 0 with no
    // attached WindowStation+Desktop, which silently breaks every
    // window-driving tool. Surface the misconfiguration directly so users
    // don't waste hours debugging tools that are working as designed.
    let in_session_0 = match diag::current_session_id() {
        Some(0) => {
            report.push(
                Probe::warn(
                    "interactive session",
                    "running in Session 0 (services); window-driving tools (list_windows, click, type_text, screenshot, get_window_state) will return empty results — these APIs need an attached interactive desktop.",
                )
                .with_detail(
                    "re-run cua-driver from an interactive logon (RDP, console, or a scheduled task in the user's session) for the GUI tools to function.",
                ),
            );
            true
        }
        Some(sid) => {
            match diag::interactive_desktop_check() {
                Ok(true) => report.push(Probe::ok(
                    "interactive session",
                    format!(
                        "session {sid} has an attached interactive desktop (WinSta0 + foreground window)"
                    ),
                )),
                Ok(false) => report.push(
                    Probe::warn(
                        "interactive session",
                        format!(
                            "session {sid}: WinSta0 reachable but no foreground window — desktop may be locked or no app is in the foreground"
                        ),
                    ),
                ),
                Err(e) => report.push(Probe::warn(
                    "interactive session",
                    format!("session {sid} desktop probe failed: {e}"),
                )),
            }
            false
        }
        None => {
            report.push(Probe::warn(
                "interactive session",
                "ProcessIdToSessionId failed — cannot determine session id",
            ));
            false
        }
    };

    // COM / UI Automation availability.
    match diag::ui_automation_available() {
        Ok(()) => report.push(Probe::ok(
            "UI Automation",
            "CoCreateInstance(CUIAutomation) succeeded",
        )),
        Err(e) => report.push(Probe::err(
            "UI Automation",
            format!("CoCreateInstance(CUIAutomation) failed: {e}"),
        )),
    }

    // EnumWindows count — cross-check the session probe. When Session 0
    // is in play, this almost always reports zero visible windows, which
    // reinforces the warning above instead of looking like a separate bug.
    let visible = platform_windows::win32::list_windows(None).len();
    let probe = Probe::ok(
        "EnumWindows visible",
        format!("{visible} window{}", if visible == 1 { "" } else { "s" }),
    );
    let probe = if visible == 0 && in_session_0 {
        probe.with_detail(
            "consistent with the Session 0 warning above — EnumWindows is scoped to the calling session's desktop.",
        )
    } else {
        probe
    };
    report.push(probe);
}

/// Run `gdbus introspect` against the AT-SPI accessibility bus and report
/// whether it returned success within `timeout`. Any of: spawn failure,
/// timeout elapsed, non-zero exit — all collapse to `false` (the caller
/// only cares about reachability, not the failure mode).
///
/// Kept separate from `append_platform_probes` so it's straightforward to
/// unit-test the timeout path without invoking the full doctor run.
#[cfg(target_os = "linux")]
fn probe_at_spi_bus_via_gdbus(timeout: std::time::Duration) -> bool {
    use std::process::{Command, Stdio};
    use wait_timeout::ChildExt;

    let mut child = match Command::new("gdbus")
        .args([
            "introspect",
            "--session",
            "--dest",
            "org.a11y.Bus",
            "--object-path",
            "/org/a11y/bus",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match child.wait_timeout(timeout) {
        Ok(Some(status)) => status.success(),
        Ok(None) => {
            // Timed out — kill the stuck child so we don't leave a
            // gdbus process hanging around after `doctor` exits.
            let _ = child.kill();
            let _ = child.wait();
            false
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn append_platform_probes(report: &mut Report) {
    // Display server probe. Order matters: Wayland wins when both are set
    // (XWayland leaves DISPLAY pointing at the X server XWayland exposes,
    // but the actual session is still Wayland).
    let display = std::env::var("DISPLAY").ok().filter(|v| !v.is_empty());
    let wayland = std::env::var("WAYLAND_DISPLAY").ok().filter(|v| !v.is_empty());
    match (display.as_deref(), wayland.as_deref()) {
        (None, None) => report.push(
            Probe::warn(
                "display server",
                "neither DISPLAY nor WAYLAND_DISPLAY set — window-driving tools will fail",
            )
            .with_detail(
                "run from an interactive desktop session (X11 / Wayland with XWayland) or set DISPLAY explicitly.",
            ),
        ),
        (Some(d), None) => report.push(Probe::ok(
            "display server",
            format!("X11 (DISPLAY={d})"),
        )),
        (None, Some(w)) => report.push(
            Probe::warn(
                "display server",
                format!("Wayland only (WAYLAND_DISPLAY={w}, DISPLAY unset)"),
            )
            .with_detail(
                "X11 tools (list_windows, screenshot) need XWayland — start your session with XWayland enabled.",
            ),
        ),
        (Some(d), Some(w)) => report.push(Probe::ok(
            "display server",
            format!("Wayland+XWayland (WAYLAND_DISPLAY={w}, DISPLAY={d})"),
        )),
    }

    // X11 window enumeration probe. An empty result could mean either an
    // unreachable display or a healthy display with no top-level windows
    // open — `list_windows` doesn't distinguish the two — so the warning
    // hedges instead of asserting a connection failure.
    match platform_linux::x11::list_windows(None) {
        v if v.is_empty() => report.push(
            Probe::warn(
                "X11 connection",
                "no top-level windows returned (possible disconnected or inaccessible X11 display)",
            ),
        ),
        v => report.push(Probe::ok(
            "X11 connection",
            format!("connected, {} visible top-level window{}", v.len(), if v.len() == 1 { "" } else { "s" }),
        )),
    }

    // AT-SPI bus probe. We don't link D-Bus directly — instead, check for
    // the AT_SPI_BUS env var which the at-spi daemon advertises when
    // running, and fall back to checking gdbus's view of the
    // org.a11y.Bus name.
    let at_spi_env = std::env::var("AT_SPI_BUS").ok().filter(|v| !v.is_empty());
    match at_spi_env {
        Some(addr) => report.push(Probe::ok("AT-SPI", format!("bus address present (AT_SPI_BUS={addr})"))),
        None => {
            // Bounded wait — a hung session bus daemon would otherwise
            // block `doctor` indefinitely. 3s is enough for a healthy
            // gdbus introspect to complete (single round-trip on the
            // session bus) and short enough that a stuck bus surfaces
            // as a warning instead of looking like the binary froze.
            let bus_ok = probe_at_spi_bus_via_gdbus(std::time::Duration::from_secs(3));
            if bus_ok {
                report.push(Probe::ok("AT-SPI", "org.a11y.Bus reachable via session bus"));
            } else {
                report.push(
                    Probe::warn("AT-SPI", "accessibility bus not reachable")
                        .with_detail(
                            "install at-spi2-core and ensure the user session has D-Bus running for get_window_state to work.",
                        ),
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn append_platform_probes(report: &mut Report) {
    // The legacy cleanup behavior — preserved as opportunistic probes so
    // existing users on stale installs still get the cleanup, but the
    // output is now structured.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let legacy_plist = format!("{home}/Library/LaunchAgents/com.trycua.cua_driver_updater.plist");
    let legacy_script = "/usr/local/bin/cua-driver-update";

    if std::path::Path::new(&legacy_plist).exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &legacy_plist])
            .status();
        match std::fs::remove_file(&legacy_plist) {
            Ok(()) => report.push(Probe::ok(
                "legacy LaunchAgent",
                format!("removed stale {legacy_plist}"),
            )),
            Err(e) => report.push(Probe::warn(
                "legacy LaunchAgent",
                format!("could not remove {legacy_plist}: {e}"),
            )),
        }
    } else {
        report.push(Probe::ok("legacy LaunchAgent", "not present"));
    }

    if std::path::Path::new(legacy_script).exists() {
        match std::fs::remove_file(legacy_script) {
            Ok(()) => report.push(Probe::ok(
                "legacy update script",
                format!("removed stale {legacy_script}"),
            )),
            Err(_) => report.push(
                Probe::warn(
                    "legacy update script",
                    format!("{legacy_script} present and root-owned — remove with `sudo rm -f {legacy_script}`"),
                ),
            ),
        }
    } else {
        report.push(Probe::ok("legacy update script", "not present"));
    }

    report.push(Probe::ok(
        "TCC + cdhash report",
        "for a full bundle / signature / TCC dump, run `cua-driver diagnose`",
    ));
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn append_platform_probes(report: &mut Report) {
    report.push(Probe::warn(
        "platform",
        format!("no platform-specific probes implemented for {}", std::env::consts::OS),
    ));
}

// ── Public entry point ────────────────────────────────────────────────────

/// Run every probe and return the aggregated report.
pub fn run() -> Report {
    let mut report = Report::default();
    report.push(probe_version());
    report.push(probe_install_layout());
    report.push(probe_home_dir());
    report.push(probe_telemetry());
    append_platform_probes(&mut report);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_to_text_renders_status_tag_and_message() {
        let mut r = Report::default();
        r.push(Probe::ok("a", "alpha"));
        r.push(Probe::warn("b", "bravo").with_detail("two\nlines"));
        r.push(Probe::err("c", "charlie"));
        let text = r.to_text();
        assert!(text.contains("[ok  ] a: alpha"));
        assert!(text.contains("[warn] b: bravo"));
        assert!(text.contains("         two"));
        assert!(text.contains("         lines"));
        assert!(text.contains("[err ] c: charlie"));
    }

    #[test]
    fn report_to_json_marks_ok_false_when_any_error() {
        let mut r = Report::default();
        r.push(Probe::ok("a", "alpha"));
        r.push(Probe::err("b", "bravo"));
        let json = r.to_json();
        assert_eq!(json["ok"], serde_json::Value::Bool(false));
        assert_eq!(json["probes"].as_array().unwrap().len(), 2);
        assert_eq!(json["probes"][1]["status"], "err");
    }

    #[test]
    fn report_to_json_marks_ok_true_when_only_warnings() {
        let mut r = Report::default();
        r.push(Probe::ok("a", "alpha"));
        r.push(Probe::warn("b", "bravo"));
        let json = r.to_json();
        // Warnings do not fail the run — exit-code-driving flag stays true.
        assert_eq!(json["ok"], serde_json::Value::Bool(true));
    }

    #[test]
    fn cross_platform_probes_always_emit_something() {
        // Smoke test: run the cross-platform probes and confirm they all
        // produced a probe (no silent dropouts).
        let v = probe_version();
        assert_eq!(v.label, "binary");
        let i = probe_install_layout();
        assert_eq!(i.label, "install dir");
        let h = probe_home_dir();
        assert_eq!(h.label, "home dir");
        let t = probe_telemetry();
        assert_eq!(t.label, "telemetry");
    }

    #[test]
    fn run_emits_at_least_cross_platform_probes() {
        let report = run();
        // 4 cross-platform + at least 1 platform-specific.
        assert!(report.probes.len() >= 5, "got {}", report.probes.len());
    }
}
