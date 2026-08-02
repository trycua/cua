//! `xdg-desktop-portal` Screenshot capture path.
//!
//! Reaches GNOME/Mutter, KDE/KWin, and any compositor that ships an
//! xdg-desktop-portal backend — superset of the wlroots-only
//! `zwlr_screencopy_manager_v1` path. The portal asks the user for consent
//! on first invocation per session (GNOME caches via `Settings → Privacy →
//! Screenshot`; KDE Plasma 6 caches per requesting binary); subsequent
//! calls in the same session are silent.
//!
//! ashpd is async (zbus under the hood); the rest of `wayland::screenshot_*`
//! is sync because it's called from `tokio::task::spawn_blocking`. We build
//! a dedicated single-threaded tokio runtime per call so the async ashpd
//! work has somewhere to live without colliding with the caller's runtime.

use std::path::PathBuf;

use ashpd::desktop::screenshot::Screenshot;

/// Capture the current display via `org.freedesktop.portal.Screenshot.Screenshot`.
/// Returns the PNG/JPEG bytes the portal wrote to disk, or a typed error
/// when the portal isn't reachable (e.g. wlroots compositor without
/// `xdg-desktop-portal-wlr`, KDE without `xdg-desktop-portal-kde` running,
/// or no D-Bus session bus).
///
/// `interactive=false` skips the customization dialog (region select,
/// post-shot editor) — the user still consents to the screenshot itself
/// on the first call per session unless they've pre-approved the
/// requesting binary in their DE's privacy settings.
pub fn screenshot_via_portal() -> anyhow::Result<Vec<u8>> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(screenshot_via_portal_blocking)
            .join()
            .map_err(|_| anyhow::anyhow!("xdg-desktop-portal Screenshot worker panicked"))?;
    }
    screenshot_via_portal_blocking()
}

fn screenshot_via_portal_blocking() -> anyhow::Result<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd: {e}"))?;
    let uri = rt.block_on(async {
        let connection = super::portal::fresh_session_connection().await?;
        Screenshot::request()
            .interactive(false)
            .modal(false)
            .connection(Some(connection))
            .send()
            .await
            .map_err(map_ashpd_err)?
            .response()
            .map_err(map_ashpd_err)
            .map(|resp| resp.uri().clone())
    })?;

    // The portal returns a file:// URI pointing at a PNG (typically in
    // /tmp or $XDG_RUNTIME_DIR/doc/...). Decode it via the `url` crate.
    let path = uri_to_path(&uri.to_string())
        .ok_or_else(|| anyhow::anyhow!("portal returned a non-file URI: {uri}"))?;
    let bytes = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("portal screenshot at {} unreadable: {e}", path.display()))?;
    // Best-effort cleanup: the portal places the file in a process-readable
    // location that can outlive our process. Removing avoids the disk
    // accumulating one PNG per call. If removal fails (permissions, file
    // already gone) we silently drop the error — the bytes are already
    // captured.
    let _ = std::fs::remove_file(&path);
    Ok(bytes)
}

/// Convert a `file://...` URI string to a local PathBuf. Returns None for
/// non-file schemes (e.g. portal `doc://...` URIs in some sandboxed
/// configurations, which would need a second resolution step through the
/// document portal).
fn uri_to_path(uri_str: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri_str).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

fn map_ashpd_err<E: std::fmt::Display>(e: E) -> anyhow::Error {
    let msg = format!("xdg-desktop-portal Screenshot failed: {e}");
    // Pattern-match common failure modes to produce a typed-ish error
    // the caller can pivot on.
    if msg.contains("ServiceUnknown") || msg.contains("NotFound") {
        anyhow::anyhow!("xdg-desktop-portal is not running on this session ({msg}). Install xdg-desktop-portal-gnome / xdg-desktop-portal-kde / xdg-desktop-portal-wlr for your compositor.")
    } else if msg.contains("Cancelled") || msg.contains("denied") {
        anyhow::anyhow!(
            "xdg-desktop-portal Screenshot consent dialog was cancelled or denied: {msg}"
        )
    } else {
        anyhow::anyhow!(msg)
    }
}

/// Probe whether the portal *service* is reachable on the session bus.
/// Returns Ok(true) when the `org.freedesktop.portal.Desktop` name has an
/// owner, Ok(false) otherwise, and Err(...) for transport failures
/// (no session bus, etc.).
///
/// Read-only and side-effect free — does NOT actually invoke the
/// Screenshot interface, so it doesn't cache a consent grant on GNOME /
/// KDE backends that gate Screenshot on user prompts. The doctor uses
/// this to surface portal availability without burning an interactive
/// dialog every time `hermes computer-use doctor` runs.
///
/// Note: a true return here means the portal *service* is present. The
/// Screenshot *interface* on that service may still be missing or
/// disabled — for that, an actual `Screenshot::request().send().await`
/// is needed. We accept the false-positive risk in exchange for the
/// no-consent, no-burn-dialog property.
pub fn probe_portal() -> anyhow::Result<bool> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd probe: {e}"))?;
    rt.block_on(async {
        tokio::time::timeout(super::portal::PROBE_TIMEOUT, async {
            // Cheapest possible probe: open a session bus and check that the
            // portal name has an owner. We avoid Screenshot::request() because
            // that would cache a (potentially undesired) consent grant.
            let conn = super::portal::fresh_session_connection().await?;
            let proxy = zbus::fdo::DBusProxy::new(&conn)
                .await
                .map_err(|e| anyhow::anyhow!("dbus proxy creation failed: {e}"))?;
            let has_owner = proxy
                .name_has_owner(
                    "org.freedesktop.portal.Desktop"
                        .try_into()
                        .map_err(|e| anyhow::anyhow!("bus name parse failed: {e}"))?,
                )
                .await
                .map_err(|e| anyhow::anyhow!("name_has_owner failed: {e}"))?;
            Ok(has_owner)
        })
        .await
        .map_err(|_| anyhow::anyhow!("xdg-desktop-portal availability probe timed out"))?
    })
}
