//! Switch on Chromium / Electron accessibility for the whole desktop session.
//!
//! Chromium — and therefore every Electron, CEF, and Chrome-based app — ships
//! its accessibility tree disabled and only builds it once it believes an
//! assistive technology is listening. It decides that by watching the
//! freedesktop accessibility status published on the session bus: the
//! `org.a11y.Bus` service exposes an `org.a11y.Status` interface whose
//! `ScreenReaderEnabled` property is the signal. Until that property is true a
//! Chromium window registers either nothing or an empty application on the
//! AT-SPI registry, so [`crate::atspi`] walks an empty tree and
//! `get_window_state` reports the app as having no elements — it is invisible
//! to the driver. Electron apps shipped as AppImages behave identically; they
//! embed the same Chromium.
//!
//! A real screen reader turns the Chromium signal on. Doing that ourselves is
//! unsafe on GNOME: its settings daemon treats the signal as a user request and
//! launches Orca. GNOME therefore gets only the generic `IsEnabled` signal by
//! default. Other desktops retain the Chromium signal for compatibility, and a
//! caller can choose either policy explicitly with
//! `CUA_DRIVER_RS_A11Y_ADVERTISE_MODE`.
//!
//! Everything here is best-effort. A session without an accessibility bus (some
//! headless or minimal setups) just yields an error we log and ignore; enabling
//! accessibility must never be able to fail daemon startup.

use std::sync::Once;

use anyhow::{anyhow, Context};
use atspi::zbus;

/// Well-known session-bus name of the freedesktop accessibility-bus launcher,
/// which also carries the session's accessibility status.
const ACCESSIBILITY_BUS_SERVICE: &str = "org.a11y.Bus";
/// Object on [`ACCESSIBILITY_BUS_SERVICE`] exposing `org.a11y.Status`.
const ACCESSIBILITY_BUS_OBJECT: &str = "/org/a11y/bus";
/// Interface holding the session's accessibility-enabled / screen-reader flags.
const ACCESSIBILITY_STATUS_INTERFACE: &str = "org.a11y.Status";
/// Property Chromium watches to decide whether to build its AT-SPI tree.
const SCREEN_READER_ENABLED_PROPERTY: &str = "ScreenReaderEnabled";
/// Companion property GTK/Qt watch to load their AT-SPI bridges.
const ACCESSIBILITY_IS_ENABLED_PROPERTY: &str = "IsEnabled";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdvertiseMode {
    All,
    IsEnabledOnly,
    None,
}

/// Advertise an assistive technology to the session exactly once per daemon
/// process, so Chromium/Electron (including Electron AppImages), GTK, and Qt
/// expose their accessibility trees to [`crate::atspi`]. Idempotent and
/// best-effort: repeated calls do nothing, and any failure is logged and
/// swallowed so it can never block startup.
pub fn ensure_chromium_accessibility_enabled() {
    static ADVERTISED: Once = Once::new();
    ADVERTISED.call_once(|| {
        let mode = advertise_mode_from(
            std::env::var_os("CUA_DRIVER_RS_DISABLE_A11Y_ADVERTISE").is_some(),
            std::env::var("CUA_DRIVER_RS_A11Y_ADVERTISE_MODE")
                .ok()
                .as_deref(),
            std::env::var("XDG_CURRENT_DESKTOP")
                .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
                .or_else(|_| std::env::var("DESKTOP_SESSION"))
                .ok()
                .as_deref(),
        );
        if mode == AdvertiseMode::None {
            tracing::debug!(
                "accessibility advertisement disabled; leaving session status untouched"
            );
            return;
        }
        if let Err(error) = advertise_accessibility_to_session(mode) {
            tracing::debug!(
                "skipped advertising accessibility to the session \
                 (Chromium/Electron trees may stay empty): {error:#}"
            );
        }
    });
}

fn advertise_accessibility_to_session(mode: AdvertiseMode) -> anyhow::Result<()> {
    // The daemon's tokio runtime is already driving this thread when the tool
    // registry is built, and `block_on` panics if called from within a runtime.
    // Run the one-shot bus work on a dedicated OS thread that owns a small
    // runtime of its own, then join it — no nesting, torn down once it returns.
    std::thread::Builder::new()
        .name("cua-a11y-advertise".into())
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(advertise_accessibility(mode))
        })
        .context("spawning the accessibility-advertise thread")?
        .join()
        .map_err(|_| anyhow!("accessibility-advertise thread panicked"))?
}

async fn advertise_accessibility(mode: AdvertiseMode) -> anyhow::Result<()> {
    let session_bus = zbus::Connection::session().await?;
    let status = zbus::Proxy::new(
        &session_bus,
        ACCESSIBILITY_BUS_SERVICE,
        ACCESSIBILITY_BUS_OBJECT,
        ACCESSIBILITY_STATUS_INTERFACE,
    )
    .await?;

    // Don't clobber a screen reader the user is already running: only write when
    // a flag is currently false, so an active Orca session stays authoritative
    // and we avoid emitting a redundant PropertiesChanged.
    if mode == AdvertiseMode::All && !is_flag_set(&status, SCREEN_READER_ENABLED_PROPERTY).await {
        status
            .set_property(SCREEN_READER_ENABLED_PROPERTY, true)
            .await?;
    }
    if !is_flag_set(&status, ACCESSIBILITY_IS_ENABLED_PROPERTY).await {
        status
            .set_property(ACCESSIBILITY_IS_ENABLED_PROPERTY, true)
            .await?;
    }
    Ok(())
}

fn advertise_mode_from(
    disabled: bool,
    configured: Option<&str>,
    desktop: Option<&str>,
) -> AdvertiseMode {
    if disabled {
        return AdvertiseMode::None;
    }
    match configured
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("all") => AdvertiseMode::All,
        Some("is_enabled_only") => AdvertiseMode::IsEnabledOnly,
        Some("none") => AdvertiseMode::None,
        Some(other) => {
            tracing::warn!(
                mode = other,
                "unknown CUA_DRIVER_RS_A11Y_ADVERTISE_MODE; using desktop default"
            );
            desktop_default_mode(desktop)
        }
        None => desktop_default_mode(desktop),
    }
}

fn desktop_default_mode(desktop: Option<&str>) -> AdvertiseMode {
    let is_gnome = desktop.is_some_and(|desktop| {
        desktop
            .split([':', ';'])
            .any(|part| part.trim().eq_ignore_ascii_case("gnome"))
    });
    if is_gnome {
        AdvertiseMode::IsEnabledOnly
    } else {
        AdvertiseMode::All
    }
}

/// Read a boolean status property, treating an unreadable property as unset so
/// the caller falls through to writing it.
async fn is_flag_set(status: &zbus::Proxy<'_>, property: &str) -> bool {
    status.get_property::<bool>(property).await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{advertise_mode_from, AdvertiseMode};

    #[test]
    fn gnome_default_does_not_claim_a_screen_reader() {
        assert_eq!(
            advertise_mode_from(false, None, Some("ubuntu:GNOME")),
            AdvertiseMode::IsEnabledOnly
        );
    }

    #[test]
    fn non_gnome_default_preserves_chromium_compatibility() {
        assert_eq!(
            advertise_mode_from(false, None, Some("KDE")),
            AdvertiseMode::All
        );
    }

    #[test]
    fn explicit_mode_overrides_desktop_default() {
        assert_eq!(
            advertise_mode_from(false, Some("all"), Some("GNOME")),
            AdvertiseMode::All
        );
        assert_eq!(
            advertise_mode_from(false, Some("is_enabled_only"), Some("KDE")),
            AdvertiseMode::IsEnabledOnly
        );
        assert_eq!(
            advertise_mode_from(false, Some("none"), Some("KDE")),
            AdvertiseMode::None
        );
    }

    #[test]
    fn legacy_disable_wins_over_explicit_mode() {
        assert_eq!(
            advertise_mode_from(true, Some("all"), Some("KDE")),
            AdvertiseMode::None
        );
    }
}
