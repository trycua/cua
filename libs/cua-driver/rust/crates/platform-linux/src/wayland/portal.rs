//! Shared xdg-desktop-portal connection helpers.
//!
//! ashpd's convenience constructors cache their first session-bus connection
//! process-wide. That is unsafe for our portal callers because several of them
//! deliberately use short-lived Tokio runtimes: once the first runtime is
//! dropped, the cached zbus connection no longer has a running executor and a
//! later portal call can wait forever. Always give ashpd an explicit
//! connection whose lifetime is owned by the caller's runtime instead.

use std::time::Duration;

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) async fn fresh_session_connection() -> anyhow::Result<zbus::Connection> {
    zbus::Connection::session()
        .await
        .map_err(|e| anyhow::anyhow!("xdg-desktop-portal session bus unreachable: {e}"))
}
