// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which xdg-desktop-portal backend serves RemoteDesktop.
//!
//! The portal frontend (`org.freedesktop.portal.Desktop`) can be reachable
//! while the backend it routes RemoteDesktop to does not export
//! `org.freedesktop.impl.portal.RemoteDesktop`. That happens when
//! `xdg-desktop-portal-kde` is picked inside a GNOME (Mutter) session: every
//! input call then fails with "No such interface
//! 'org.freedesktop.impl.portal.RemoteDesktop'" (#3638). Doctor picks the
//! backend the way the frontend does (`src/xdp-portal-impl.c` in
//! xdg-desktop-portal 1.20), from `portals.conf` and the installed `.portal`
//! files, and asks that backend whether it exports the interface.

use std::path::{Path, PathBuf};

pub(crate) const REMOTE_DESKTOP_IMPL: &str = "org.freedesktop.impl.portal.RemoteDesktop";
const BACKEND_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const GTK_BACKEND: &str = "org.freedesktop.impl.portal.desktop.gtk";

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

    fn used_in(&self, desktop: &str) -> bool {
        self.use_in.iter().any(|u| u.eq_ignore_ascii_case(desktop))
    }
}

/// The `[preferred]` group of the `portals.conf` the frontend uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortalConf {
    pub path: PathBuf,
    pub interfaces: Vec<(String, Vec<String>)>,
    pub default: Option<Vec<String>>,
}

impl PortalConf {
    fn interface(&self, interface: &str) -> Option<&[String]> {
        self.interfaces
            .iter()
            .find(|(k, _)| k == interface)
            .map(|(_, v)| v.as_slice())
    }
}

/// The config search result: the file in use, plus files that exist but
/// could not be read, which the frontend skips.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConfSearch {
    pub active: Option<PortalConf>,
    pub unreadable: Vec<(PathBuf, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice<'a> {
    Backend(&'a PortalFile),
    /// The config says `none`.
    Vetoed,
    None,
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
    },
    /// The config sets RemoteDesktop to `none`.
    Vetoed,
    /// No installed backend the frontend would pick declares RemoteDesktop.
    NoBackend,
    /// No `.portal` files found, or the backend could not be asked (not
    /// running and not activatable, timeout). Doctor keeps its earlier verdict.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diagnosis {
    pub route: Route,
    pub conf: Option<PathBuf>,
    pub unreadable: Vec<(PathBuf, String)>,
    /// Another installed backend that declares RemoteDesktop.
    pub suggest: Option<String>,
    /// The highest-priority config file: `<desktop>-portals.conf` in the
    /// user's config directory.
    pub pin_file: PathBuf,
}

impl Diagnosis {
    /// `(message, hint)` when input through the portal is known to fail.
    pub(crate) fn problem(&self) -> Option<(String, String)> {
        let message = match &self.route {
            Route::Served { .. } | Route::Unknown => return None,
            Route::NotExported { backend } => format!(
                "xdg-desktop-portal routes RemoteDesktop to the {backend} backend{}, but \
                 xdg-desktop-portal-{backend} does not export {REMOTE_DESKTOP_IMPL} in \
                 this session, so every click and key press fails with \"No such interface\".",
                self.conf_note(),
            ),
            Route::Vetoed => format!(
                "xdg-desktop-portal is configured with RemoteDesktop set to `none`{}, so \
                 clicks and key presses cannot be delivered.",
                self.conf_note(),
            ),
            Route::NoBackend => format!(
                "xdg-desktop-portal has no backend for RemoteDesktop{}: none it would pick \
                 declares {REMOTE_DESKTOP_IMPL}, so clicks and key presses cannot be delivered.",
                self.conf_note(),
            ),
        };
        let skipped: String = self
            .unreadable
            .iter()
            .map(|(path, error)| {
                format!(
                    " xdg-desktop-portal skips {} because it cannot be read ({error}).",
                    path.display()
                )
            })
            .collect();
        let backend = self.suggest.as_deref().unwrap_or("<backend>");
        let mut hint = format!(
            "Under `[preferred]` in {}, set `{REMOTE_DESKTOP_IMPL}={backend}`",
            self.pin_file.display()
        );
        if let Some(conf) = self.conf.as_ref().filter(|c| **c != self.pin_file) {
            hint.push_str(&format!(
                " (that file replaces {}, so copy its other settings)",
                conf.display()
            ));
        }
        hint.push_str(
            ". Keep the file readable (mode 644), then restart xdg-desktop-portal or log \
             out and back in.",
        );
        Some((format!("{message}{skipped}"), hint))
    }

    fn conf_note(&self) -> String {
        match &self.conf {
            Some(path) => format!(" (per {})", path.display()),
            None => String::new(),
        }
    }
}

/// Parse a `.portal` file's `[portal]` group. `name` is the file stem.
/// `None` when the frontend would reject the file.
pub(crate) fn parse_portal_file(name: &str, text: &str) -> Option<PortalFile> {
    let groups = parse_key_file(text)?;
    let group = key_file_group(&groups, "portal");
    let get = |key: &str| {
        group
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    Some(PortalFile {
        name: name.to_owned(),
        dbus_name: get("DBusName")?.to_owned(),
        interfaces: parse_list(get("Interfaces")?)?,
        use_in: match get("UseIn") {
            Some(value) => parse_list(value)?,
            None => Vec::new(),
        },
    })
}

/// Parse `portals.conf`. `None` when the frontend would skip the file and
/// move on to the next one: GLib rejects it, `[preferred]` has no keys, or a
/// value is not a valid list.
pub(crate) fn parse_conf(path: &Path, text: &str) -> Option<PortalConf> {
    let groups = parse_key_file(text)?;
    let group = key_file_group(&groups, "preferred");
    if group.is_empty() {
        return None;
    }
    let mut conf = PortalConf {
        path: path.to_owned(),
        interfaces: Vec::new(),
        default: None,
    };
    for (key, value) in group {
        let list = parse_list(value)?;
        if key == "default" {
            conf.default = Some(list);
        } else {
            conf.interfaces.push((key.clone(), list));
        }
    }
    Some(conf)
}

/// Sort the way the frontend does: a backend whose `UseIn` names an earlier
/// current desktop first, then by name.
pub(crate) fn sort_portals(portals: &mut [PortalFile], desktops: &[String]) {
    let rank = |p: &PortalFile| {
        desktops
            .iter()
            .position(|d| p.used_in(d))
            .unwrap_or(usize::MAX)
    };
    portals.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)));
}

/// The backend the frontend picks for `interface` (`find_portal_implementation`).
/// `portals` must be sorted with [`sort_portals`].
fn choose<'a>(
    conf: Option<&PortalConf>,
    portals: &'a [PortalFile],
    desktops: &[String],
    interface: &str,
) -> Choice<'a> {
    let any = || portals.iter().find(|p| p.declares(interface));
    let named = |name: &str| {
        portals
            .iter()
            .find(|p| p.name == name)
            .filter(|p| p.declares(interface))
    };
    if let Some(conf) = conf {
        let list = conf.interface(interface).or(conf.default.as_deref());
        if list.is_some_and(|l| l.iter().any(|e| e == "none")) {
            return Choice::Vetoed;
        }
        let mut picked = None;
        for entry in conf.interface(interface).unwrap_or_default() {
            if entry == "*" {
                picked = any();
                break;
            }
            if let Some(p) = named(entry) {
                picked = Some(p);
                break;
            }
        }
        if picked.is_none() {
            for entry in conf.default.as_deref().unwrap_or_default() {
                // The frontend resolves `default=*` against the literal
                // interface name "default", which no backend declares.
                if entry == "*" {
                    break;
                }
                if let Some(p) = named(entry) {
                    picked = Some(p);
                    break;
                }
            }
        }
        if let Some(p) = picked {
            return Choice::Backend(p);
        }
    }
    for desktop in desktops {
        if let Some(p) = portals
            .iter()
            .find(|p| p.declares(interface) && p.used_in(desktop))
        {
            return Choice::Backend(p);
        }
    }
    match portals
        .iter()
        .find(|p| p.dbus_name == GTK_BACKEND && p.declares(interface))
    {
        Some(p) => Choice::Backend(p),
        None => Choice::None,
    }
}

/// Resolve where RemoteDesktop goes. `portals` must be sorted with
/// [`sort_portals`]. `exports` asks a backend whether it exports the
/// interface; `None` means it could not be asked.
pub(crate) fn resolve(
    search: &ConfSearch,
    portals: &[PortalFile],
    desktops: &[String],
    pin_file: PathBuf,
    exports: impl FnOnce(&PortalFile) -> Option<bool>,
) -> Diagnosis {
    let conf = search.active.as_ref();
    let choice = choose(conf, portals, desktops, REMOTE_DESKTOP_IMPL);
    let chosen = match &choice {
        Choice::Backend(p) => Some(p.name.as_str()),
        _ => None,
    };
    let route = match choice {
        Choice::Vetoed => Route::Vetoed,
        Choice::None if portals.is_empty() => Route::Unknown,
        Choice::None => Route::NoBackend,
        Choice::Backend(p) => match exports(p) {
            Some(true) => Route::Served {
                backend: p.name.clone(),
            },
            Some(false) => Route::NotExported {
                backend: p.name.clone(),
            },
            None => Route::Unknown,
        },
    };
    Diagnosis {
        route,
        conf: conf.map(|c| c.path.clone()),
        unreadable: search.unreadable.clone(),
        suggest: portals
            .iter()
            .find(|p| p.declares(REMOTE_DESKTOP_IMPL) && Some(p.name.as_str()) != chosen)
            .map(|p| p.name.clone()),
        pin_file,
    }
}

/// Valid, lowercased `XDG_CURRENT_DESKTOP` entries, as the frontend uses them.
pub(crate) fn current_desktops() -> Vec<String> {
    parse_desktops(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

fn parse_desktops(value: &str) -> Vec<String> {
    value
        .split(':')
        .filter(|d| {
            !d.is_empty()
                && d.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .map(|d| d.to_ascii_lowercase())
        .collect()
}

/// The frontend's config search: in each directory, `<desktop>-portals.conf`
/// for each current desktop, then `portals.conf`.
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

/// The first candidate that can be read and has a `[preferred]` key. Files
/// that exist but cannot be read are skipped, as the frontend skips them.
pub(crate) fn read_conf(candidates: &[PathBuf]) -> ConfSearch {
    let mut search = ConfSearch::default();
    for path in candidates {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                if let Some(conf) = parse_conf(path, &text) {
                    search.active = Some(conf);
                    return search;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => search.unreadable.push((path.clone(), e.to_string())),
        }
    }
    search
}

/// Installed `.portal` files; the first directory wins for a name.
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

fn data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(dirs::data_dir());
    roots.extend(env_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    roots.push(PathBuf::from("/usr/share"));
    roots
}

/// Config directories in the frontend's order: XDG_CONFIG_HOME,
/// XDG_CONFIG_DIRS, /etc, XDG_DATA_HOME, XDG_DATA_DIRS, /usr/share.
pub(crate) fn conf_dirs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(dirs::config_dir());
    roots.extend(env_dirs("XDG_CONFIG_DIRS", "/etc/xdg"));
    roots.push(PathBuf::from("/etc"));
    roots.extend(data_roots());
    roots
        .into_iter()
        .map(|r| r.join("xdg-desktop-portal"))
        .collect()
}

pub(crate) fn portal_file_dirs() -> Vec<PathBuf> {
    data_roots()
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
pub(crate) fn probe() -> Diagnosis {
    let desktops = current_desktops();
    let search = read_conf(&conf_candidates(&conf_dirs(), &desktops));
    let mut portals = read_portal_files(&portal_file_dirs());
    sort_portals(&mut portals, &desktops);
    let pin_name = match desktops.first() {
        Some(desktop) => format!("{desktop}-portals.conf"),
        None => "portals.conf".to_owned(),
    };
    let pin_file = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("xdg-desktop-portal")
        .join(pin_name);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok();
    resolve(&search, &portals, &desktops, pin_file, |backend| {
        rt.as_ref()?.block_on(async {
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

type KeyFileGroups = Vec<(String, Vec<(String, String)>)>;

/// Groups of a key file, parsed as GLib's `GKeyFile` does: comments and
/// blank lines skipped, repeated groups merged, a repeated key's last value
/// kept, locale keys (`Key[de]`) left out, and the value's leading (not
/// trailing) whitespace dropped. `None` when GLib rejects the file.
fn parse_key_file(text: &str) -> Option<KeyFileGroups> {
    let mut groups: KeyFileGroups = Vec::new();
    let mut current = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.trim_end().strip_suffix(']')?;
            if name.is_empty() || name.contains(['[', ']']) {
                return None;
            }
            current = Some(match groups.iter().position(|(g, _)| g == name) {
                Some(at) => at,
                None => {
                    groups.push((name.to_owned(), Vec::new()));
                    groups.len() - 1
                }
            });
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.trim_end();
        if key.is_empty() {
            return None;
        }
        let (_, pairs) = &mut groups[current?];
        if key.contains('[') {
            continue;
        }
        let value = value.trim_start().to_owned();
        match pairs.iter_mut().find(|(k, _)| k == key) {
            Some(pair) => pair.1 = value,
            None => pairs.push((key.to_owned(), value)),
        }
    }
    Some(groups)
}

fn key_file_group<'a>(groups: &'a KeyFileGroups, name: &str) -> &'a [(String, String)] {
    groups
        .iter()
        .find(|(g, _)| g == name)
        .map(|(_, pairs)| pairs.as_slice())
        .unwrap_or_default()
}

/// A `;`-separated list as `g_key_file_get_string_list` reads it: escapes
/// decoded, items not trimmed, one trailing empty item dropped. `None` for
/// an invalid escape.
fn parse_list(value: &str) -> Option<Vec<String>> {
    let mut items = vec![String::new()];
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        let item = items.last_mut()?;
        match c {
            ';' => items.push(String::new()),
            '\\' => item.push(match chars.next()? {
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                ';' => ';',
                _ => return None,
            }),
            c => item.push(c),
        }
    }
    if items.last().is_some_and(String::is_empty) {
        items.pop();
    }
    Some(items)
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
    const CONF_PATH: &str = "/home/u/.config/xdg-desktop-portal/portals.conf";
    const PIN_PATH: &str = "/home/u/.config/xdg-desktop-portal/gnome-portals.conf";

    fn gnome() -> Vec<String> {
        vec!["gnome".into()]
    }

    fn portals(desktops: &[String]) -> Vec<PortalFile> {
        let mut portals = vec![
            parse_portal_file("kde", KDE_PORTAL).unwrap(),
            parse_portal_file("gtk", GTK_PORTAL).unwrap(),
            parse_portal_file("gnome", GNOME_PORTAL).unwrap(),
        ];
        sort_portals(&mut portals, desktops);
        portals
    }

    fn conf(text: &str) -> ConfSearch {
        ConfSearch {
            active: parse_conf(Path::new(CONF_PATH), text),
            unreadable: Vec::new(),
        }
    }

    /// Only the gnome backend exports RemoteDesktop under Mutter.
    fn mutter(backend: &PortalFile) -> Option<bool> {
        Some(backend.name == "gnome")
    }

    fn diagnose(search: &ConfSearch) -> Diagnosis {
        resolve(
            search,
            &portals(&gnome()),
            &gnome(),
            PathBuf::from(PIN_PATH),
            mutter,
        )
    }

    #[test]
    fn a_kde_pin_in_a_gnome_session_names_the_backend_that_lacks_remote_desktop() {
        let diagnosis = diagnose(&conf("[preferred]\ndefault=kde\n"));
        assert_eq!(
            diagnosis.route,
            Route::NotExported {
                backend: "kde".into()
            }
        );
        let (message, hint) = diagnosis.problem().unwrap();
        assert_eq!(
            message,
            format!(
                "xdg-desktop-portal routes RemoteDesktop to the kde backend (per {CONF_PATH}), \
                 but xdg-desktop-portal-kde does not export {REMOTE_DESKTOP_IMPL} in this \
                 session, so every click and key press fails with \"No such interface\"."
            )
        );
        assert_eq!(
            hint,
            format!(
                "Under `[preferred]` in {PIN_PATH}, set `{REMOTE_DESKTOP_IMPL}=gnome` (that \
                 file replaces {CONF_PATH}, so copy its other settings). Keep the file \
                 readable (mode 644), then restart xdg-desktop-portal or log out and back in."
            )
        );
    }

    #[test]
    fn an_unconfigured_gnome_session_takes_the_backend_used_in_gnome() {
        let diagnosis = diagnose(&ConfSearch::default());
        assert_eq!(
            diagnosis.route,
            Route::Served {
                backend: "gnome".into()
            }
        );
        assert!(diagnosis.problem().is_none());
    }

    #[test]
    fn use_in_follows_the_order_of_the_current_desktops() {
        let desktops = vec!["kde".to_owned(), "gnome".to_owned()];
        let diagnosis = resolve(
            &ConfSearch::default(),
            &portals(&desktops),
            &desktops,
            PathBuf::from(PIN_PATH),
            mutter,
        );
        assert_eq!(
            diagnosis.route,
            Route::NotExported {
                backend: "kde".into()
            }
        );
    }

    #[test]
    fn a_gnome_pin_routes_to_gnome() {
        let text = "[preferred]\ndefault=gnome\norg.freedesktop.impl.portal.Secret=gnome-keyring\n";
        assert_eq!(
            diagnose(&conf(text)).route,
            Route::Served {
                backend: "gnome".into()
            }
        );
    }

    #[test]
    fn an_interface_key_naming_no_usable_backend_falls_back_to_default() {
        let text = "[preferred]\ndefault=gnome\norg.freedesktop.impl.portal.RemoteDesktop=gtk\n";
        assert_eq!(
            diagnose(&conf(text)).route,
            Route::Served {
                backend: "gnome".into()
            }
        );
    }

    #[test]
    fn an_interface_key_overrides_default() {
        let text =
            "[preferred]\ndefault=gnome\norg.freedesktop.impl.portal.RemoteDesktop=gtk;kde\n";
        assert_eq!(
            diagnose(&conf(text)).route,
            Route::NotExported {
                backend: "kde".into()
            }
        );
    }

    #[test]
    fn a_star_takes_the_first_backend_in_desktop_order() {
        let text = "[preferred]\norg.freedesktop.impl.portal.RemoteDesktop=*\n";
        assert_eq!(
            diagnose(&conf(text)).route,
            Route::Served {
                backend: "gnome".into()
            }
        );
    }

    #[test]
    fn a_default_star_falls_through_to_use_in() {
        let desktops = vec!["kde".to_owned()];
        let diagnosis = resolve(
            &conf("[preferred]\ndefault=*\n"),
            &portals(&desktops),
            &desktops,
            PathBuf::from(PIN_PATH),
            mutter,
        );
        assert_eq!(
            diagnosis.route,
            Route::NotExported {
                backend: "kde".into()
            }
        );
    }

    #[test]
    fn without_a_matching_desktop_only_the_gtk_backend_is_a_fallback() {
        let desktops = vec!["sway".to_owned()];
        let diagnosis = resolve(
            &ConfSearch::default(),
            &portals(&desktops),
            &desktops,
            PathBuf::from(PIN_PATH),
            mutter,
        );
        assert_eq!(diagnosis.route, Route::NoBackend);
        assert!(diagnosis.problem().is_some());
    }

    #[test]
    fn none_on_the_interface_or_default_vetoes_remote_desktop() {
        for text in [
            "[preferred]\ndefault=gnome\norg.freedesktop.impl.portal.RemoteDesktop=none\n",
            "[preferred]\ndefault=none\n",
        ] {
            let diagnosis = diagnose(&conf(text));
            assert_eq!(diagnosis.route, Route::Vetoed, "{text}");
            let (message, _) = diagnosis.problem().unwrap();
            assert!(message.contains("set to `none`"), "{message}");
        }
        let text = "[preferred]\ndefault=none\norg.freedesktop.impl.portal.RemoteDesktop=gnome\n";
        assert_eq!(
            diagnose(&conf(text)).route,
            Route::Served {
                backend: "gnome".into()
            }
        );
    }

    #[test]
    fn a_skipped_unreadable_conf_is_named_only_when_input_is_broken() {
        let unreadable = (
            PathBuf::from(CONF_PATH),
            "Permission denied (os error 13)".to_owned(),
        );
        let working = ConfSearch {
            active: None,
            unreadable: vec![unreadable.clone()],
        };
        assert!(diagnose(&working).problem().is_none());

        let broken = ConfSearch {
            active: parse_conf(
                Path::new("/etc/xdg/xdg-desktop-portal/portals.conf"),
                "[preferred]\ndefault=kde\n",
            ),
            unreadable: vec![unreadable],
        };
        let (message, _) = diagnose(&broken).problem().unwrap();
        assert!(
            message.ends_with(&format!(
                " xdg-desktop-portal skips {CONF_PATH} because it cannot be read \
                 (Permission denied (os error 13))."
            )),
            "{message}"
        );
    }

    #[test]
    fn a_backend_that_cannot_be_asked_keeps_the_earlier_verdict() {
        let diagnosis = resolve(
            &ConfSearch::default(),
            &portals(&gnome()),
            &gnome(),
            PathBuf::from(PIN_PATH),
            |_| None,
        );
        assert_eq!(diagnosis.route, Route::Unknown);
        assert!(diagnosis.problem().is_none());
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
    fn read_conf_skips_missing_files_and_files_without_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gnome-portals.conf");
        let empty = dir.path().join("portals.conf");
        let used = dir.path().join("other.conf");
        std::fs::write(&empty, "# nothing\n").unwrap();
        std::fs::write(&used, "[preferred]\ndefault=kde\n").unwrap();
        let search = read_conf(&[missing, empty, used.clone()]);
        assert!(search.unreadable.is_empty());
        assert_eq!(
            search.active,
            Some(PortalConf {
                path: used,
                interfaces: Vec::new(),
                default: Some(vec!["kde".into()]),
            })
        );
    }

    #[test]
    fn read_conf_records_an_unreadable_file_and_keeps_searching() {
        let dir = tempfile::tempdir().unwrap();
        // A directory in the file's place fails to read with an error other
        // than NotFound, without depending on the user's permissions.
        let unreadable = dir.path().join("gnome-portals.conf");
        std::fs::create_dir(&unreadable).unwrap();
        let used = dir.path().join("portals.conf");
        std::fs::write(&used, "[preferred]\ndefault=gnome\n").unwrap();
        let search = read_conf(&[unreadable.clone(), used.clone()]);
        assert_eq!(search.unreadable.len(), 1);
        assert_eq!(search.unreadable[0].0, unreadable);
        assert_eq!(search.active.map(|c| c.path), Some(used));
    }

    #[test]
    fn conf_values_are_read_as_glib_reads_them() {
        let path = Path::new(CONF_PATH);
        let defaults = |text: &str| parse_conf(path, text).and_then(|c| c.default);
        // The last of a repeated key wins.
        assert_eq!(
            defaults("[preferred]\ndefault=none\ndefault=gnome\n"),
            Some(vec!["gnome".to_owned()])
        );
        // Items keep their whitespace, so ` none ` is not `none`.
        assert_eq!(
            defaults("[preferred]\ndefault = gnome; none \n"),
            Some(vec!["gnome".to_owned(), " none ".to_owned()])
        );
        assert_eq!(
            defaults("  [preferred]  \r\n  default=gnome;;kde;\r\n"),
            Some(vec!["gnome".to_owned(), String::new(), "kde".to_owned()])
        );
        assert_eq!(
            defaults("[preferred]\ndefault=a\\;b;c\\s\n"),
            Some(vec!["a;b".to_owned(), "c ".to_owned()])
        );
        assert_eq!(
            defaults("[preferred]\ndefault=x\n[other]\na=b\n[preferred]\ndefault=kde\n"),
            Some(vec!["kde".to_owned()])
        );
        let locale = parse_conf(path, "[preferred]\ndefault=gnome\nkey[de]=x\n").unwrap();
        assert!(locale.interfaces.is_empty(), "{locale:?}");
    }

    #[test]
    fn a_conf_glib_rejects_is_skipped_for_the_next_one() {
        for text in [
            "[preferred]\ndefault=none\nstray\n",
            "default=none\n[preferred]\ndefault=none\n",
            "[preferred]\n=none\n",
            "[preferred\ndefault=none\n",
            "[preferred]x\ndefault=none\n",
            "[preferred]\ndefault=none;\\\n",
        ] {
            assert_eq!(parse_conf(Path::new(CONF_PATH), text), None, "{text:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        let rejected = dir.path().join("gnome-portals.conf");
        let used = dir.path().join("portals.conf");
        std::fs::write(&rejected, "[preferred]\ndefault=none\nstray\n").unwrap();
        std::fs::write(&used, "[preferred]\ndefault=gnome\n").unwrap();
        let search = read_conf(&[rejected, used.clone()]);
        assert_eq!(search.active.map(|c| c.path), Some(used));
    }

    #[test]
    fn a_none_veto_is_reported_without_installed_portal_files() {
        let diagnosis = resolve(
            &conf("[preferred]\ndefault=none\n"),
            &[],
            &gnome(),
            PathBuf::from(PIN_PATH),
            mutter,
        );
        assert_eq!(diagnosis.route, Route::Vetoed);
        let empty = resolve(
            &ConfSearch::default(),
            &[],
            &gnome(),
            PathBuf::from(PIN_PATH),
            mutter,
        );
        assert_eq!(empty.route, Route::Unknown);
    }

    #[test]
    fn desktops_drop_invalid_entries_and_are_lowercased() {
        assert_eq!(parse_desktops("ubuntu:GNOME"), ["ubuntu", "gnome"]);
        assert_eq!(parse_desktops("KDE::bad/name"), ["kde"]);
    }
}
