// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which xdg-desktop-portal backend serves RemoteDesktop.
//!
//! The portal frontend (`org.freedesktop.portal.Desktop`) can be reachable
//! while the backend it routes RemoteDesktop to does not export
//! `org.freedesktop.impl.portal.RemoteDesktop`. That happens when
//! `xdg-desktop-portal-kde` is picked inside a GNOME (Mutter) session: every
//! input call then fails with "No such interface
//! 'org.freedesktop.impl.portal.RemoteDesktop'" (#3638). Doctor resolves the
//! backend the way the frontend does, from `portals.conf` and the installed
//! `.portal` files, and asks that backend whether it exports the interface.

use std::path::{Path, PathBuf};

pub(crate) const REMOTE_DESKTOP_IMPL: &str = "org.freedesktop.impl.portal.RemoteDesktop";
const BACKEND_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

/// One installed `<name>.portal` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortalFile {
    pub name: String,
    pub dbus_name: String,
    pub interfaces: Vec<String>,
    pub use_in: Vec<String>,
}

impl PortalFile {
    fn declares(&self, interface: &str) -> bool {
        self.interfaces.iter().any(|i| i == interface)
    }
}

/// The `portals.conf` the frontend reads, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortalConf {
    None,
    Read { path: PathBuf, text: String },
    Unreadable { path: PathBuf, error: String },
}

/// Where RemoteDesktop calls go, and whether that backend can take them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Served {
        backend: String,
    },
    /// The chosen backend is running but does not export the interface.
    NotExported {
        backend: String,
        conf: Option<PathBuf>,
    },
    /// The config names no installed backend that declares RemoteDesktop,
    /// or says `none`.
    NoBackend {
        conf: Option<PathBuf>,
    },
    UnreadableConf {
        path: PathBuf,
        error: String,
    },
    /// The backend could not be asked (not running and not activatable,
    /// timeout). Doctor keeps its earlier verdict.
    Unknown,
}

impl Route {
    /// `(message, hint)` when input through the portal is known to fail.
    pub(crate) fn problem(&self, desktops: &[String]) -> Option<(String, String)> {
        let pin = pin_hint(desktops);
        match self {
            Route::Served { .. } | Route::Unknown => None,
            Route::NotExported { backend, conf } => Some((
                format!(
                    "xdg-desktop-portal routes RemoteDesktop to the {backend} backend{}, \
                     but xdg-desktop-portal-{backend} does not export {REMOTE_DESKTOP_IMPL} \
                     in this session, so every click and key press fails with \
                     \"No such interface\".",
                    conf_note(conf.as_deref()),
                ),
                pin,
            )),
            Route::NoBackend { conf } => Some((
                format!(
                    "xdg-desktop-portal has no backend for RemoteDesktop{}: no \
                     installed backend it allows declares {REMOTE_DESKTOP_IMPL}, so \
                     clicks and key presses cannot be delivered.",
                    conf_note(conf.as_deref()),
                ),
                pin,
            )),
            Route::UnreadableConf { path, error } => Some((
                format!(
                    "{} exists but cannot be read ({error}), so xdg-desktop-portal \
                     cannot apply the backend it pins.",
                    path.display()
                ),
                format!(
                    "Make {} readable (for example `chmod 644 {}`), then restart \
                     xdg-desktop-portal or log out and back in.",
                    path.display(),
                    path.display()
                ),
            )),
        }
    }
}

fn conf_note(conf: Option<&Path>) -> String {
    match conf {
        Some(path) => format!(" (per {})", path.display()),
        None => String::new(),
    }
}

fn pin_hint(desktops: &[String]) -> String {
    let backend = desktops
        .first()
        .map(String::as_str)
        .filter(|d| !d.is_empty())
        .unwrap_or("<desktop>");
    format!(
        "Pin the backend for this desktop in ~/.config/xdg-desktop-portal/portals.conf \
         (`[preferred]` then `default={backend}`), make the file readable (mode 644), \
         then restart xdg-desktop-portal or log out and back in."
    )
}

/// Parse a `.portal` file's `[portal]` group. `name` is the file stem.
pub(crate) fn parse_portal_file(name: &str, text: &str) -> Option<PortalFile> {
    let group = key_file_group(text, "portal");
    let get = |key: &str| group.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    Some(PortalFile {
        name: name.to_owned(),
        dbus_name: get("DBusName")?,
        interfaces: split_list(&get("Interfaces").unwrap_or_default()),
        use_in: split_list(&get("UseIn").unwrap_or_default())
            .into_iter()
            .map(|d| d.to_ascii_lowercase())
            .collect(),
    })
}

/// The backend list `portals.conf` gives `interface`: its own key, else
/// `default`. `None` when the file says nothing about it.
pub(crate) fn preferred_backends(conf: &str, interface: &str) -> Option<Vec<String>> {
    let group = key_file_group(conf, "preferred");
    let value = |key: &str| group.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    value(interface)
        .or_else(|| value("default"))
        .map(|v| split_list(v))
}

/// The backend the frontend picks for `interface`. With a config, its list
/// is tried in order (`*` is any backend, `none` stops). Without one, a
/// backend whose `UseIn` names the current desktop wins, then any backend.
pub(crate) fn choose_backend<'a>(
    preferred: Option<&[String]>,
    portals: &'a [PortalFile],
    desktops: &[String],
    interface: &str,
) -> Option<&'a PortalFile> {
    let mut candidates: Vec<&PortalFile> =
        portals.iter().filter(|p| p.declares(interface)).collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    match preferred {
        Some(list) => {
            for entry in list {
                match entry.as_str() {
                    "none" => return None,
                    "*" => return candidates.first().copied(),
                    name => {
                        if let Some(p) = candidates.iter().find(|p| p.name == name) {
                            return Some(p);
                        }
                    }
                }
            }
            None
        }
        None => candidates
            .iter()
            .find(|p| p.use_in.iter().any(|u| desktops.contains(u)))
            .or_else(|| candidates.first())
            .copied(),
    }
}

/// Resolve where RemoteDesktop goes. `exports` asks a running backend
/// whether it exports the interface; `None` means it could not be asked.
pub(crate) fn resolve(
    conf: &PortalConf,
    portals: &[PortalFile],
    desktops: &[String],
    exports: impl FnOnce(&PortalFile) -> Option<bool>,
) -> Route {
    let (preferred, conf_path) = match conf {
        PortalConf::Unreadable { path, error } => {
            return Route::UnreadableConf {
                path: path.clone(),
                error: error.clone(),
            }
        }
        PortalConf::Read { path, text } => match preferred_backends(text, REMOTE_DESKTOP_IMPL) {
            Some(list) => (Some(list), Some(path.clone())),
            None => (None, None),
        },
        PortalConf::None => (None, None),
    };
    if portals.is_empty() {
        return Route::Unknown;
    }
    let Some(backend) =
        choose_backend(preferred.as_deref(), portals, desktops, REMOTE_DESKTOP_IMPL)
    else {
        return Route::NoBackend { conf: conf_path };
    };
    match exports(backend) {
        Some(true) => Route::Served {
            backend: backend.name.clone(),
        },
        Some(false) => Route::NotExported {
            backend: backend.name.clone(),
            conf: conf_path,
        },
        None => Route::Unknown,
    }
}

/// Lowercased `XDG_CURRENT_DESKTOP` entries, as the frontend matches them.
pub(crate) fn current_desktops() -> Vec<String> {
    split_list(
        &std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .replace(':', ";"),
    )
    .into_iter()
    .map(|d| d.to_ascii_lowercase())
    .collect()
}

/// The frontend's config search: in each directory, `<desktop>-portals.conf`
/// for each current desktop, then `portals.conf`. The first file found wins.
pub(crate) fn conf_candidates(dirs: &[PathBuf], desktops: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        for desktop in desktops {
            out.push(dir.join(format!("{desktop}-portals.conf")));
        }
        out.push(dir.join("portals.conf"));
    }
    out
}

pub(crate) fn read_conf(candidates: &[PathBuf]) -> PortalConf {
    for path in candidates {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                return PortalConf::Read {
                    path: path.clone(),
                    text,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return PortalConf::Unreadable {
                    path: path.clone(),
                    error: e.to_string(),
                }
            }
        }
    }
    PortalConf::None
}

/// Installed `.portal` files, first directory wins for a name.
pub(crate) fn read_portal_files(dirs: &[PathBuf]) -> Vec<PortalFile> {
    let mut out: Vec<PortalFile> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("portal") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if out.iter().any(|p| p.name == name) {
                continue;
            }
            if let Some(file) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_portal_file(name, &text))
            {
                out.push(file);
            }
        }
    }
    out
}

fn env_dirs(var: &str, default: &str) -> Vec<PathBuf> {
    let value = std::env::var(var).ok().filter(|v| !v.is_empty());
    value
        .as_deref()
        .unwrap_or(default)
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Config directories in the frontend's order: XDG_CONFIG_HOME,
/// XDG_CONFIG_DIRS, /etc, XDG_DATA_HOME, XDG_DATA_DIRS.
pub(crate) fn conf_dirs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(dirs::config_dir());
    roots.extend(env_dirs("XDG_CONFIG_DIRS", "/etc/xdg"));
    roots.push(PathBuf::from("/etc"));
    roots.extend(dirs::data_dir());
    roots.extend(env_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    roots
        .into_iter()
        .map(|r| r.join("xdg-desktop-portal"))
        .collect()
}

pub(crate) fn portal_file_dirs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(dirs::data_dir());
    roots.extend(env_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    roots
        .into_iter()
        .map(|r| r.join("xdg-desktop-portal").join("portals"))
        .collect()
}

/// Introspect the backend's portal object for `interface`.
pub(crate) async fn backend_exports(
    connection: &zbus::Connection,
    dbus_name: &str,
    interface: &str,
) -> Option<bool> {
    let proxy = zbus::fdo::IntrospectableProxy::builder(connection)
        .destination(dbus_name.to_owned())
        .ok()?
        .path(BACKEND_OBJECT_PATH)
        .ok()?
        .build()
        .await
        .ok()?;
    let xml = proxy.introspect().await.ok()?;
    Some(xml.contains(&format!("\"{interface}\"")))
}

/// Read the config and `.portal` files, then introspect the chosen backend.
pub(crate) fn probe() -> Route {
    let desktops = current_desktops();
    let conf = read_conf(&conf_candidates(&conf_dirs(), &desktops));
    let portals = read_portal_files(&portal_file_dirs());
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Route::Unknown;
    };
    resolve(&conf, &portals, &desktops, |backend| {
        rt.block_on(async {
            tokio::time::timeout(super::portal::PROBE_TIMEOUT, async {
                let connection = super::portal::fresh_session_connection().await.ok()?;
                backend_exports(&connection, &backend.dbus_name, REMOTE_DESKTOP_IMPL).await
            })
            .await
            .ok()
            .flatten()
        })
    })
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `key=value` pairs of one `[group]` in a GKeyFile-style file.
fn key_file_group(text: &str, group: &str) -> Vec<(String, String)> {
    let header = format!("[{group}]");
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if inside {
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GNOME_PORTAL: &str = "[portal]\n\
        DBusName=org.freedesktop.impl.portal.desktop.gnome\n\
        Interfaces=org.freedesktop.impl.portal.Screenshot;org.freedesktop.impl.portal.RemoteDesktop;\n\
        UseIn=gnome\n";
    const KDE_PORTAL: &str = "[portal]\n\
        DBusName=org.freedesktop.impl.portal.desktop.kde\n\
        Interfaces=org.freedesktop.impl.portal.RemoteDesktop;org.freedesktop.impl.portal.Screenshot;\n\
        UseIn=KDE\n";
    const GTK_PORTAL: &str = "[portal]\n\
        DBusName=org.freedesktop.impl.portal.desktop.gtk\n\
        Interfaces=org.freedesktop.impl.portal.FileChooser;\n\
        UseIn=gnome\n";

    fn portals() -> Vec<PortalFile> {
        vec![
            parse_portal_file("gnome", GNOME_PORTAL).unwrap(),
            parse_portal_file("kde", KDE_PORTAL).unwrap(),
            parse_portal_file("gtk", GTK_PORTAL).unwrap(),
        ]
    }

    fn gnome() -> Vec<String> {
        vec!["gnome".into()]
    }

    fn conf(text: &str) -> PortalConf {
        PortalConf::Read {
            path: PathBuf::from("/home/u/.config/xdg-desktop-portal/portals.conf"),
            text: text.into(),
        }
    }

    /// Only the gnome backend exports RemoteDesktop under Mutter.
    fn mutter(backend: &PortalFile) -> Option<bool> {
        Some(backend.name == "gnome")
    }

    #[test]
    fn a_kde_pin_in_a_gnome_session_names_the_backend_that_lacks_remote_desktop() {
        let route = resolve(
            &conf("[preferred]\ndefault=kde\n"),
            &portals(),
            &gnome(),
            mutter,
        );
        assert_eq!(
            route,
            Route::NotExported {
                backend: "kde".into(),
                conf: Some(PathBuf::from(
                    "/home/u/.config/xdg-desktop-portal/portals.conf"
                )),
            }
        );
        let (message, hint) = route.problem(&gnome()).unwrap();
        assert!(
            message.contains("routes RemoteDesktop to the kde backend")
                && message.contains("No such interface"),
            "{message}"
        );
        assert!(hint.contains("default=gnome"), "{hint}");
    }

    #[test]
    fn an_unconfigured_gnome_session_prefers_the_backend_used_in_gnome() {
        let route = resolve(&PortalConf::None, &portals(), &gnome(), mutter);
        assert_eq!(
            route,
            Route::Served {
                backend: "gnome".into()
            }
        );
        assert!(route.problem(&gnome()).is_none());
    }

    #[test]
    fn a_gnome_pin_routes_to_gnome() {
        let text = "[preferred]\ndefault=gnome\norg.freedesktop.impl.portal.Secret=gnome-keyring\n";
        let route = resolve(&conf(text), &portals(), &gnome(), mutter);
        assert_eq!(
            route,
            Route::Served {
                backend: "gnome".into()
            }
        );
    }

    #[test]
    fn an_interface_key_overrides_default_and_skips_backends_without_it() {
        let text = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.RemoteDesktop=gtk;kde\n";
        let route = resolve(&conf(text), &portals(), &gnome(), mutter);
        assert!(
            matches!(&route, Route::NotExported { backend, .. } if backend == "kde"),
            "{route:?}"
        );
    }

    #[test]
    fn none_leaves_remote_desktop_without_a_backend() {
        let text = "[preferred]\ndefault=gnome\norg.freedesktop.impl.portal.RemoteDesktop=none\n";
        let route = resolve(&conf(text), &portals(), &gnome(), mutter);
        assert!(matches!(route, Route::NoBackend { .. }), "{route:?}");
        assert!(route.problem(&gnome()).is_some());
    }

    #[test]
    fn an_unreadable_conf_is_reported_with_its_path() {
        let path = PathBuf::from("/home/u/.config/xdg-desktop-portal/portals.conf");
        let route = resolve(
            &PortalConf::Unreadable {
                path: path.clone(),
                error: "Permission denied (os error 13)".into(),
            },
            &portals(),
            &gnome(),
            mutter,
        );
        let (message, hint) = route.problem(&gnome()).unwrap();
        assert!(
            message.contains(&path.display().to_string()) && message.contains("Permission denied"),
            "{message}"
        );
        assert!(hint.contains("chmod 644"), "{hint}");
    }

    #[test]
    fn a_backend_that_cannot_be_asked_keeps_the_earlier_verdict() {
        let route = resolve(&PortalConf::None, &portals(), &gnome(), |_| None);
        assert_eq!(route, Route::Unknown);
        assert!(route.problem(&gnome()).is_none());
    }

    #[test]
    fn conf_search_tries_the_desktop_file_before_portals_conf_in_each_directory() {
        let dirs = [PathBuf::from("/a"), PathBuf::from("/b")];
        let desktops = vec!["ubuntu".to_owned(), "gnome".to_owned()];
        assert_eq!(
            conf_candidates(&dirs, &desktops),
            [
                "/a/ubuntu-portals.conf",
                "/a/gnome-portals.conf",
                "/a/portals.conf",
                "/b/ubuntu-portals.conf",
                "/b/gnome-portals.conf",
                "/b/portals.conf",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn read_conf_takes_the_first_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("gnome-portals.conf");
        let second = dir.path().join("portals.conf");
        std::fs::write(&second, "[preferred]\ndefault=kde\n").unwrap();
        assert_eq!(
            read_conf(&[first.clone(), second.clone()]),
            PortalConf::Read {
                path: second,
                text: "[preferred]\ndefault=kde\n".into()
            }
        );
    }
}
