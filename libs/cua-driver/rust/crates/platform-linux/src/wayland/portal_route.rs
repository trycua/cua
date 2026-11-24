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
//!
//! The frontend reads its config only when it starts, so the files on disk
//! are a prediction. Doctor trusts it only when it agrees with what the
//! running frontend exports and no config file changed after the frontend
//! started; otherwise it keeps its earlier verdict and says to restart.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
    /// The backend could not be asked (not running and not activatable,
    /// timeout), the prediction disagrees with what the running frontend
    /// exports, or a config changed after the frontend started. Doctor
    /// keeps its earlier verdict.
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
    /// The prediction disagreed with the running frontend.
    pub mismatch: bool,
    /// Config files changed after the running frontend started.
    pub stale: Vec<PathBuf>,
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

    /// Record config files changed after the running frontend started. It
    /// may still use the old config, so the prediction is not trusted.
    pub(crate) fn mark_stale(&mut self, stale: Vec<PathBuf>) {
        if !stale.is_empty() {
            self.route = Route::Unknown;
        }
        self.stale = stale;
    }

    /// Why the files on disk may not describe the running frontend.
    pub(crate) fn note(&self) -> Option<String> {
        let restart = "restart xdg-desktop-portal (`systemctl --user restart \
                       xdg-desktop-portal`) or log out and back in";
        if let Some(path) = self.stale.first() {
            return Some(format!(
                "xdg-desktop-portal reads its config only when it starts, and {} changed \
                 after it started, so the running portal may not use what is on disk; \
                 {restart}.",
                path.display()
            ));
        }
        self.mismatch.then(|| {
            format!("The running xdg-desktop-portal does not match its config on disk; {restart}.")
        })
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
pub(crate) fn parse_portal_file(name: &str, text: impl AsRef<[u8]>) -> Option<PortalFile> {
    let groups = parse_key_file(text.as_ref())?;
    let group = key_file_group(&groups, "portal").unwrap_or_default();
    let get = |key: &str| {
        group
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_deref())
    };
    let dbus_name = get("DBusName")?;
    let interfaces = parse_list(get("Interfaces")?)?;
    if !is_bus_name(dbus_name)
        || !interfaces
            .iter()
            .all(|i| is_interface_name(i) && i.starts_with("org.freedesktop.impl.portal."))
    {
        return None;
    }
    Some(PortalFile {
        name: name.to_owned(),
        dbus_name: dbus_name.to_owned(),
        interfaces,
        // The frontend registers a backend whose UseIn it cannot read.
        use_in: get("UseIn").and_then(parse_list).unwrap_or_default(),
    })
}

/// Parse `portals.conf`. `None` when the frontend would skip the file and
/// move on to the next one: GLib rejects it, it has no `[preferred]` group,
/// or a value is not a valid list. An empty `[preferred]` group still ends
/// the search.
pub(crate) fn parse_conf(path: &Path, text: impl AsRef<[u8]>) -> Option<PortalConf> {
    let groups = parse_key_file(text.as_ref())?;
    let group = key_file_group(&groups, "preferred")?;
    let mut conf = PortalConf {
        path: path.to_owned(),
        interfaces: Vec::new(),
        default: None,
    };
    // The frontend reads every key as a list, and drops the file when one
    // cannot be read.
    for (key, value) in group {
        let list = parse_list(value.as_deref()?)?;
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
/// [`sort_portals`]. `frontend_exports` is whether the running frontend
/// exports RemoteDesktop, when known: it does for a chosen backend and not
/// for `none` or no backend, so a prediction that disagrees is not trusted.
/// `exports` asks a backend whether it exports the interface; `None` means
/// it could not be asked.
pub(crate) fn resolve(
    search: &ConfSearch,
    portals: &[PortalFile],
    desktops: &[String],
    pin_file: PathBuf,
    frontend_exports: Option<bool>,
    exports: impl FnOnce(&PortalFile) -> Option<bool>,
) -> Diagnosis {
    let conf = search.active.as_ref();
    let choice = choose(conf, portals, desktops, REMOTE_DESKTOP_IMPL);
    let chosen = match &choice {
        Choice::Backend(p) => Some(p.name.as_str()),
        _ => None,
    };
    let predicted = matches!(choice, Choice::Backend(_));
    let mismatch = frontend_exports.is_some_and(|live| live != predicted);
    let route = match choice {
        _ if mismatch => Route::Unknown,
        // An empty scan may be directories doctor cannot see, unless the
        // frontend confirms it has no backend.
        Choice::None if portals.is_empty() && frontend_exports != Some(false) => Route::Unknown,
        Choice::Vetoed => Route::Vetoed,
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
        mismatch,
        stale: Vec::new(),
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
        match std::fs::read(path) {
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
            if let Some(file) = std::fs::read(&path)
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

/// `XDG_DESKTOP_PORTAL_DIR` replaces every config and `.portal` directory.
fn portal_dir_override() -> Option<PathBuf> {
    std::env::var_os("XDG_DESKTOP_PORTAL_DIR").map(PathBuf::from)
}

/// Config directories in the frontend's order: XDG_CONFIG_HOME,
/// XDG_CONFIG_DIRS, /etc, XDG_DATA_HOME, XDG_DATA_DIRS, /usr/share.
pub(crate) fn conf_dirs() -> Vec<PathBuf> {
    if let Some(dir) = portal_dir_override() {
        return vec![dir];
    }
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
    if let Some(dir) = portal_dir_override() {
        return vec![dir];
    }
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
    Some(xml_declares_interface(&xml, interface))
}

/// Whether introspection XML has `<interface name="…">` for `interface`,
/// with either quote style, outside comments.
fn xml_declares_interface(xml: &str, interface: &str) -> bool {
    let mut uncommented = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        uncommented.push_str(&rest[..start]);
        rest = rest[start..]
            .find("-->")
            .map_or("", |end| &rest[start + end + 3..]);
    }
    uncommented.push_str(rest);
    uncommented
        .split("<interface")
        .skip(1)
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_whitespace()))
        .filter_map(|rest| rest.split('>').next())
        .any(|tag| {
            let mut rest = tag;
            while let Some(at) = rest.find("name") {
                let before = rest[..at].chars().last();
                let after = rest[at + 4..].trim_start();
                rest = &rest[at + 4..];
                if !before.is_some_and(|c| c.is_ascii_whitespace()) {
                    continue;
                }
                let Some(value) = after.strip_prefix('=').map(str::trim_start) else {
                    continue;
                };
                let Some(quote) = value.chars().next().filter(|q| matches!(q, '"' | '\'')) else {
                    continue;
                };
                return value[1..].split(quote).next() == Some(interface);
            }
            false
        })
}

/// When the process that owns `org.freedesktop.portal.Desktop` started.
async fn frontend_started(connection: &zbus::Connection) -> Option<SystemTime> {
    let proxy = zbus::fdo::DBusProxy::new(connection).await.ok()?;
    let name = zbus::names::BusName::try_from("org.freedesktop.portal.Desktop").ok()?;
    let pid = proxy.get_connection_unix_process_id(name).await.ok()?;
    let ticks = crate::proc_fs::process_instance_id(pid)?;
    // SAFETY: sysconf has no preconditions.
    let per_second = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok()?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let since_boot = Duration::from_secs_f64(ticks as f64 / per_second.max(1) as f64);
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(boot) + since_boot)
}

/// Existing candidates modified after `started`.
fn changed_since(candidates: &[PathBuf], started: SystemTime) -> Vec<PathBuf> {
    candidates
        .iter()
        .filter(|path| {
            std::fs::metadata(path)
                .and_then(|m| m.modified())
                .is_ok_and(|modified| modified > started)
        })
        .cloned()
        .collect()
}

/// Read the config and `.portal` files, then introspect the chosen backend.
/// `frontend_exports` is whether the running frontend exports RemoteDesktop,
/// when known.
pub(crate) fn probe(frontend_exports: Option<bool>) -> Diagnosis {
    let desktops = current_desktops();
    let candidates = conf_candidates(&conf_dirs(), &desktops);
    let search = read_conf(&candidates);
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
    let mut diagnosis = resolve(
        &search,
        &portals,
        &desktops,
        pin_file,
        frontend_exports,
        |backend| {
            rt.as_ref()?.block_on(async {
                tokio::time::timeout(super::portal::PROBE_TIMEOUT, async {
                    let connection = super::portal::fresh_session_connection().await.ok()?;
                    backend_exports(&connection, &backend.dbus_name, REMOTE_DESKTOP_IMPL).await
                })
                .await
                .ok()
                .flatten()
            })
        },
    );
    let started = rt.as_ref().and_then(|rt| {
        rt.block_on(async {
            tokio::time::timeout(super::portal::PROBE_TIMEOUT, async {
                let connection = super::portal::fresh_session_connection().await.ok()?;
                frontend_started(&connection).await
            })
            .await
            .ok()
            .flatten()
        })
    });
    if let Some(started) = started {
        diagnosis.mark_stale(changed_since(&candidates, started));
    }
    diagnosis
}

/// Each group's keys, with a value that is `None` when it is not UTF-8.
type KeyFileGroups = Vec<(String, Vec<(String, Option<String>)>)>;

/// Groups of a key file, parsed as GLib's `GKeyFile` does: each line ends
/// at its first NUL, comments and blank lines are skipped, repeated groups
/// merged, a repeated key's last value kept, locale keys (`Key[de]`) left
/// out, and the value's leading (not trailing) whitespace dropped. `None`
/// when GLib rejects the file.
fn parse_key_file(text: &[u8]) -> Option<KeyFileGroups> {
    let mut groups: KeyFileGroups = Vec::new();
    let mut current = None;
    let lines: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
    for (at, line) in lines.iter().enumerate() {
        // One CR before an LF ends the line; a CR anywhere else is content.
        let line = match line.strip_suffix(b"\r") {
            Some(stripped) if at + 1 < lines.len() => stripped,
            _ => line,
        };
        let line = line.split(|&b| b == 0).next().unwrap_or_default();
        let start = line
            .iter()
            .position(|&b| !glib_space(b))
            .unwrap_or(line.len());
        let line = &line[start..];
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if let Some(rest) = line.strip_prefix(b"[") {
            let end = rest
                .iter()
                .rposition(|&b| b != b' ' && b != b'\t')
                .map_or(0, |i| i + 1);
            let name = rest[..end].strip_suffix(b"]")?;
            if name.is_empty()
                || name
                    .iter()
                    .any(|&b| b == b'[' || b == b']' || b.is_ascii_control())
            {
                return None;
            }
            let name = String::from_utf8_lossy(name).into_owned();
            current = Some(match groups.iter().position(|(g, _)| *g == name) {
                Some(at) => at,
                None => {
                    groups.push((name, Vec::new()));
                    groups.len() - 1
                }
            });
            continue;
        }
        let eq = line.iter().position(|&b| b == b'=')?;
        let key = &line[..eq];
        let key_end = key
            .iter()
            .rposition(|&b| !glib_space(b))
            .map_or(0, |i| i + 1);
        let key = String::from_utf8_lossy(&key[..key_end]).into_owned();
        let value = &line[eq + 1..];
        let value_start = value
            .iter()
            .position(|&b| !glib_space(b))
            .unwrap_or(value.len());
        let value = std::str::from_utf8(&value[value_start..])
            .ok()
            .map(str::to_owned);
        let (_, pairs) = &mut groups[current?];
        match key_kind(&key)? {
            KeyKind::Plain => {}
            KeyKind::Localized => continue,
        }
        if current == Some(0)
            && key == "Encoding"
            && !value
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("UTF-8"))
        {
            return None;
        }
        match pairs.iter_mut().find(|(k, _)| *k == key) {
            Some(pair) => pair.1 = value,
            None => pairs.push((key, value)),
        }
    }
    Some(groups)
}

enum KeyKind {
    Plain,
    /// `Key[locale]`, which `g_key_file_get_keys` leaves out.
    Localized,
}

/// `g_key_file_is_key_name`: `None` for a key GLib rejects the file over.
fn key_kind(key: &str) -> Option<KeyKind> {
    let (name, locale) = match key.split_once('[') {
        Some((name, rest)) => (name, Some(rest.strip_suffix(']')?)),
        None => (key, None),
    };
    if name.is_empty() || name.contains(']') || name.starts_with(' ') || name.ends_with(' ') {
        return None;
    }
    match locale {
        None => Some(KeyKind::Plain),
        Some(locale)
            if !locale
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@')) =>
        {
            None
        }
        // GLib lists `Key[]` as a key of its own.
        Some("") => Some(KeyKind::Plain),
        Some(_) => Some(KeyKind::Localized),
    }
}

/// `g_ascii_isspace`: ASCII only, and no vertical tab.
fn glib_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// `g_dbus_is_interface_name`.
fn is_interface_name(name: &str) -> bool {
    name.len() <= 255
        && name.split('.').count() >= 2
        && name.split('.').all(|element| {
            let mut chars = element.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// `g_dbus_is_name`: a unique (`:1.42`) or well-known bus name.
fn is_bus_name(name: &str) -> bool {
    let (unique, body) = match name.strip_prefix(':') {
        Some(body) => (true, body),
        None => (false, name),
    };
    name.len() <= 255
        && body.split('.').count() >= 2
        && body.split('.').all(|element| {
            let mut chars = element.chars();
            chars.next().is_some_and(|c| {
                c.is_ascii_alphabetic() || c == '_' || c == '-' || (unique && c.is_ascii_digit())
            }) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

fn key_file_group<'a>(
    groups: &'a KeyFileGroups,
    name: &str,
) -> Option<&'a [(String, Option<String>)]> {
    groups
        .iter()
        .find(|(g, _)| g == name)
        .map(|(_, pairs)| pairs.as_slice())
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
    fn read_conf_skips_missing_files_and_files_without_a_preferred_group() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gnome-portals.conf");
        let no_group = dir.path().join("portals.conf");
        let empty_group = dir.path().join("empty.conf");
        let lower = dir.path().join("lower.conf");
        std::fs::write(&no_group, "# nothing\n[other]\na=b\n").unwrap();
        std::fs::write(&empty_group, "[preferred]\n").unwrap();
        std::fs::write(&lower, "[preferred]\ndefault=none\n").unwrap();
        let search = read_conf(&[missing, no_group, empty_group.clone(), lower]);
        assert!(search.unreadable.is_empty());
        // An empty `[preferred]` group is a config: the frontend stops there.
        assert_eq!(
            search.active,
            Some(PortalConf {
                path: empty_group,
                interfaces: Vec::new(),
                default: None,
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
        let locale = parse_conf(
            path,
            "[preferred]\ndefault=gnome\nkey[de_DE.UTF-8@euro]=x\ndefault[en]=none\n",
        )
        .unwrap();
        assert!(locale.interfaces.is_empty(), "{locale:?}");
        assert_eq!(locale.default, Some(vec!["gnome".to_owned()]));
        // Only a CR before an LF ends a line.
        assert_eq!(
            defaults("[preferred]\r\ndefault=none\r\r\n"),
            Some(vec!["none\r".to_owned()])
        );
        assert_eq!(
            defaults("[preferred]\ndefault=none\r"),
            Some(vec!["none\r".to_owned()])
        );
        // Encoding is only checked in the first group; non-ASCII control
        // characters are allowed in group names.
        assert_eq!(
            defaults("[a\u{85}b]\nEncoding=utf-8\n[preferred]\nEncoding=ISO\ndefault=kde\n"),
            Some(vec!["kde".to_owned()])
        );
        let empty_locale = parse_conf(path, "[preferred]\nk[]=x\n").unwrap();
        assert_eq!(
            empty_locale.interfaces,
            [("k[]".to_owned(), vec!["x".to_owned()])]
        );
        // Only ASCII whitespace is trimmed, so this is not `none`.
        assert_eq!(
            defaults("[preferred]\ndefault=\u{a0}none\n"),
            Some(vec!["\u{a0}none".to_owned()])
        );
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
            "[preferred]\ndefault=none;\\q\n",
            "[preferred]\ndefault=none\nbad[key=gnome\n",
            "[preferred]\ndefault=none\nbad]=gnome\n",
            "[preferred]\ndefault=none\n[bad\tgroup]\nfoo=x",
            "[preferred]\x0c\ndefault=none\n",
            "[preferred]\ndefault=none\nk [de]=x\n",
            "[preferred]\ndefault=none\nk[d e]=x\n",
            "[preferred]\nEncoding=ISO-8859-1\ndefault=none\n",
            "[pre\x7fferred]\ndefault=none\n",
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
            None,
            mutter,
        );
        assert_eq!(diagnosis.route, Route::Vetoed);
        let empty = resolve(
            &ConfSearch::default(),
            &[],
            &gnome(),
            PathBuf::from(PIN_PATH),
            None,
            mutter,
        );
        assert_eq!(empty.route, Route::Unknown);
    }

    #[test]
    fn portal_files_are_registered_as_the_frontend_registers_them() {
        let bad_use_in = parse_portal_file(
            "kde",
            "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.kde\n\
             Interfaces=org.freedesktop.impl.portal.RemoteDesktop;\nUseIn=KDE;\\q\n",
        )
        .unwrap();
        assert!(bad_use_in.use_in.is_empty(), "{bad_use_in:?}");
        for text in [
            "[portal]\nInterfaces=org.freedesktop.impl.portal.RemoteDesktop;\n",
            "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.x\n",
            "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.x\n\
             Interfaces=org.freedesktop.portal.RemoteDesktop;\n",
        ] {
            assert_eq!(parse_portal_file("x", text), None, "{text:?}");
        }
    }

    #[test]
    fn a_prediction_the_running_frontend_contradicts_is_not_trusted() {
        let resolve_live = |search: &ConfSearch, portals: &[PortalFile], live| {
            resolve(
                search,
                portals,
                &gnome(),
                PathBuf::from(PIN_PATH),
                Some(live),
                mutter,
            )
        };
        let all = portals(&gnome());
        // Edited to `none` after the frontend started: it still exports
        // RemoteDesktop, so the veto on disk is not the running state.
        let vetoed = resolve_live(&conf("[preferred]\ndefault=none\n"), &all, true);
        assert_eq!(vetoed.route, Route::Unknown);
        assert!(vetoed.mismatch && vetoed.problem().is_none());
        assert!(
            vetoed
                .note()
                .unwrap()
                .contains("does not match its config on disk"),
            "{vetoed:?}"
        );
        // A kde route the frontend does not export is not reported either.
        let kde = resolve_live(&conf("[preferred]\ndefault=kde\n"), &all, false);
        assert_eq!(kde.route, Route::Unknown);
        // Agreement keeps the verdict.
        let kde = resolve_live(&conf("[preferred]\ndefault=kde\n"), &all, true);
        assert_eq!(
            kde.route,
            Route::NotExported {
                backend: "kde".into()
            }
        );
        assert!(!kde.mismatch);
        // No installed backend: the frontend agrees when it hides RemoteDesktop.
        assert_eq!(
            resolve_live(&ConfSearch::default(), &[], false).route,
            Route::NoBackend
        );
        assert_eq!(
            resolve_live(&ConfSearch::default(), &[], true).route,
            Route::Unknown
        );
    }

    #[test]
    fn a_config_changed_after_the_frontend_started_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let conf_path = dir.path().join("portals.conf");
        std::fs::write(&conf_path, "[preferred]\ndefault=gnome\n").unwrap();
        let missing = dir.path().join("gnome-portals.conf");
        let candidates = [missing, conf_path.clone()];
        let before = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(changed_since(&candidates, before), vec![conf_path.clone()]);
        let after = SystemTime::now() + Duration::from_secs(3600);
        assert!(changed_since(&candidates, after).is_empty());

        // A kde pin written after the frontend started: the running portal
        // may still route to gnome, so doctor does not fail it.
        let mut diagnosis = diagnose(&conf("[preferred]\ndefault=kde\n"));
        assert!(diagnosis.problem().is_some());
        diagnosis.mark_stale(vec![conf_path.clone()]);
        assert_eq!(diagnosis.route, Route::Unknown);
        assert!(diagnosis.problem().is_none());
        assert_eq!(
            diagnosis.note().unwrap(),
            format!(
                "xdg-desktop-portal reads its config only when it starts, and {} changed \
                 after it started, so the running portal may not use what is on disk; \
                 restart xdg-desktop-portal (`systemctl --user restart xdg-desktop-portal`) \
                 or log out and back in.",
                conf_path.display()
            )
        );
        let mut fresh = diagnose(&conf("[preferred]\ndefault=kde\n"));
        fresh.mark_stale(Vec::new());
        assert!(fresh.problem().is_some());
    }

    #[test]
    fn introspection_matches_interface_elements_only() {
        let rd = REMOTE_DESKTOP_IMPL;
        assert!(xml_declares_interface(
            &format!("<node><interface name=\"{rd}\"><method name=\"Start\"/></interface></node>"),
            rd
        ));
        assert!(xml_declares_interface(
            &format!("<node>\n <interface\n   name = '{rd}' >\n</interface></node>"),
            rd
        ));
        assert!(!xml_declares_interface(
            &format!(
                "<node><interface name=\"org.x.Y\"><annotation name=\"a\" value=\"{rd}\"/>\
                 </interface></node>"
            ),
            rd
        ));
        assert!(!xml_declares_interface(
            &format!("<node><interfaces name=\"{rd}\"/><interface xname=\"{rd}\"/></node>"),
            rd
        ));
        assert!(!xml_declares_interface(
            &format!("<node><!-- <interface name=\"{rd}\"> --></node>"),
            rd
        ));
    }

    #[test]
    fn portal_files_need_valid_bus_and_interface_names() {
        let file = |dbus_name: &str, interfaces: &str| {
            parse_portal_file(
                "x",
                format!("[portal]\nDBusName={dbus_name}\nInterfaces={interfaces}\n"),
            )
        };
        let rd = REMOTE_DESKTOP_IMPL;
        assert!(file("org.freedesktop.impl.portal.desktop.x", rd).is_some());
        assert!(file(":1.42", rd).is_some());
        for (dbus_name, interfaces) in [
            ("org.freedesktop.impl.portal.desktop.x ", rd.to_owned()),
            ("single", rd.to_owned()),
            ("org.1x", rd.to_owned()),
            ("org..x", rd.to_owned()),
            (
                "org.freedesktop.impl.portal.desktop.x",
                format!("{rd};org.freedesktop.impl.portal.FileChooser "),
            ),
            (
                "org.freedesktop.impl.portal.desktop.x",
                "org.freedesktop.impl.portal.1x".to_owned(),
            ),
        ] {
            assert_eq!(
                file(dbus_name, &interfaces),
                None,
                "{dbus_name:?} {interfaces:?}"
            );
        }
    }

    #[test]
    fn conf_bytes_are_read_as_glib_reads_them() {
        let path = Path::new(CONF_PATH);
        let defaults = |text: &[u8]| parse_conf(path, text).and_then(|c| c.default);
        // Invalid UTF-8 in a comment or in another group is fine.
        assert_eq!(
            defaults(b"# \xff\n[preferred]\ndefault=gnome\n[o]\nk=\xff\n"),
            Some(vec!["gnome".to_owned()])
        );
        // In a [preferred] value the frontend cannot read it and skips the file.
        assert_eq!(parse_conf(path, b"[preferred]\ndefault=\xff\n"), None);
        // A line ends at its first NUL.
        assert_eq!(
            defaults(b"[preferred]\ndefault=none\x00x\n\x00[x]\n"),
            Some(vec!["none".to_owned()])
        );
        assert_eq!(parse_conf(path, b"[preferred]\nde\x00fault=none\n"), None);
    }

    #[test]
    fn desktops_drop_invalid_entries_and_are_lowercased() {
        assert_eq!(parse_desktops("ubuntu:GNOME"), ["ubuntu", "gnome"]);
        assert_eq!(parse_desktops("KDE::bad/name"), ["kde"]);
    }
}
