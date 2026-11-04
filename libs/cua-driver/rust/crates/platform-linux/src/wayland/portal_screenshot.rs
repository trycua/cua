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
//!
//! This module owns the portal tier's cascade policy: a refusal from a
//! reachable portal is terminal, while an unreachable or unresponsive portal
//! lets the display cascade continue.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use ashpd::desktop::screenshot::Screenshot;
use ashpd::desktop::ResponseError;
use ashpd::PortalError;

/// Deadline for the whole portal Screenshot request, connection included.
/// Much longer than [`super::portal::PROBE_TIMEOUT`] because a person may
/// have to answer a first-use consent dialog; without it an unanswered
/// dialog would block the capture call forever.
pub(super) const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a portal Screenshot request did not produce a file URI.
pub(super) enum RequestError {
    /// The session bus could not be reached.
    Connect(anyhow::Error),
    /// The portal (or ashpd talking to it) reported a failure.
    Portal(ashpd::Error),
}

impl From<ashpd::Error> for RequestError {
    fn from(e: ashpd::Error) -> Self {
        Self::Portal(e)
    }
}

/// Capture the current display via `org.freedesktop.portal.Screenshot.Screenshot`.
///
/// - `Ok(Some(bytes))`: the portal captured the display.
/// - `Ok(None)`: the portal is unavailable (no session bus, no portal
///   backend, no response, or no answer within [`SCREENSHOT_TIMEOUT`]); the
///   reason is logged and the caller may try another capture path.
/// - `Err(_)`: the portal refused the screenshot (denied, dismissed, or
///   ended without a grant). This is terminal for the display cascade.
///
/// `interactive=false` skips the customization dialog (region select,
/// post-shot editor) — the user still consents to the screenshot itself
/// on the first call per session unless they've pre-approved the
/// requesting binary in their DE's privacy settings.
pub fn screenshot_via_portal() -> anyhow::Result<Option<Vec<u8>>> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(screenshot_via_portal_blocking)
            .join()
            .map_err(|_| anyhow::anyhow!("xdg-desktop-portal Screenshot worker panicked"))?;
    }
    screenshot_via_portal_blocking()
}

fn screenshot_via_portal_blocking() -> anyhow::Result<Option<Vec<u8>>> {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            return Ok(unavailable(format!(
                "failed to build tokio runtime for ashpd: {e}"
            )))
        }
    };
    let request = async {
        let connection = super::portal::fresh_session_connection()
            .await
            .map_err(RequestError::Connect)?;
        let response = Screenshot::request()
            .interactive(false)
            .modal(false)
            .connection(Some(connection))
            .send()
            .await?
            .response()?;
        Ok(response.uri().to_string())
    };
    rt.block_on(request_outcome(request, SCREENSHOT_TIMEOUT))
}

/// Apply the portal tier's policy to one Screenshot request: bound it by
/// `deadline`, turn a refusal into a terminal error, and read the captured
/// file. Everything else is "unavailable" (`Ok(None)`).
pub(super) async fn request_outcome(
    request: impl Future<Output = Result<String, RequestError>>,
    deadline: Duration,
) -> anyhow::Result<Option<Vec<u8>>> {
    let uri = match tokio::time::timeout(deadline, request).await {
        Err(_) => return Ok(unavailable(format!("request timed out after {deadline:?}"))),
        Ok(Err(RequestError::Connect(e))) => return Ok(unavailable(e)),
        Ok(Err(RequestError::Portal(e))) => match refusal(&e) {
            Some(refused) => return Err(refused),
            None => return Ok(unavailable(describe_unavailable(&e))),
        },
        Ok(Ok(uri)) => uri,
    };
    // The portal returns a file:// URI pointing at a PNG (typically in
    // /tmp or $XDG_RUNTIME_DIR/doc/...). Decode it via the `url` crate.
    let Some(path) = uri_to_path(&uri) else {
        return Ok(unavailable(format!(
            "portal returned a non-file URI: {uri}"
        )));
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Ok(unavailable(format!(
                "portal screenshot at {} unreadable: {e}",
                path.display()
            )))
        }
    };
    // Best-effort cleanup: the portal places the file in a process-readable
    // location that can outlive our process. Removing avoids the disk
    // accumulating one PNG per call. If removal fails (permissions, file
    // already gone) we silently drop the error — the bytes are already
    // captured.
    let _ = std::fs::remove_file(&path);
    Ok(Some(bytes))
}

fn unavailable(reason: impl std::fmt::Display) -> Option<Vec<u8>> {
    tracing::debug!("xdg-desktop-portal Screenshot unavailable ({reason}); falling through to X11");
    None
}

/// A terminal error when the reachable portal answered without granting the
/// screenshot, or `None` when the failure means the portal is unavailable.
fn refusal(e: &ashpd::Error) -> Option<anyhow::Error> {
    match e {
        ashpd::Error::Response(ResponseError::Cancelled)
        | ashpd::Error::Portal(PortalError::Cancelled(_) | PortalError::NotAllowed(_)) => {
            Some(anyhow::anyhow!(
                "xdg-desktop-portal Screenshot denied: {e}. Grant screenshot permission for \
                 this process in the desktop's privacy settings and retry."
            ))
        }
        // ashpd: the interaction ended for an unspecified reason. Not
        // reported as a denial, but still no screenshot from a reachable
        // portal, so an X11 root image would not be the requested desktop.
        ashpd::Error::Response(ResponseError::Other) => Some(anyhow::anyhow!(
            "xdg-desktop-portal Screenshot not granted: the portal ended the request without \
             a screenshot and gave no reason. Check the portal backend logs or the desktop's \
             screenshot permission."
        )),
        _ => None,
    }
}

fn describe_unavailable(e: &ashpd::Error) -> String {
    let msg = e.to_string();
    if matches!(e, ashpd::Error::PortalNotFound(_))
        || msg.contains("ServiceUnknown")
        || msg.contains("NotFound")
    {
        format!("xdg-desktop-portal is not running on this session ({msg}). Install xdg-desktop-portal-gnome / xdg-desktop-portal-kde / xdg-desktop-portal-wlr for your compositor.")
    } else {
        msg
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn pending_request_times_out_at_deadline() {
        let started = tokio::time::Instant::now();
        let outcome = request_outcome(
            std::future::pending::<Result<String, RequestError>>(),
            SCREENSHOT_TIMEOUT,
        )
        .await;
        assert!(outcome.unwrap().is_none());
        assert!(started.elapsed() >= SCREENSHOT_TIMEOUT);
    }

    #[tokio::test]
    async fn request_resolving_before_deadline_returns_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("portal-shot.png");
        std::fs::write(&path, b"png-bytes").unwrap();
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        let outcome = request_outcome(async { Ok(uri) }, SCREENSHOT_TIMEOUT).await;
        assert_eq!(outcome.unwrap().as_deref(), Some(&b"png-bytes"[..]));
        assert!(
            !path.exists(),
            "portal file should be removed after reading"
        );
    }
}
