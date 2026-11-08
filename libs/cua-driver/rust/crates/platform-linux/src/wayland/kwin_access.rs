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
        replace(&entry, &desktop_entry(&entry_exe()?)?)?;
        replace(&saved, setting.as_str())?;
    }
    refresh_service_cache();
    if setting != Setting::Off && super::kwin_capture::may_be_session() {
        wait_until_kwin_lists_windows();
    }
    Ok(())
}

/// KWin rereads its desktop-entry cache on its own schedule after
/// `kbuildsycoca6` exits, so `on` returns once a fresh connection is granted.
fn wait_until_kwin_lists_windows() {
    let deadline = Instant::now() + Duration::from_secs(5);
    while super::kwin_capture::list_windows().is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The executable the entry names. A release install's `packages/current` link
/// is named instead of the versioned release it points at, so the grant
/// survives updates: KWin resolves `Exec` when it checks.
fn entry_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe()?.canonicalize().map(stable_exe)
}

fn stable_exe(exe: PathBuf) -> PathBuf {
    let current = exe
        .parent()
        .and_then(Path::parent)
        .filter(|releases| releases.file_name().is_some_and(|name| name == "releases"))
        .and_then(Path::parent)
        .zip(exe.file_name())
        .map(|(packages, name)| packages.join("current").join(name));
    current
        .filter(|current| current.canonicalize().is_ok_and(|target| target == exe))
        .unwrap_or(exe)
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

/// The switch, whether a saved portal Deny keeps it off, and what the desktop
/// entry names: what `cua-driver kwin-capture status`, the installer and the
/// health report all read, so none of them guesses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub setting: Setting,
    /// Switched on, but a saved screenshot Deny (or a portal permission store
    /// that did not answer) keeps the adapter off.
    pub kept_off: bool,
    pub entry: Entry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    None,
    /// Names this executable: KWin 6.7 and older grant it.
    Current(PathBuf),
    /// Names another executable: KWin 6.7 and older refuse this one.
    Other(PathBuf),
    /// Names this executable without the interfaces capture needs.
    Ungranted(PathBuf),
}

impl Status {
    /// Whether the KWin adapter runs: the same answer as [`allowed`].
    pub fn on(&self) -> bool {
        self.setting != Setting::Off && !self.kept_off
    }
}

pub fn status() -> Status {
    let setting = Setting::saved();
    let kept_off = setting == Setting::On && portal_denies_screenshots();
    let entry = match desktop_entry_path().filter(|path| path.exists()) {
        None => Entry::None,
        Some(path) => match (entry_exe(), std::fs::read_to_string(&path)) {
            (Ok(exe), Ok(text)) => classify(path, &text, &exe),
            _ => Entry::Other(path),
        },
    };
    Status {
        setting,
        kept_off,
        entry,
    }
}

/// What `cua-driver kwin-capture status` prints.
pub fn report() -> String {
    describe(&status())
}

fn describe(status: &Status) -> String {
    let mut lines = vec![format!("kwin window capture: {}", status.setting.as_str())];
    if status.kept_off {
        lines.push(
            "kept off: the desktop portal has a saved screenshot Deny, or its permission \
             store did not answer; run `cua-driver kwin-capture on --override-portal-deny` \
             to ignore it"
                .into(),
        );
    }
    lines.push(match &status.entry {
        Entry::Current(path) => format!("desktop entry: {}", path.display()),
        Entry::Other(path) => format!(
            "desktop entry: {} names another executable; run `cua-driver kwin-capture on` again",
            path.display()
        ),
        Entry::Ungranted(path) => format!(
            "desktop entry: {} doesn't grant window management and ScreenShot2; run \
             `cua-driver kwin-capture on` again",
            path.display()
        ),
        Entry::None => "desktop entry: none".into(),
    });
    lines.join("\n") + "\n"
}

/// The grants capture needs, by the key KWin 6.7 and older read them from.
/// `org_kde_kwin_keystate` is left out: only typing uses it.
const CAPTURE_GRANTS: [(&str, &str); 2] = [
    (
        "X-KDE-Wayland-Interfaces",
        "org_kde_plasma_window_management",
    ),
    (
        "X-KDE-DBUS-Restricted-Interfaces",
        "org.kde.KWin.ScreenShot2",
    ),
];

/// Judge an existing entry by what KWin reads from it, the `Exec` program and
/// the granted interfaces, so an edited `Name` or comment still counts.
pub(crate) fn classify(path: PathBuf, text: &str, exe: &Path) -> Entry {
    let found = fields(text);
    let program = exec_program(&found).map(Path::new);
    if !program.is_some_and(|program| same_file(program, exe)) {
        return Entry::Other(path);
    }
    let granted = CAPTURE_GRANTS.iter().all(|(key, interface)| {
        found
            .get(key)
            .is_some_and(|list| list.split(',').any(|item| item.trim() == *interface))
    });
    if granted {
        Entry::Current(path)
    } else {
        Entry::Ungranted(path)
    }
}

/// KWin resolves links when it matches `Exec`, so two paths to one file match.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// The keys of the `[Desktop Entry]` group.
fn fields(text: &str) -> HashMap<&str, &str> {
    text.lines()
        .map(str::trim)
        .skip_while(|line| *line != "[Desktop Entry]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect()
}

/// The program the `Exec` line runs: its first, possibly quoted, word.
fn exec_program<'a>(fields: &HashMap<&str, &'a str>) -> Option<&'a str> {
    let exec = fields.get("Exec")?;
    Some(match exec.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => exec.split_whitespace().next().unwrap_or(""),
    })
}

fn desktop_entry_path() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|data| data.join("applications").join(DESKTOP_ENTRY))
}

/// KWin matches the entry's `Exec` program to the client's executable.
pub(crate) fn desktop_entry(exe: &Path) -> anyhow::Result<String> {
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
    fn the_status_names_the_switch_a_kept_off_deny_and_the_entry() {
        let entry = PathBuf::from("/home/u/.local/share/applications/ai.cua.driver.desktop");
        let on = Status {
            setting: Setting::On,
            kept_off: false,
            entry: Entry::Current(entry.clone()),
        };
        assert!(on.on());
        assert_eq!(
            describe(&on),
            format!(
                "kwin window capture: on\ndesktop entry: {}\n",
                entry.display()
            )
        );
        let kept_off = Status {
            kept_off: true,
            ..on.clone()
        };
        assert!(!kept_off.on());
        assert!(describe(&kept_off).contains("\nkept off: the desktop portal"));
        let other = Status {
            entry: Entry::Other(entry.clone()),
            ..on.clone()
        };
        assert!(other.on());
        assert!(describe(&other).contains("names another executable"));
        let ungranted = Status {
            entry: Entry::Ungranted(entry.clone()),
            ..on.clone()
        };
        assert!(describe(&ungranted).contains("doesn't grant window management and ScreenShot2"));
        let off = Status {
            setting: Setting::Off,
            kept_off: false,
            entry: Entry::None,
        };
        assert!(!off.on());
        assert_eq!(
            describe(&off),
            "kwin window capture: off\ndesktop entry: none\n"
        );
        let overridden = Status {
            setting: Setting::OverridePortalDeny,
            ..off
        };
        assert!(overridden.on());
    }

    #[test]
    fn an_entry_is_judged_by_its_exec_program_and_grants_not_its_whole_text() {
        let exe = Path::new("/opt/cua driver/cua-driver");
        let path = PathBuf::from("/home/u/.local/share/applications/ai.cua.driver.desktop");
        let written = desktop_entry(exe).unwrap();
        let judge = |text: &str| classify(path.clone(), text, exe);
        assert_eq!(judge(&written), Entry::Current(path.clone()));
        // Edits KWin doesn't read keep the grant.
        let renamed = written.replace("Name=Cua Driver", "Name=Mine\n# a note\nComment=x");
        assert_eq!(judge(&renamed), Entry::Current(path.clone()));
        let reordered = written.replace(
            "org_kde_plasma_window_management,org_kde_kwin_keystate",
            "org_kde_kwin_keystate, org_kde_plasma_window_management",
        );
        assert_eq!(judge(&reordered), Entry::Current(path.clone()));
        // Another program, quoted or not, is another executable.
        let moved = written.replace("/opt/cua driver/cua-driver", "/usr/bin/cua-driver");
        assert_eq!(judge(&moved), Entry::Other(path.clone()));
        let unquoted = written.replace("\"/opt/cua driver/cua-driver\"", "/opt/cua");
        assert_eq!(judge(&unquoted), Entry::Other(path.clone()));
        assert_eq!(judge(""), Entry::Other(path.clone()));
        // This program without the interfaces capture needs is not granted.
        let no_screenshot = written.replace("=org.kde.KWin.ScreenShot2", "=");
        assert_eq!(judge(&no_screenshot), Entry::Ungranted(path.clone()));
        let no_windows = written.replace("org_kde_plasma_window_management,", "");
        assert_eq!(judge(&no_windows), Entry::Ungranted(path.clone()));
        let no_keystate = written.replace(",org_kde_kwin_keystate", "");
        assert_eq!(judge(&no_keystate), Entry::Current(path.clone()));
        // Keys outside the [Desktop Entry] group don't count.
        let other_group = written.replace(
            "X-KDE-DBUS-Restricted-Interfaces",
            "[Desktop Action x]\nX-KDE-DBUS-Restricted-Interfaces",
        );
        assert_eq!(judge(&other_group), Entry::Ungranted(path.clone()));
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

    #[cfg(unix)]
    #[test]
    fn an_entry_naming_a_link_to_this_executable_names_this_executable() {
        let root = std::env::temp_dir().join(format!("kwin-entry-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let (exe, link, other) = (
            root.join("cua-driver"),
            root.join("link"),
            root.join("other"),
        );
        std::fs::write(&exe, "").unwrap();
        std::fs::write(&other, "").unwrap();
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let path = root.join("ai.cua.driver.desktop");
        let entry = |named: &Path| desktop_entry(named).unwrap();
        assert_eq!(
            classify(path.clone(), &entry(&link), &exe),
            Entry::Current(path.clone())
        );
        assert_eq!(
            classify(path.clone(), &entry(&exe), &link),
            Entry::Current(path.clone())
        );
        assert_eq!(
            classify(path.clone(), &entry(&other), &exe),
            Entry::Other(path.clone())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_release_install_names_the_current_link_so_updates_keep_the_grant() {
        let root = std::env::temp_dir().join(format!("kwin-access-{}", std::process::id()));
        std::fs::create_dir_all(root.join("packages/releases/1.0.0-x86_64")).unwrap();
        let packages = root.canonicalize().unwrap().join("packages");
        let exe = packages.join("releases/1.0.0-x86_64/cua-driver");
        std::fs::write(&exe, "").unwrap();
        // No current link yet, or one pointing at another release: the binary itself.
        assert_eq!(stable_exe(exe.clone()), exe);
        std::os::unix::fs::symlink("releases/0.9.0-x86_64", packages.join("current")).unwrap();
        assert_eq!(stable_exe(exe.clone()), exe);
        std::fs::remove_file(packages.join("current")).unwrap();
        std::os::unix::fs::symlink("releases/1.0.0-x86_64", packages.join("current")).unwrap();
        assert_eq!(stable_exe(exe.clone()), packages.join("current/cua-driver"));
        // A binary outside a release layout is named as is.
        let system = PathBuf::from("/usr/bin/cua-driver");
        assert_eq!(stable_exe(system.clone()), system);
        std::fs::remove_dir_all(root).unwrap();
    }
}
