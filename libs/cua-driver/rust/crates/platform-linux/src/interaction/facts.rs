//! Facts an adapter reports to the shared policy, each one known or
//! unknown with the reason it could not be observed.

use std::time::Duration;

use anyhow::{ensure, Result};
use zbus::zvariant::OwnedObjectPath;

/// A fact an adapter observed, or why it could not.
pub enum Fact<T> {
    Known(T),
    Unknown(String),
}

const LOCK_PROBE_CAP: Duration = Duration::from_millis(250);

/// Whether the session's screensaver reports itself active. KWin and X11
/// sessions ask `org.freedesktop.ScreenSaver`, GNOME asks
/// `org.gnome.ScreenSaver`; the answer counts only when the service's owner
/// runs in this process's logind session. niri, Sway and Hyprland expose no
/// lock state, and a missing service, a timeout or another session's owner
/// leave it unknown.
pub fn locked() -> Fact<bool> {
    use crate::wayland::{hyprland, is_wayland, niri};
    if niri::is_session()
        || (is_wayland() && hyprland::is_session())
        || std::env::var_os("SWAYSOCK").is_some()
    {
        return Fact::Unknown("this compositor reports no lock state".into());
    }
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.contains("GNOME"));
    let (service, path) = if gnome {
        ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver")
    } else {
        (
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
        )
    };
    let wait = super::operation_deadline(LOCK_PROBE_CAP)
        .saturating_duration_since(std::time::Instant::now());
    let probe = std::thread::spawn(move || -> Result<bool> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            tokio::time::timeout(wait, screensaver_active(service, path))
                .await
                .map_err(|_| anyhow::anyhow!("{service} did not answer within {wait:?}"))?
        })
    });
    match probe.join() {
        Ok(Ok(active)) => Fact::Known(active),
        Ok(Err(error)) => Fact::Unknown(format!("{service}.GetActive: {error:#}")),
        Err(_) => Fact::Unknown(format!("{service}.GetActive probe panicked")),
    }
}

async fn screensaver_active(service: &str, path: &str) -> Result<bool> {
    let session = zbus::Connection::session().await?;
    let bus = zbus::Proxy::new(
        &session,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let owner: String = bus.call("GetNameOwner", &(service,)).await?;
    let owner_pid: u32 = bus
        .call("GetConnectionUnixProcessID", &(owner.as_str(),))
        .await?;
    let system = zbus::Connection::system().await?;
    let logind = zbus::Proxy::new(
        &system,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let theirs: OwnedObjectPath = logind.call("GetSessionByPID", &(owner_pid,)).await?;
    let ours: OwnedObjectPath = logind
        .call("GetSessionByPID", &(std::process::id(),))
        .await?;
    ensure!(
        theirs == ours,
        "its owner runs in logind session {}, not this process's {}",
        theirs.as_str(),
        ours.as_str()
    );
    let saver = zbus::Proxy::new(&session, owner.as_str(), path, service).await?;
    Ok(saver.call("GetActive", &()).await?)
}
