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
//! A real screen reader (Orca) turns the tree on simply by writing that status
//! property when it starts. We do the same once, at daemon startup. Because the
//! status object lives on the session-scoped accessibility-bus launcher rather
//! than in our process, the flag is session-wide, outlives us, and takes effect
//! retroactively on apps that are already running — no relaunch and no
//! per-application command-line flag. GTK and Qt gate their own AT-SPI bridges
//! on the companion `IsEnabled` property, so setting it warms those toolkits
//! too.
//!
//! Everything here is best-effort. A session without an accessibility bus (some
//! headless or minimal setups) just yields an error we log and ignore; enabling
//! accessibility must never be able to fail daemon startup.

use std::sync::Once;

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

/// Advertise an assistive technology to the session exactly once per daemon
/// process, so Chromium/Electron (including Electron AppImages), GTK, and Qt
/// expose their accessibility trees to [`crate::atspi`]. Idempotent and
/// best-effort: repeated calls do nothing, and any failure is logged and
/// swallowed so it can never block startup.
pub fn ensure_chromium_accessibility_enabled() {
    static ADVERTISED: Once = Once::new();
    ADVERTISED.call_once(|| {
        // Opt-out for the rare session that wants its accessibility status left
        // untouched (e.g. one already driven by a real screen reader the user
        // configured deliberately).
        if std::env::var_os("CUA_DRIVER_RS_DISABLE_A11Y_ADVERTISE").is_some() {
            tracing::debug!(
                "CUA_DRIVER_RS_DISABLE_A11Y_ADVERTISE set; leaving session \
                 accessibility status untouched"
            );
            return;
        }
        if let Err(error) = advertise_screen_reader_to_session() {
            tracing::debug!(
                "skipped advertising accessibility to the session \
                 (Chromium/Electron trees may stay empty): {error:#}"
            );
        }
    });
}

fn advertise_screen_reader_to_session() -> anyhow::Result<()> {
    // One-shot startup work: a private current-thread runtime keeps this off the
    // shared AT-SPI walk runtime and is torn down as soon as the writes land.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let session_bus = zbus::Connection::session().await?;
        let status = zbus::Proxy::new(
            &session_bus,
            ACCESSIBILITY_BUS_SERVICE,
            ACCESSIBILITY_BUS_OBJECT,
            ACCESSIBILITY_STATUS_INTERFACE,
        )
        .await?;

        // Don't clobber a screen reader the user is already running: only write
        // when a flag is currently false, so an active Orca session stays
        // authoritative and we avoid emitting a redundant PropertiesChanged.
        if !is_flag_set(&status, SCREEN_READER_ENABLED_PROPERTY).await {
            status
                .set_property(SCREEN_READER_ENABLED_PROPERTY, true)
                .await?;
        }
        if !is_flag_set(&status, ACCESSIBILITY_IS_ENABLED_PROPERTY).await {
            status
                .set_property(ACCESSIBILITY_IS_ENABLED_PROPERTY, true)
                .await?;
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// Read a boolean status property, treating an unreadable property as unset so
/// the caller falls through to writing it.
async fn is_flag_set(status: &zbus::Proxy<'_>, property: &str) -> bool {
    status.get_property::<bool>(property).await.unwrap_or(false)
}
