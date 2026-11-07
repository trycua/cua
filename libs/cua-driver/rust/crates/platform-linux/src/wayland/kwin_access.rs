// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The operator's switch for the KWin window adapter. It lives in its own file,
//! `~/.cua-driver/kwin-window-capture`, which only `cua-driver kwin-capture`
//! writes, and is read on every call, so a change needs no restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SETTING_FILE: &str = "kwin-window-capture";
const DESKTOP_ENTRY: &str = "ai.cua.driver.desktop";
/// One portal answer serves the several checks of one call.
const PORTAL_ANSWER_TTL: Duration = Duration::from_secs(1);
const PORTAL_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Off,
    On,
    /// On, even when the user saved a screenshot Deny in the desktop portal.
    OverridePortalDeny,
}

impl Setting {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "on" => Some(Self::On),
            "override-portal-deny" => Some(Self::OverridePortalDeny),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::OverridePortalDeny => "override-portal-deny",
        }
    }

    fn saved() -> Self {
        setting_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| Self::parse(text.trim()))
            .unwrap_or(Self::Off)
    }
}

/// Whether the KWin adapter may run for this call.
pub(crate) fn allowed() -> bool {
    allowed_with(Setting::saved(), portal_denies_screenshots)
}

fn allowed_with(setting: Setting, portal_denies: impl FnOnce() -> bool) -> bool {
    match setting {
        Setting::Off => false,
        Setting::On => !portal_denies(),
        Setting::OverridePortalDeny => true,
    }
}

/// A screenshot Deny saved in xdg-desktop-portal's permission store, where
/// unsandboxed apps share the empty app id. KWin's own capture API never
/// consults it, so the driver does. Only a store that answers, or that doesn't
/// exist, rules a Deny out: a lookup that fails any other way counts as one.
fn portal_denies_screenshots() -> bool {
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, denied)) = *last {
        if at.elapsed() < PORTAL_ANSWER_TTL {
            return denied;
        }
    }
    let denied = super::kwin_capture::run(async {
        let lookup = async {
            let connection = zbus::Connection::session().await?;
            let store = zbus::Proxy::new(
                &connection,
                "org.freedesktop.impl.portal.PermissionStore",
                "/org/freedesktop/impl/portal/PermissionStore",
                "org.freedesktop.impl.portal.PermissionStore",
            )
            .await?;
            store
                .call::<_, _, (HashMap<String, Vec<String>>, zbus::zvariant::OwnedValue)>(
                    "Lookup",
                    &("screenshot", "screenshot"),
                )
                .await
        };
        Ok(match tokio::time::timeout(PORTAL_TIMEOUT, lookup).await {
            Ok(Ok((permissions, _))) => denies(&permissions),
            Ok(Err(zbus::Error::MethodError(name, ..))) => !no_saved_answer(name.as_str()),
            _ => true,
        })
    })
    .unwrap_or(true);
    *last = Some((Instant::now(), denied));
    denied
}

/// The store has no screenshot entry, or there is no store to have saved one.
fn no_saved_answer(error: &str) -> bool {
    matches!(
        error,
        "org.freedesktop.portal.Error.NotFound"
            | "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
    )
}

fn denies(permissions: &HashMap<String, Vec<String>>) -> bool {
    permissions
        .get("")
        .and_then(|values| values.first())
        .is_some_and(|value| value == "no")
}

/// Save `setting`. On KWin 6.7 and older the desktop entry is what lets this
/// executable reach window management and ScreenShot2, so it is written for
/// on and removed for off; KWin 6.8 and newer don't check it.
pub fn save(setting: Setting) -> anyhow::Result<()> {
    let entry = desktop_entry_path().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let saved = setting_path().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    if setting == Setting::Off {
        replace(&saved, setting.as_str())?;
        match std::fs::remove_file(&entry) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    } else {
        let exe = std::env::current_exe()?.canonicalize()?;
        replace(&entry, &desktop_entry(&exe)?)?;
        replace(&saved, setting.as_str())?;
    }
    refresh_service_cache();
    Ok(())
}

/// Write through a sibling file and rename, so a reader never sees half of it.
fn replace(path: &Path, text: &str) -> anyhow::Result<()> {
    let dir = path.parent().expect("a file under a directory");
    std::fs::create_dir_all(dir)?;
    let partial = dir.join(format!(".{}.partial", std::process::id()));
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn setting_path() -> Option<PathBuf> {
    pip_preview::default_config_path().map(|config| config.with_file_name(SETTING_FILE))
}

/// What `cua-driver kwin-capture status` prints.
pub fn report() -> String {
    let setting = Setting::saved();
    let denied = setting == Setting::On && portal_denies_screenshots();
    let entry = desktop_entry_path().filter(|path| path.exists());
    let current = entry.as_deref().is_some_and(|path| {
        let exe = std::env::current_exe().and_then(|exe| exe.canonicalize());
        let text = std::fs::read_to_string(path);
        matches!((exe, text), (Ok(exe), Ok(text)) if desktop_entry(&exe).is_ok_and(|entry| entry == text))
    });
    let mut lines = vec![format!("kwin window capture: {}", setting.as_str())];
    if denied {
        lines.push(
            "kept off: the desktop portal has a saved screenshot Deny, or its permission \
             store did not answer; run `cua-driver kwin-capture on --override-portal-deny` \
             to ignore it"
                .into(),
        );
    }
    lines.push(match (entry, current) {
        (Some(path), true) => format!("desktop entry: {}", path.display()),
        (Some(path), false) => format!(
            "desktop entry: {} names another executable; run `cua-driver kwin-capture on` again",
            path.display()
        ),
        (None, _) => "desktop entry: none".into(),
    });
    lines.join("\n") + "\n"
}

fn desktop_entry_path() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|data| data.join("applications").join(DESKTOP_ENTRY))
}

/// KWin matches the entry's `Exec` program to the client's executable.
fn desktop_entry(exe: &Path) -> anyhow::Result<String> {
    let exe = exe.to_str().unwrap_or("\"");
    anyhow::ensure!(
        !exe.contains(['"', '\\', '$', '`', '%', '\n']),
        "the cua-driver path can't be written to a desktop entry: {exe}"
    );
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=Cua Driver\nExec=\"{exe}\" serve\nNoDisplay=true\n\
         X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,org_kde_kwin_keystate\n\
         X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n"
    ))
}

/// KWin reads desktop entries through KSycoca; rebuild it so a change applies now.
fn refresh_service_cache() {
    let Ok(mut child) = std::process::Command::new("kbuildsycoca6")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_the_operator_turned_it_on_and_a_saved_deny_holds_unless_overridden() {
        let unasked = || -> bool { panic!("the portal is not consulted") };
        assert!(!allowed_with(Setting::Off, unasked));
        assert!(allowed_with(Setting::On, || false));
        assert!(!allowed_with(Setting::On, || true));
        assert!(allowed_with(Setting::OverridePortalDeny, unasked));
        // Anything but a saved value reads as off.
        assert_eq!(Setting::parse("true"), None);
        for setting in [Setting::Off, Setting::On, Setting::OverridePortalDeny] {
            assert_eq!(Setting::parse(setting.as_str()), Some(setting));
        }
    }

    #[test]
    fn only_a_saved_no_for_unsandboxed_apps_is_a_deny() {
        let store = |app: &str, value: &str| HashMap::from([(app.into(), vec![value.into()])]);
        assert!(denies(&store("", "no")));
        assert!(!denies(&store("", "yes")));
        assert!(!denies(&store("", "ask")));
        assert!(!denies(&store("org.example.App", "no")));
        assert!(!denies(&HashMap::new()));
        // A store that can't answer may be hiding a Deny.
        assert!(no_saved_answer("org.freedesktop.portal.Error.NotFound"));
        assert!(no_saved_answer("org.freedesktop.DBus.Error.ServiceUnknown"));
        assert!(!no_saved_answer("org.freedesktop.DBus.Error.NoReply"));
        assert!(!no_saved_answer("org.freedesktop.DBus.Error.AccessDenied"));
    }

    #[test]
    fn the_desktop_entry_names_the_executable_kwin_will_match() {
        let entry = desktop_entry(Path::new("/opt/cua driver/cua-driver")).unwrap();
        assert!(entry.contains("\nExec=\"/opt/cua driver/cua-driver\" serve\n"));
        assert!(entry.contains("X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n"));
        // KWin compares the raw path, so a character Exec would escape can't be written.
        assert!(desktop_entry(Path::new("/opt/$x/cua-driver")).is_err());
        assert!(desktop_entry(Path::new("/opt/100%/cua-driver")).is_err());
    }
}
