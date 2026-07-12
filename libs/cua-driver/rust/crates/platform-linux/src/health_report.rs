//! Linux `health_report` provider.
//!
//! Implements [`cua_driver_core::health_report::HealthCheckProvider`]
//! for Linux. Reuses the same probes the Linux `check_permissions`
//! tool runs — X11 connectivity for AX (driven via XSendEvent +
//! AT-SPI) and X11 again for the screen capture path.
//!
//! The TCC / bundle checks emit `status: "skip"` with `message: "not
//! applicable on Linux"`; Linux has no TCC.

use async_trait::async_trait;
use cua_driver_core::health_report::{
    CheckData, CheckEntry, HealthCheckProvider, NAME_AX_CAPABILITY, NAME_BINARY_VERSION,
    NAME_BUNDLE_IDENTITY, NAME_PLATFORM_SUPPORTED, NAME_SCREEN_CAPTURE_CAPABILITY,
    NAME_SESSION_ACTIVE, NAME_TCC_ACCESSIBILITY, NAME_TCC_SCREEN_RECORDING,
};

/// Doctor entry that surfaces which wlroots manager globals the running
/// Wayland compositor advertises (foreign-toplevel / screencopy /
/// virtual-pointer / wl_shm). Linux-specific; skipped when not on Wayland.
pub const NAME_WAYLAND_BACKEND: &str = "wayland_backend";

/// Linux canonical check names — same Swift-PR contract, with TCC /
/// bundle entries surfaced as `skip("not applicable on Linux")`. The
/// full set of entries always appears so cross-platform consumers see
/// a complete check map.
pub const LINUX_CHECK_NAMES: &[&str] = &[
    NAME_BINARY_VERSION,
    NAME_PLATFORM_SUPPORTED,
    NAME_SESSION_ACTIVE,
    NAME_BUNDLE_IDENTITY,
    NAME_TCC_ACCESSIBILITY,
    NAME_TCC_SCREEN_RECORDING,
    NAME_AX_CAPABILITY,
    NAME_SCREEN_CAPTURE_CAPABILITY,
    NAME_WAYLAND_BACKEND,
];

pub struct LinuxHealthProvider;

#[async_trait]
impl HealthCheckProvider for LinuxHealthProvider {
    fn platform(&self) -> &'static str {
        "linux"
    }

    fn check_names(&self) -> &'static [&'static str] {
        LINUX_CHECK_NAMES
    }

    async fn run_check(&self, name: &str) -> CheckEntry {
        match name {
            NAME_BINARY_VERSION => check_binary_version(),
            NAME_PLATFORM_SUPPORTED => check_platform_supported(),
            NAME_SESSION_ACTIVE => check_session_active(),
            NAME_BUNDLE_IDENTITY => skip_not_applicable(NAME_BUNDLE_IDENTITY),
            NAME_TCC_ACCESSIBILITY => skip_not_applicable(NAME_TCC_ACCESSIBILITY),
            NAME_TCC_SCREEN_RECORDING => skip_not_applicable(NAME_TCC_SCREEN_RECORDING),
            NAME_AX_CAPABILITY => check_ax_capability().await,
            NAME_SCREEN_CAPTURE_CAPABILITY => check_screen_capture_capability().await,
            NAME_WAYLAND_BACKEND => check_wayland_backend().await,
            other => CheckEntry::skip(
                other.to_owned(),
                "Unknown check name (not implemented on this platform).",
            ),
        }
    }
}

// ── Individual checks ────────────────────────────────────────────────────────

pub(crate) fn check_binary_version() -> CheckEntry {
    CheckEntry::pass(
        NAME_BINARY_VERSION,
        format!("cua-driver {}", env!("CARGO_PKG_VERSION")),
    )
}

pub(crate) fn check_platform_supported() -> CheckEntry {
    let arch = arch_label();
    let os_version = os_release_pretty_name().unwrap_or_else(|| "Linux".to_owned());
    CheckEntry::pass(
        NAME_PLATFORM_SUPPORTED,
        format!("{os_version} ({arch})"),
    )
    .with_data(CheckData {
        os_version: Some(os_version),
        architecture: Some(arch.to_owned()),
        ..Default::default()
    })
}

pub(crate) fn check_session_active() -> CheckEntry {
    CheckEntry::pass(NAME_SESSION_ACTIVE, "MCP session is active.")
}

fn skip_not_applicable(name: &str) -> CheckEntry {
    CheckEntry::skip(name.to_owned(), "not applicable on Linux".to_owned())
}

async fn check_ax_capability() -> CheckEntry {
    // Native Wayland: AT-SPI lives entirely on D-Bus, so the AX prereq is
    // `org.a11y.Bus` being reachable on the session bus. Falls back to X11
    // reachability for X / XWayland sessions.
    if is_wayland_session() {
        let bus_ok = tokio::task::spawn_blocking(probe_a11y_bus)
            .await
            .unwrap_or(false);
        if bus_ok {
            return CheckEntry::pass(
                NAME_AX_CAPABILITY,
                "Wayland session: org.a11y.Bus reachable on the session bus; \
                 AT-SPI inspection will work.",
            );
        }
        return CheckEntry::fail(
            NAME_AX_CAPABILITY,
            "Wayland session: org.a11y.Bus is not reachable on the session bus; \
             AT-SPI inspection will fail.",
            "Start the AT-SPI bus (`/usr/libexec/at-spi-bus-launcher`) or enable \
             accessibility in your desktop's settings.",
        );
    }
    // Mirror the existing `check_permissions` Linux probe: X11
    // connectivity is the AX prerequisite (AT-SPI is over D-Bus, but
    // input/readback need an X server). Cheap and side-effect-free.
    let x11_ok = tokio::task::spawn_blocking(probe_x11_connect)
        .await
        .unwrap_or(false);
    if x11_ok {
        // X11 input works, but AT-SPI *inspection* (get_window_state) also needs
        // org.a11y.Bus on the session bus. Probe it so we don't claim AX works
        // when the tree would come back empty — the DBUS_SESSION_BUS_ADDRESS-
        // unset / a11y-bridge-off case the daemon now auto-recovers at startup.
        let a11y_ok = tokio::task::spawn_blocking(probe_a11y_bus)
            .await
            .unwrap_or(false);
        if a11y_ok {
            return CheckEntry::pass(
                NAME_AX_CAPABILITY,
                "X11 reachable and org.a11y.Bus is on the session bus; \
                 AT-SPI inspection + XSendEvent input will work.",
            );
        }
        let hint = if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
            "DBUS_SESSION_BUS_ADDRESS is set but org.a11y.Bus has no owner — enable \
             accessibility (`gsettings set org.gnome.desktop.interface \
             toolkit-accessibility true`) and/or start the AT-SPI bus \
             (`/usr/libexec/at-spi-bus-launcher`)."
        } else {
            "DBUS_SESSION_BUS_ADDRESS is unset and none was auto-discovered, so there \
             is no session bus to reach AT-SPI on — start the daemon inside the desktop \
             session, or ensure a /run/user/<uid>/bus socket exists (the daemon adopts \
             it at startup)."
        };
        return CheckEntry::fail(
            NAME_AX_CAPABILITY,
            "X11 is reachable but AT-SPI (org.a11y.Bus) is not — UI inspection \
             (get_window_state) will return empty trees; X11 input injection still works.",
            hint,
        );
    }
    let display_set = std::env::var_os("DISPLAY").is_some();
    let xauth_unset = std::env::var_os("XAUTHORITY").is_none();
    let hint = if display_set && xauth_unset {
        // SSH-driven Wayland+Xwayland: DISPLAY inherited but no auth cookie (#1926).
        "DISPLAY is set but XAUTHORITY is not (typical when driving a \
         Wayland+Xwayland session over SSH): the X server's per-session auth \
         cookie isn't discoverable, so X11 connects fail 'Authorization required'. \
         Point XAUTHORITY at the Xwayland '-auth' cookie file, or restart the \
         cua-driver daemon — it auto-discovers the cookie from /proc at startup."
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "Pure Wayland session: opt into the experimental Wayland backend by \
         setting CUA_DRIVER_RS_ENABLE_WAYLAND=1, or run the target under XWayland."
    } else {
        "Set DISPLAY (X11) — under Wayland, run via XWayland or expose XDG_SESSION_TYPE=x11."
    };
    CheckEntry::fail(
        NAME_AX_CAPABILITY,
        "X11 is not reachable; UI inspection and event injection will fail.",
        hint,
    )
}

async fn check_screen_capture_capability() -> CheckEntry {
    // Native Wayland: capture flows through a cascade — wlroots screencopy
    // (sway / labwc / kwin 5.27+ / hyprland) → ext-image-copy-capture-v1
    // (sway 1.10+ / labwc 0.8+ / niri / KDE 6.2+ / GNOME 47+) →
    // xdg-desktop-portal Screenshot (GNOME / KDE / COSMIC + portal backend).
    // Report which tier is reachable so users on mutter / kwin don't see a
    // misleading "screen capture will fail" just because wlroots isn't
    // present.
    if is_wayland_session() {
        let snap = tokio::task::spawn_blocking(probe_wayland_managers)
            .await
            .ok()
            .and_then(|r| r.ok());
        match &snap {
            Some(m) if m.screencopy && m.wl_shm => {
                return CheckEntry::pass(
                    NAME_SCREEN_CAPTURE_CAPABILITY,
                    "Wayland session: zwlr_screencopy_manager_v1 + wl_shm advertised; \
                     native output capture is functional.",
                );
            }
            Some(m) if m.ext_image_copy_capture && m.ext_output_image_capture_source => {
                return CheckEntry::pass(
                    NAME_SCREEN_CAPTURE_CAPABILITY,
                    "Wayland session: ext-image-copy-capture-v1 + \
                     ext-output-image-capture-source-v1 advertised; cross-DE \
                     native output capture is functional.",
                );
            }
            _ => {}
        }
        // No wlr screencopy AND no ext-image-copy-capture. Probe the
        // portal as the last native tier. The probe is read-only — it
        // checks for a name owner on the bus, does NOT take a screenshot.
        let portal_ok = tokio::task::spawn_blocking(probe_portal_screenshot)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(false);
        if portal_ok {
            return CheckEntry::pass(
                NAME_SCREEN_CAPTURE_CAPABILITY,
                "Wayland session: no native screencopy globals, but \
                 xdg-desktop-portal Screenshot is reachable; capture will go \
                 through the portal (one consent prompt per session).",
            );
        }
        return CheckEntry::fail(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            "Wayland session: no native screencopy globals, and \
             xdg-desktop-portal is not reachable on the session bus; \
             screen capture will fail.",
            "Use a wlroots-based compositor (sway, labwc, hyprland, kwin 5.27+) \
             or install xdg-desktop-portal-gnome / xdg-desktop-portal-kde / \
             xdg-desktop-portal-wlr.",
        );
    }
    // On Linux the canonical capture path is X11 GetImage / xwd / scrot
    // — all require an open X11 connection. The probe is the same one
    // we use for ax_capability, but the consumer-facing message and
    // hint differ so future drift between the two doesn't break either
    // contract.
    let x11_ok = tokio::task::spawn_blocking(probe_x11_connect)
        .await
        .unwrap_or(false);
    if x11_ok {
        return CheckEntry::pass(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            "X11 reachable; screen capture path is functional.",
        );
    }
    let hint = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "Pure Wayland session: opt into the experimental Wayland backend by \
         setting CUA_DRIVER_RS_ENABLE_WAYLAND=1, or run the target under XWayland."
    } else {
        "Set DISPLAY (X11). Pure Wayland sessions require an XWayland bridge for capture."
    };
    CheckEntry::fail(
        NAME_SCREEN_CAPTURE_CAPABILITY,
        "X11 is not reachable; screen capture will fail.",
        hint,
    )
}

/// `wayland_backend` doctor entry — surfaces which wlroots manager globals
/// the running compositor advertises. Skipped on non-Wayland sessions and
/// when the experimental Wayland backend isn't opted into, so X11 users see
/// a neutral skip rather than a confusing failure.
async fn check_wayland_backend() -> CheckEntry {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return CheckEntry::skip(
            NAME_WAYLAND_BACKEND.to_owned(),
            "No WAYLAND_DISPLAY in the environment — not a Wayland session.",
        );
    }
    if !wayland_opt_in_enabled() {
        return CheckEntry::skip(
            NAME_WAYLAND_BACKEND.to_owned(),
            format!(
                "Wayland session detected, but the experimental backend is opt-in. \
                 Set {}=1 to enable native Wayland and re-run doctor.",
                wayland_env_name()
            ),
        );
    }
    let snap = match tokio::task::spawn_blocking(probe_wayland_managers).await {
        Ok(Ok(snap)) => snap,
        Ok(Err(e)) => {
            return CheckEntry::fail(
                NAME_WAYLAND_BACKEND,
                format!("Failed to connect to the Wayland compositor: {e}"),
                "Verify WAYLAND_DISPLAY points at a running compositor socket.",
            );
        }
        Err(e) => {
            return CheckEntry::fail(
                NAME_WAYLAND_BACKEND,
                format!("Wayland probe task error: {e}"),
                "Re-run the doctor; if it persists, file a bug with the doctor output.",
            );
        }
    };
    let remote_desktop_portal_reachable = if crate::wayland::PORTAL_INPUT_ENABLED {
        tokio::task::spawn_blocking(probe_portal_remote_desktop)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(false)
    } else {
        false
    };
    classify_wayland_backend(
        &snap,
        crate::wayland::PORTAL_INPUT_ENABLED,
        remote_desktop_portal_reachable,
        crate::wayland::shell_helper::list_windows(None).is_some(),
    )
}

fn classify_wayland_backend(
    snap: &crate::wayland::WaylandManagers,
    portal_libei_enabled: bool,
    remote_desktop_portal_reachable: bool,
    target_activation_available: bool,
) -> CheckEntry {
    let msg = format!(
        "foreign-toplevel={ftl}, screencopy={cap}, ext-image-copy={ext_cap}, \
         ext-output-source={ext_src}, virtual-pointer={vp}, wl_shm={shm}",
        ftl = snap.foreign_toplevel,
        cap = snap.screencopy,
        ext_cap = snap.ext_image_copy_capture,
        ext_src = snap.ext_output_image_capture_source,
        vp = snap.virtual_pointer,
        shm = snap.wl_shm,
    );
    if snap.foreign_toplevel && snap.screencopy && snap.virtual_pointer && snap.wl_shm {
        return CheckEntry::pass(
            NAME_WAYLAND_BACKEND,
            format!("All wlroots manager globals advertised ({msg})."),
        );
    }
    if !snap.virtual_pointer {
        if remote_desktop_portal_reachable && target_activation_available {
            return CheckEntry::pass(
                NAME_WAYLAND_BACKEND,
                format!(
                    "No wlroots virtual-pointer advertised ({msg}), but this \
                     portal/libei build can reach the RemoteDesktop portal \
                     (proxy reachability only — the full create_session → \
                     select_devices → start → connect_to_eis handshake is NOT \
                     exercised here, to avoid a consent prompt on every doctor \
                     run, so this is not a guarantee that injection succeeds). \
                     The compositor helper also provides verified target \
                     activation before focus-bound portal input."
                ),
            );
        }
        if remote_desktop_portal_reachable {
            return CheckEntry::fail(
                NAME_WAYLAND_BACKEND,
                format!(
                    "The RemoteDesktop portal is reachable, but this compositor \
                     has no verified target-activation adapter ({msg}). Portal/libei \
                     input is global and would otherwise affect whichever window is \
                     focused, so cua-driver refuses foreground dispatch."
                ),
                "On GNOME, install and enable the bundled WinRects Shell helper, then \
                 log out and back in. KDE foreground input remains unavailable until \
                 a target-addressable KWin activation adapter is installed; AX actions \
                 and exact background refusals remain usable.",
            );
        }
        if portal_libei_enabled {
            return CheckEntry::fail(
                NAME_WAYLAND_BACKEND,
                format!(
                    "No wlroots virtual-pointer advertised ({msg}) and the \
                     portal/libei RemoteDesktop backend is compiled in but not \
                     reachable on this session; clicks and key presses have no \
                     native Wayland input backend."
                ),
                "Ensure xdg-desktop-portal and a desktop backend such as \
                 xdg-desktop-portal-gnome or xdg-desktop-portal-kde are running \
                 on the session bus, or run under XWayland.",
            );
        }
        return CheckEntry::fail(
            NAME_WAYLAND_BACKEND,
            format!(
                "Input injection has no backend on this compositor ({msg}): it \
                 advertises no zwlr_virtual_pointer and this build was compiled \
                 without libei/portal support, so clicks and key presses will not \
                 be delivered (the agent cursor still renders). list_windows and \
                 screen capture are unaffected."
            ),
            "Use the portal-enabled Linux build (compiled with --features \
             portal-input) for input on KDE Plasma / GNOME, or a wlroots \
             compositor (sway, labwc, hyprland) where zwlr_virtual_pointer exists.",
        );
    }
    // Partial-pass: list_windows + capture both work, but some optional
    // wlroots globals are absent. Require `wl_shm` here too —
    // `check_screen_capture_capability` gates on both `screencopy && wl_shm`,
    // so excluding `wl_shm` from the partial-pass verdict would let the
    // matrices disagree on degenerate compositors that omit it.
    if snap.foreign_toplevel && snap.screencopy && snap.wl_shm {
        return CheckEntry::pass(
            NAME_WAYLAND_BACKEND,
            format!(
                "Core wlroots manager globals available; some optional globals missing ({msg})."
            ),
        );
    }
    CheckEntry::fail(
        NAME_WAYLAND_BACKEND,
        format!(
            "Compositor does not advertise a complete native Wayland backend \
             set ({msg})."
        ),
        "Use a wlroots-based compositor (sway, labwc, hyprland), a portal/libei \
         build on GNOME/KDE, or run under XWayland.",
    )
}

// ── Probes ───────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn probe_x11_connect() -> bool {
    x11rb::rust_connection::RustConnection::connect(None).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn probe_x11_connect() -> bool {
    false
}

/// Probe whether `org.a11y.Bus` is reachable on the session bus — the
/// canonical AT-SPI prerequisite on Wayland, where there is no X server to
/// stand in. Returns false on any error (no session bus, no a11y service,
/// timeout, etc.) so the doctor message stays simple.
#[cfg(target_os = "linux")]
pub(crate) fn probe_a11y_bus() -> bool {
    use atspi::zbus;
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return false,
    };
    rt.block_on(async {
        let bus = match zbus::Connection::session().await {
            Ok(b) => b,
            Err(_) => return false,
        };
        let proxy = match zbus::fdo::DBusProxy::new(&bus).await {
            Ok(p) => p,
            Err(_) => return false,
        };
        // `name_has_owner` is the cheap direct existence check — avoids
        // pulling the entire session-bus name registry just to look up one
        // entry. Matches the pattern in `wayland::portal_screenshot::probe_portal`.
        let bus_name = match "org.a11y.Bus".try_into() {
            Ok(n) => n,
            Err(_) => return false,
        };
        proxy.name_has_owner(bus_name).await.unwrap_or(false)
    })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn probe_a11y_bus() -> bool {
    false
}

/// True when the Wayland backend is opted in and a Wayland session is
/// active. Wrapper around `wayland::is_wayland()` so non-Linux builds of
/// this file compile (the `wayland` module is gated on `target_os = linux`).
#[cfg(target_os = "linux")]
fn is_wayland_session() -> bool {
    crate::wayland::is_wayland()
}

#[cfg(not(target_os = "linux"))]
fn is_wayland_session() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn wayland_opt_in_enabled() -> bool {
    crate::wayland::wayland_enabled()
}

#[cfg(not(target_os = "linux"))]
fn wayland_opt_in_enabled() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn wayland_env_name() -> &'static str {
    crate::wayland::ENABLE_WAYLAND_ENV
}

#[cfg(not(target_os = "linux"))]
fn wayland_env_name() -> &'static str {
    "CUA_DRIVER_RS_ENABLE_WAYLAND"
}

/// Snapshot of wlroots manager globals. The non-Linux stub returns an empty
/// snapshot so off-platform builds stay green; doctor short-circuits before
/// reaching it via [`is_wayland_session`].
#[cfg(target_os = "linux")]
fn probe_wayland_managers() -> anyhow::Result<crate::wayland::WaylandManagers> {
    crate::wayland::probe_managers()
}

#[cfg(not(target_os = "linux"))]
fn probe_wayland_managers() -> anyhow::Result<WaylandManagers> {
    Ok(WaylandManagers::default())
}

#[cfg(target_os = "linux")]
fn probe_portal_screenshot() -> anyhow::Result<bool> {
    crate::wayland::portal_screenshot::probe_portal()
}

#[cfg(not(target_os = "linux"))]
fn probe_portal_screenshot() -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(target_os = "linux")]
fn probe_portal_remote_desktop() -> anyhow::Result<bool> {
    #[cfg(feature = "portal-input")]
    {
        use ashpd::desktop::remote_desktop::RemoteDesktop;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for RemoteDesktop probe: {e}"))?;

        rt.block_on(async {
            match RemoteDesktop::new().await {
                Ok(_) => Ok(true),
                Err(e) => {
                    let msg = format!("{e}");
                    if msg.contains("ServiceUnknown")
                        || msg.contains("NameHasNoOwner")
                        || msg.contains("NotFound")
                    {
                        Ok(false)
                    } else {
                        Err(anyhow::anyhow!("portal RemoteDesktop probe failed: {e}"))
                    }
                }
            }
        })
    }
    #[cfg(not(feature = "portal-input"))]
    {
        Ok(false)
    }
}

#[cfg(not(target_os = "linux"))]
fn probe_portal_remote_desktop() -> anyhow::Result<bool> {
    Ok(false)
}

/// Stub of `wayland::WaylandManagers` so off-Linux builds compile. Always
/// reports nothing advertised — non-Linux code paths never call this.
#[cfg(not(target_os = "linux"))]
#[derive(Default, Clone, Debug)]
struct WaylandManagers {
    foreign_toplevel: bool,
    screencopy: bool,
    ext_image_copy_capture: bool,
    ext_output_image_capture_source: bool,
    virtual_pointer: bool,
    wl_shm: bool,
}

/// `PRETTY_NAME=` out of `/etc/os-release`. Fails to "Linux" so the
/// downstream message stays human-readable.
fn os_release_pretty_name() -> Option<String> {
    let s = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            let v = rest.trim_matches('"').to_owned();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::health_report::{CheckStatus, HealthReportTool};
    use cua_driver_core::tool::Tool;
    use std::sync::Arc;

    #[test]
    fn binary_version_always_passes() {
        let entry = check_binary_version();
        assert_eq!(entry.status, CheckStatus::Pass);
        assert!(entry.message.contains("cua-driver "));
    }

    #[test]
    fn session_active_passes() {
        let entry = check_session_active();
        assert_eq!(entry.status, CheckStatus::Pass);
    }

    #[test]
    fn tcc_and_bundle_are_skipped_with_canonical_message() {
        for name in [
            NAME_TCC_ACCESSIBILITY,
            NAME_TCC_SCREEN_RECORDING,
            NAME_BUNDLE_IDENTITY,
        ] {
            let entry = skip_not_applicable(name);
            assert_eq!(entry.status, CheckStatus::Skip, "{name} must be skipped");
            assert_eq!(entry.message, "not applicable on Linux");
        }
    }

    #[test]
    fn wayland_backend_passes_on_non_wlroots_when_portal_libei_backend_is_reachable() {
        let snap = crate::wayland::WaylandManagers {
            foreign_toplevel: false,
            screencopy: false,
            ext_image_copy_capture: false,
            ext_output_image_capture_source: false,
            virtual_pointer: false,
            wl_shm: true,
        };

        let entry = classify_wayland_backend(&snap, true, true, true);

        assert_eq!(entry.status, CheckStatus::Pass);
        assert!(entry.message.contains("portal/libei"), "{}", entry.message);
        assert!(entry.message.contains("RemoteDesktop"), "{}", entry.message);
    }

    #[test]
    fn wayland_backend_fails_on_non_wlroots_when_portal_libei_backend_is_unreachable() {
        let snap = crate::wayland::WaylandManagers {
            foreign_toplevel: false,
            screencopy: false,
            ext_image_copy_capture: false,
            ext_output_image_capture_source: false,
            virtual_pointer: false,
            wl_shm: true,
        };

        let entry = classify_wayland_backend(&snap, true, false, false);

        assert_eq!(entry.status, CheckStatus::Fail);
        assert!(entry.message.contains("portal/libei"), "{}", entry.message);
        assert!(entry.hint.as_deref().unwrap_or("").contains("xdg-desktop-portal"));
    }

    #[test]
    fn wayland_backend_fails_closed_when_portal_input_cannot_target_a_window() {
        let snap = crate::wayland::WaylandManagers {
            foreign_toplevel: false,
            screencopy: false,
            ext_image_copy_capture: false,
            ext_output_image_capture_source: false,
            virtual_pointer: false,
            wl_shm: true,
        };

        let entry = classify_wayland_backend(&snap, true, true, false);

        assert_eq!(entry.status, CheckStatus::Fail);
        assert!(entry.message.contains("target-activation"), "{}", entry.message);
        assert!(entry.message.contains("wrong application"), "{}", entry.message);
    }

    #[tokio::test]
    async fn invoke_full_run_produces_linux_check_set() {
        let provider = Arc::new(LinuxHealthProvider);
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(serde_json::json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError"
        );
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured["platform"], "linux");
        assert_eq!(structured["schema_version"], "1");
        let names: Vec<&str> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        let expected: Vec<&str> = LINUX_CHECK_NAMES.to_vec();
        assert_eq!(names, expected);

        let by_name: std::collections::HashMap<_, _> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["name"].as_str().unwrap(), c))
            .collect();
        for name in [
            NAME_TCC_ACCESSIBILITY,
            NAME_TCC_SCREEN_RECORDING,
            NAME_BUNDLE_IDENTITY,
        ] {
            let entry = by_name[name];
            assert_eq!(
                entry["status"], "skip",
                "{name} must be skipped on Linux"
            );
            assert_eq!(entry["message"], "not applicable on Linux");
        }
    }
}
