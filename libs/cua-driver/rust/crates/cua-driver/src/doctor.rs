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
        Self {
            label: label.into(),
            status: Status::Ok,
            message: message.into(),
            detail: None,
        }
    }
    pub fn warn(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Warn,
            message: message.into(),
            detail: None,
        }
    }
    pub fn err(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Err,
            message: message.into(),
            detail: None,
        }
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
                obj.insert(
                    "status".into(),
                    serde_json::Value::String(p.status.tag().into()),
                );
                obj.insert(
                    "message".into(),
                    serde_json::Value::String(p.message.clone()),
                );
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
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
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
    let cua_home = home.join(crate::bundle::user_home_subdirectory());
    if !cua_home.exists() {
        return Probe::warn(
            "home dir",
            format!(
                "{} does not exist yet (created on first run)",
                cua_home.display()
            ),
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

/// Probe the same effective persisted/environment state as `telemetry status`.
fn probe_telemetry() -> Probe {
    let status = crate::telemetry::status();
    if status.enabled {
        let identity = if status.installation_id_present {
            "install-id present"
        } else {
            "install-id not yet generated"
        };
        Probe::ok(
            "telemetry",
            format!("enabled via {} ({identity})", status.source),
        )
    } else {
        Probe::ok(
            "telemetry",
            format!("disabled via {} (installation ID retained)", status.source),
        )
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
    let desktop = diag::desktop_state();
    let in_session_0 = match desktop.session_id {
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
            if desktop.has_foreground_window() {
                report.push(Probe::ok(
                    "interactive session",
                    format!(
                        "session {sid} has an attached interactive desktop ({})",
                        desktop.summary()
                    ),
                ));
            } else if desktop.input_desktop_is_default() {
                report.push(
                    Probe::warn(
                        "interactive session",
                        format!(
                            "session {sid}: Default input desktop is reachable but no window is foreground ({})",
                            desktop.summary()
                        ),
                    )
                    .with_detail(
                        "GUI tests can seed foreground by launching their focus sentinel; unattended runners should still validate that the sentinel becomes foreground.",
                    ),
                );
            } else {
                report.push(
                    Probe::warn(
                        "interactive session",
                        format!(
                            "session {sid}: input desktop is not the user Default desktop ({})",
                            desktop.summary()
                        ),
                    )
                    .with_detail(
                        "this usually means the RDP/console session is locked or disconnected; reconnect, use tscon-to-console, or boot the disposable GUI VM with an unlocked console session.",
                    ),
                );
            }
            false
        }
        None => {
            report.push(Probe::warn(
                "interactive session",
                format!(
                    "ProcessIdToSessionId failed — cannot determine session id ({})",
                    desktop.summary()
                ),
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
    match wait_for_child(&mut child, timeout) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            // Timed out or failed — kill the stuck child so we don't leave a
            // gdbus process hanging around after `doctor` exits.
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn wait_for_child(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }

        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Ok(None);
        };
        if remaining.is_zero() {
            return Ok(None);
        }
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(15)));
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct KwinDoctorFacts<'a> {
    installed: bool,
    protocol_supported: bool,
    owner_trusted: bool,
    enumeration_available: bool,
    activation_verified: bool,
    fully_available: bool,
    message: &'a str,
}

#[cfg(target_os = "linux")]
fn kwin_remediation(message: &str) -> String {
    format!(
        "{message}\nRun: {}",
        platform_linux::wayland::kwin_adapter::INSTALL_COMMAND
    )
}

#[cfg(target_os = "linux")]
fn probe_kwin_portal_for_doctor() -> Result<bool, String> {
    // `doctor` is invoked from the async CLI runtime, while the platform
    // probe intentionally owns a small current-thread runtime. Keep the two
    // runtimes on different OS threads so this synchronous report cannot
    // panic with Tokio's nested-runtime guard.
    std::thread::spawn(platform_linux::health_report::probe_portal_remote_desktop)
        .join()
        .map_err(|_| "xdg-desktop-portal RemoteDesktop probe panicked".to_owned())?
        .map_err(|error| error.to_string())
}

/// Run the trusted KWin adapter diagnosis for `doctor`.
///
/// The adapter talks to KWin over zbus's blocking API, which (with the `tokio`
/// backend) requires an ambient Tokio runtime for its background tasks yet
/// panics if its own `block_on` runs on a runtime *worker* thread. `doctor`
/// runs on a plain synchronous thread with no runtime at all. Own a private
/// multi-thread runtime and run the blocking probe on one of its blocking
/// threads — the same context the async tool dispatch uses in the daemon — on a
/// dedicated OS thread so this stays correct even if `doctor` is ever invoked
/// from within another runtime.
#[cfg(target_os = "linux")]
fn diagnose_kwin_for_doctor() -> platform_linux::wayland::kwin_adapter::AdapterDiagnostic {
    std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build KWin doctor probe runtime");
        runtime.block_on(async {
            tokio::task::spawn_blocking(platform_linux::wayland::kwin_adapter::diagnose)
                .await
                .expect("KWin diagnosis task panicked")
        })
    })
    .join()
    .unwrap_or_else(
        |_| platform_linux::wayland::kwin_adapter::AdapterDiagnostic {
            message: "kwin_adapter_unavailable: KWin diagnosis thread panicked".to_owned(),
            ..Default::default()
        },
    )
}

#[cfg(target_os = "linux")]
fn append_kwin_adapter_probes(
    report: &mut Report,
    facts: KwinDoctorFacts<'_>,
    portal_compiled: bool,
    portal_probe: Result<bool, String>,
) {
    let every_required_fact = facts.installed
        && facts.protocol_supported
        && facts.owner_trusted
        && facts.enumeration_available
        && facts.activation_verified;
    let fully_available = facts.fully_available && every_required_fact;

    if facts.installed {
        report.push(Probe::ok(
            "KWin adapter installed",
            "current-user KWin target adapter is installed",
        ));
    } else {
        report.push(
            Probe::warn(
                "KWin adapter installed",
                "current-user KWin target adapter is absent",
            )
            .with_detail(kwin_remediation(facts.message)),
        );
    }

    if facts.protocol_supported {
        report.push(Probe::ok(
            "KWin adapter protocol",
            "adapter protocol version is supported",
        ));
    } else {
        report.push(
            Probe::warn(
                "KWin adapter protocol",
                "adapter protocol is absent, malformed, or unsupported",
            )
            .with_detail(kwin_remediation(facts.message)),
        );
    }

    if facts.owner_trusted {
        report.push(Probe::ok(
            "KWin adapter owner/process",
            "immutable session owner, UID, KWin process, and executable trust checks passed",
        ));
    } else {
        report.push(
            Probe::warn(
                "KWin adapter owner/process",
                "adapter owner, UID, KWin process, or executable trust could not be proven",
            )
            .with_detail(kwin_remediation(facts.message)),
        );
    }

    if facts.enumeration_available {
        report.push(Probe::ok(
            "KWin exact enumeration",
            "authoritative compositor window enumeration succeeded",
        ));
    } else {
        report.push(
            Probe::warn(
                "KWin exact enumeration",
                "authoritative compositor window enumeration is unavailable",
            )
            .with_detail(kwin_remediation(facts.message)),
        );
    }

    if facts.activation_verified {
        report.push(Probe::ok(
            "KWin activation verification",
            "exact active-window identity was verified through a fresh compositor snapshot",
        ));
    } else {
        report.push(
            Probe::warn(
                "KWin activation verification",
                "exact target activation could not be verified through a fresh compositor snapshot",
            )
            .with_detail(kwin_remediation(facts.message)),
        );
    }

    if !portal_compiled {
        report.push(
            Probe::warn(
                "KWin portal/libei input",
                "this build does not include the portal/libei input backend",
            )
            .with_detail(
                "Install a release build or rebuild cua-driver with --features portal-input.",
            ),
        );
        return;
    }

    match portal_probe {
        Err(error) => report.push(
            Probe::warn(
                "KWin portal/libei input",
                "xdg-desktop-portal RemoteDesktop probe failed",
            )
            .with_detail(format!(
                "{error}\nEnsure xdg-desktop-portal and xdg-desktop-portal-kde are running in this user session."
            )),
        ),
        Ok(false) => report.push(
            Probe::warn(
                "KWin portal/libei input",
                "xdg-desktop-portal RemoteDesktop is not reachable",
            )
            .with_detail(
                "Ensure xdg-desktop-portal and xdg-desktop-portal-kde are running in this user session.",
            ),
        ),
        Ok(true) if fully_available => report.push(Probe::ok(
            "KWin portal/libei input",
            "RemoteDesktop is reachable and global input is gated by verified exact KWin activation and restoration",
        )),
        Ok(true) => report.push(
            Probe::warn(
                "KWin portal/libei input",
                "RemoteDesktop is reachable, but global input remains blocked until the full trusted KWin path verifies",
            )
            .with_detail(kwin_remediation(facts.message)),
        ),
    }
}

#[cfg(target_os = "linux")]
fn append_platform_probes(report: &mut Report) {
    // Display server probe. Order matters: Wayland wins when both are set
    // (XWayland leaves DISPLAY pointing at the X server XWayland exposes,
    // but the actual session is still Wayland).
    let display = std::env::var("DISPLAY").ok().filter(|v| !v.is_empty());
    let wayland = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|v| !v.is_empty());
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
        v if v.is_empty() => report.push(Probe::warn(
            "X11 connection",
            "no top-level windows returned (possible disconnected or inaccessible X11 display)",
        )),
        v => report.push(Probe::ok(
            "X11 connection",
            format!(
                "connected, {} visible top-level window{}",
                v.len(),
                if v.len() == 1 { "" } else { "s" }
            ),
        )),
    }

    // AT-SPI bus probe. We don't link D-Bus directly — instead, check for
    // the AT_SPI_BUS env var which the at-spi daemon advertises when
    // running, and fall back to checking gdbus's view of the
    // org.a11y.Bus name.
    let at_spi_env = std::env::var("AT_SPI_BUS").ok().filter(|v| !v.is_empty());
    match at_spi_env {
        Some(addr) => report.push(Probe::ok(
            "AT-SPI",
            format!("bus address present (AT_SPI_BUS={addr})"),
        )),
        None => {
            // Bounded wait — a hung session bus daemon would otherwise
            // block `doctor` indefinitely. 3s is enough for a healthy
            // gdbus introspect to complete (single round-trip on the
            // session bus) and short enough that a stuck bus surfaces
            // as a warning instead of looking like the binary froze.
            let bus_ok = probe_at_spi_bus_via_gdbus(std::time::Duration::from_secs(3));
            if bus_ok {
                report.push(Probe::ok(
                    "AT-SPI",
                    "org.a11y.Bus reachable via session bus",
                ));
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

    // KDE/KWin portal input is global to the compositor's active seat. Surface
    // each trust gate separately so a reachable portal can never be mistaken
    // for safe target-addressable input.
    if platform_linux::wayland::kwin_adapter::is_kde_wayland_session() {
        let diagnosis = diagnose_kwin_for_doctor();
        let portal_compiled = platform_linux::wayland::PORTAL_INPUT_ENABLED;
        let portal_probe = if portal_compiled {
            probe_kwin_portal_for_doctor()
        } else {
            Ok(false)
        };
        append_kwin_adapter_probes(
            report,
            KwinDoctorFacts {
                installed: diagnosis.installed,
                protocol_supported: diagnosis.protocol_supported,
                owner_trusted: diagnosis.owner_trusted,
                enumeration_available: diagnosis.enumeration_available,
                activation_verified: diagnosis.activation_verified,
                fully_available: diagnosis.fully_available(),
                message: &diagnosis.message,
            },
            portal_compiled,
            portal_probe,
        );
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
        format!(
            "no platform-specific probes implemented for {}",
            std::env::consts::OS
        ),
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

    #[cfg(target_os = "linux")]
    #[test]
    fn wait_for_child_reports_exit() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let status = wait_for_child(&mut child, std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wait_for_child_times_out() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let status = wait_for_child(&mut child, std::time::Duration::from_millis(10)).unwrap();
        assert!(status.is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(target_os = "linux")]
    fn kwin_facts(
        installed: bool,
        protocol_supported: bool,
        owner_trusted: bool,
        enumeration_available: bool,
        activation_verified: bool,
    ) -> KwinDoctorFacts<'static> {
        KwinDoctorFacts {
            installed,
            protocol_supported,
            owner_trusted,
            enumeration_available,
            activation_verified,
            fully_available: installed
                && protocol_supported
                && owner_trusted
                && enumeration_available
                && activation_verified,
            message: "fixture KWin diagnosis",
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kwin_doctor_reports_every_trust_gate_and_portal_success() {
        let mut report = Report::default();
        append_kwin_adapter_probes(
            &mut report,
            kwin_facts(true, true, true, true, true),
            true,
            Ok(true),
        );

        assert_eq!(report.probes.len(), 6);
        assert!(report.probes.iter().all(|probe| probe.status == Status::Ok));
        let labels = report
            .probes
            .iter()
            .map(|probe| probe.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "KWin adapter installed",
                "KWin adapter protocol",
                "KWin adapter owner/process",
                "KWin exact enumeration",
                "KWin activation verification",
                "KWin portal/libei input",
            ]
        );
        assert!(report.probes[5].message.contains("verified exact KWin"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kwin_doctor_missing_adapter_is_fail_closed_with_exact_remediation() {
        let mut report = Report::default();
        append_kwin_adapter_probes(
            &mut report,
            kwin_facts(false, false, false, false, false),
            true,
            Ok(true),
        );

        assert_eq!(report.probes.len(), 6);
        assert!(report
            .probes
            .iter()
            .all(|probe| probe.status == Status::Warn));
        assert!(report.probes[5].message.contains("remains blocked"));
        assert!(report.probes.iter().any(|probe| {
            probe
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains(platform_linux::wayland::kwin_adapter::INSTALL_COMMAND)
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kwin_doctor_outdated_or_untrusted_adapter_never_unlocks_portal_input() {
        for facts in [
            kwin_facts(true, false, true, true, true),
            kwin_facts(true, true, false, true, true),
            kwin_facts(true, true, true, false, true),
            kwin_facts(true, true, true, true, false),
        ] {
            let mut report = Report::default();
            append_kwin_adapter_probes(&mut report, facts, true, Ok(true));

            let portal = report.probes.last().expect("portal probe");
            assert_eq!(portal.status, Status::Warn);
            assert!(portal.message.contains("remains blocked"));
            assert!(portal
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains(platform_linux::wayland::kwin_adapter::INSTALL_COMMAND));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kwin_doctor_distinguishes_missing_build_and_missing_portal() {
        let ready = kwin_facts(true, true, true, true, true);

        let mut no_feature = Report::default();
        append_kwin_adapter_probes(&mut no_feature, ready, false, Ok(false));
        assert_eq!(
            no_feature.probes.last().unwrap().message,
            "this build does not include the portal/libei input backend"
        );

        let mut no_portal = Report::default();
        append_kwin_adapter_probes(&mut no_portal, ready, true, Ok(false));
        assert_eq!(
            no_portal.probes.last().unwrap().message,
            "xdg-desktop-portal RemoteDesktop is not reachable"
        );

        let mut probe_error = Report::default();
        append_kwin_adapter_probes(
            &mut probe_error,
            ready,
            true,
            Err("fixture failure".to_owned()),
        );
        assert!(probe_error
            .probes
            .last()
            .unwrap()
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("fixture failure")));
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
