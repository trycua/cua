// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Installed host apps: `.app` bundles on macOS, `.desktop` entries on
//! Linux, Start Menu shortcuts on Windows.
//!
//! Enumeration only reads app metadata (an `Info.plist`, a `.desktop` file,
//! a shortcut's name), never an app's data. It always takes explicit roots
//! ([`enumerate`]); [`default_roots`] is the real machine's list and refuses
//! under `cfg(test)` or `CUA_ENV_TEST_SANDBOX=1`, so tests enumerate fixture
//! directories only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Platform;

/// Bound on directory entries visited per root (a pathological tree never
/// stalls a picker).
pub const MAX_ENTRIES_PER_ROOT: usize = 20_000;
/// How deep enumeration descends below a root (`/Applications/Utilities/X.app`
/// is depth 2).
pub const MAX_DEPTH: usize = 3;

/// One installed app on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostApp {
    /// Display name ("Visual Studio Code").
    pub name: String,
    /// Platform app id: a macOS bundle id or a Linux desktop id (`code` for
    /// `code.desktop`). `None` for Windows shortcuts.
    pub app_id: Option<String>,
    /// The bundle, `.desktop` file, or shortcut.
    pub path: PathBuf,
    /// Version, when the metadata says.
    pub version: Option<String>,
    /// Icon: a macOS `.icns` path inside the bundle, or a Linux icon name or
    /// path. `None` when the metadata names none.
    pub icon: Option<String>,
    /// Which platform's layout this came from.
    pub platform: Platform,
}

/// Overrides [`default_roots`] with a `:`-separated list (`;` on Windows),
/// for demos and screenshots over fixture apps.
pub const ROOTS_ENV: &str = "CUA_TELEPORT_APP_ROOTS";

/// Where apps are installed on the real machine, for `platform`
/// ([`ROOTS_ENV`] overrides). Refuses (returns an error) when host effects
/// are forbidden, so tests cannot read the user's app list by accident.
pub fn default_roots(platform: Platform) -> std::io::Result<Vec<PathBuf>> {
    if let Some(v) = std::env::var_os(ROOTS_ENV).filter(|v| !v.is_empty()) {
        return Ok(std::env::split_paths(&v).collect());
    }
    if crate::host::host_effects_forbidden() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "host app enumeration is refused in tests (cfg(test) or CUA_ENV_TEST_SANDBOX=1); \
             pass fixture roots instead",
        ));
    }
    let home = crate::host::HostEffects::home_dir(&crate::host::RealHost);
    Ok(match platform {
        Platform::MacOS => {
            let mut roots = vec![
                PathBuf::from("/Applications"),
                PathBuf::from("/System/Applications"),
            ];
            if let Some(h) = &home {
                roots.push(h.join("Applications"));
            }
            roots
        }
        Platform::Linux => {
            let mut roots = vec![];
            let data_home = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
            if let Some(d) = data_home {
                roots.push(d.join("applications"));
            }
            let dirs = std::env::var("XDG_DATA_DIRS")
                .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
            for d in dirs.split(':').filter(|d| !d.is_empty()) {
                roots.push(Path::new(d).join("applications"));
            }
            roots.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
            if let Some(h) = &home {
                roots.push(h.join(".local/share/flatpak/exports/share/applications"));
            }
            roots
        }
        Platform::Windows => {
            let mut roots = vec![];
            if let Some(p) = std::env::var_os("ProgramData") {
                roots.push(PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs"));
            }
            if let Some(p) = std::env::var_os("APPDATA") {
                roots.push(PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs"));
            }
            roots
        }
    })
}

/// Every app under `roots` for `platform`, de-duplicated by app id (the
/// first root wins, so list user roots after system ones only when the
/// system copy should win), sorted by name.
pub fn enumerate(roots: &[PathBuf], platform: Platform) -> Vec<HostApp> {
    let mut by_key: BTreeMap<String, HostApp> = BTreeMap::new();
    for root in roots {
        let mut budget = MAX_ENTRIES_PER_ROOT;
        walk(root, 0, platform, &mut budget, &mut |app| {
            let key = app
                .app_id
                .clone()
                .unwrap_or_else(|| app.name.clone())
                .to_ascii_lowercase();
            by_key.entry(key).or_insert(app);
        });
    }
    let mut out: Vec<HostApp> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.path.cmp(&b.path))
    });
    out
}

/// [`enumerate`], reused while nothing under `roots` changed: the picker
/// asks on every open, and re-reading every app's metadata is the slow part.
/// The check ([`fingerprint`]) only stats the directories and each app's
/// metadata file, so installing, removing or updating an app is seen on the
/// next call.
pub fn enumerate_cached(roots: &[PathBuf], platform: Platform) -> Vec<HostApp> {
    type Cached = (Vec<PathBuf>, Platform, u64, std::sync::Arc<Vec<HostApp>>);
    static CACHE: std::sync::Mutex<Vec<Cached>> = std::sync::Mutex::new(Vec::new());
    let stamp = fingerprint(roots, platform);
    let hit = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(r, p, s, _)| r.as_slice() == roots && *p == platform && *s == stamp)
        .map(|(.., apps)| apps.clone());
    if let Some(apps) = hit {
        return apps.as_ref().clone();
    }
    let apps = enumerate(roots, platform);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(r, p, ..)| !(r.as_slice() == roots && *p == platform));
    // A few root sets at most (the real machine's, a fixture's).
    if cache.len() >= 8 {
        cache.remove(0);
    }
    cache.push((
        roots.to_vec(),
        platform,
        stamp,
        std::sync::Arc::new(apps.clone()),
    ));
    apps
}

/// A cheap stamp of what [`enumerate`] would read: every visited
/// directory's and app's name and modification time (and a bundle's
/// `Info.plist`'s), without parsing anything.
pub fn fingerprint(roots: &[PathBuf], platform: Platform) -> u64 {
    use std::hash::{Hash, Hasher};
    fn mtime(p: &Path) -> Option<std::time::SystemTime> {
        std::fs::metadata(p).and_then(|m| m.modified()).ok()
    }
    fn visit(
        dir: &Path,
        depth: usize,
        platform: Platform,
        budget: &mut usize,
        h: &mut impl Hasher,
    ) {
        mtime(dir).hash(h);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let path = entry.path();
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let name = entry.file_name();
            name.hash(h);
            meta.modified().ok().hash(h);
            let lower = name.to_string_lossy().to_ascii_lowercase();
            if meta.is_dir() && lower.ends_with(".app") && platform == Platform::MacOS {
                mtime(&path.join("Contents/Info.plist")).hash(h);
            } else if meta.is_dir()
                && depth + 1 < MAX_DEPTH
                && !(platform == Platform::MacOS && lower.starts_with('.'))
            {
                visit(&path, depth + 1, platform, budget, h);
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for root in roots {
        root.hash(&mut h);
        let mut budget = MAX_ENTRIES_PER_ROOT;
        visit(root, 0, platform, &mut budget, &mut h);
    }
    h.finish()
}

fn walk(
    dir: &Path,
    depth: usize,
    platform: Platform,
    budget: &mut usize,
    found: &mut dyn FnMut(HostApp),
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        match platform {
            Platform::MacOS => {
                if meta.is_dir() && name.to_ascii_lowercase().ends_with(".app") {
                    if let Some(app) = read_app_bundle(&path) {
                        found(app);
                    }
                } else if meta.is_dir() && depth + 1 < MAX_DEPTH && !name.starts_with('.') {
                    walk(&path, depth + 1, platform, budget, found);
                }
            }
            Platform::Linux => {
                if meta.is_file() && name.ends_with(".desktop") {
                    if let Some(app) = read_desktop_entry(&path) {
                        found(app);
                    }
                } else if meta.is_dir() && depth + 1 < MAX_DEPTH {
                    walk(&path, depth + 1, platform, budget, found);
                }
            }
            Platform::Windows => {
                let lower = name.to_ascii_lowercase();
                if meta.is_file() && (lower.ends_with(".lnk") || lower.ends_with(".url")) {
                    if let Some(app) = read_shortcut(&path) {
                        found(app);
                    }
                } else if meta.is_dir() && depth + 1 < MAX_DEPTH {
                    walk(&path, depth + 1, platform, budget, found);
                }
            }
        }
    }
}

/// A macOS `.app` bundle's metadata from `Contents/Info.plist` (XML or
/// binary). A bundle without a readable plist still counts, named after its
/// directory.
pub fn read_app_bundle(bundle: &Path) -> Option<HostApp> {
    let stem = bundle.file_stem()?.to_string_lossy().into_owned();
    let dict = plist::Value::from_file(bundle.join("Contents/Info.plist"))
        .ok()
        .and_then(|v| v.into_dictionary());
    let get = |k: &str| {
        dict.as_ref()
            .and_then(|d| d.get(k))
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
    };
    // Background agents and helpers are not apps a user launches.
    let background = dict.as_ref().is_some_and(|d| {
        ["LSUIElement", "LSBackgroundOnly"].iter().any(|k| {
            d.get(k).is_some_and(|v| {
                v.as_boolean() == Some(true) || v.as_string().is_some_and(|s| s == "1")
            })
        })
    });
    if background {
        return None;
    }
    let name = get("CFBundleDisplayName")
        .or_else(|| get("CFBundleName"))
        .unwrap_or_else(|| stem.clone());
    // The bundle's directory name is what Finder shows; prefer it when the
    // plist name is a short internal one ("Code" for Visual Studio Code).
    let name = if stem.len() > name.len() && stem.to_lowercase().contains(&name.to_lowercase()) {
        stem
    } else {
        name
    };
    let icon = get("CFBundleIconFile").map(|f| {
        let f = if f.contains('.') {
            f
        } else {
            format!("{f}.icns")
        };
        bundle
            .join("Contents/Resources")
            .join(f)
            .to_string_lossy()
            .into_owned()
    });
    Some(HostApp {
        name,
        app_id: get("CFBundleIdentifier"),
        path: bundle.to_path_buf(),
        version: get("CFBundleShortVersionString").or_else(|| get("CFBundleVersion")),
        icon,
        platform: Platform::MacOS,
    })
}

/// A Linux `.desktop` entry (`[Desktop Entry]` group). Hidden, `NoDisplay`
/// and non-`Application` entries are skipped, as launchers skip them.
pub fn read_desktop_entry(file: &Path) -> Option<HostApp> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut in_group = false;
    let mut kv: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            // Localized keys (`Name[de]`) are ignored; the plain key wins.
            let k = k.trim();
            if !k.contains('[') {
                kv.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
        }
    }
    let truthy = |k: &str| kv.get(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if kv.get("Type").is_some_and(|t| t != "Application") || truthy("NoDisplay") || truthy("Hidden")
    {
        return None;
    }
    let name = kv.get("Name")?.clone();
    let desktop_id = file.file_stem()?.to_string_lossy().into_owned();
    Some(HostApp {
        name,
        app_id: Some(desktop_id),
        path: file.to_path_buf(),
        version: kv.get("X-AppVersion").cloned(),
        icon: kv.get("Icon").cloned(),
        platform: Platform::Linux,
    })
}

/// A Windows Start Menu shortcut, named after its file. Uninstallers and
/// documentation links are skipped.
pub fn read_shortcut(file: &Path) -> Option<HostApp> {
    let stem = file.file_stem()?.to_string_lossy().into_owned();
    let lower = stem.to_lowercase();
    if [
        "uninstall",
        "readme",
        "documentation",
        "help",
        "license",
        "release notes",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        return None;
    }
    // A shortcut name is not an app id: matching uses the name only.
    Some(HostApp {
        name: stem,
        app_id: None,
        path: file.to_path_buf(),
        version: None,
        icon: None,
        platform: Platform::Windows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ux::testing::{fixture_app, fixture_desktop};

    #[test]
    fn default_roots_refuse_in_tests() {
        assert!(default_roots(Platform::MacOS).is_err());
    }

    #[test]
    fn macos_bundles_are_read_from_xml_and_binary_plists() {
        let root = tempfile::tempdir().unwrap();
        fixture_app(
            root.path(),
            "Visual Studio Code",
            "com.microsoft.VSCode",
            "Code",
            false,
        );
        fixture_app(
            root.path(),
            "Blender",
            "org.blenderfoundation.blender",
            "Blender",
            true,
        );
        std::fs::create_dir_all(root.path().join("Utilities")).unwrap();
        fixture_app(
            &root.path().join("Utilities"),
            "Fixture Editor",
            "com.example.FixtureEditor",
            "Fixture Editor",
            false,
        );
        // A menu-bar agent is not offered.
        let agent = fixture_app(root.path(), "Agent", "com.example.agent", "Agent", false);
        let mut info = plist::Value::from_file(agent.join("Contents/Info.plist"))
            .unwrap()
            .into_dictionary()
            .unwrap();
        info.insert("LSUIElement".into(), plist::Value::Boolean(true));
        plist::Value::Dictionary(info)
            .to_file_xml(agent.join("Contents/Info.plist"))
            .unwrap();
        // Not a bundle.
        std::fs::write(root.path().join("notes.txt"), "x").unwrap();

        let apps = enumerate(&[root.path().to_path_buf()], Platform::MacOS);
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Blender", "Fixture Editor", "Visual Studio Code"]);
        let code = &apps[2];
        assert_eq!(code.app_id.as_deref(), Some("com.microsoft.VSCode"));
        assert_eq!(code.version.as_deref(), Some("1.0.0"));
        // Compared by components: the icon path uses the host's separator.
        let icon = code.icon.as_deref().unwrap();
        assert!(
            Path::new(icon).ends_with("Contents/Resources/app.icns"),
            "{icon}"
        );
    }

    #[test]
    fn linux_desktop_entries_skip_hidden_and_non_apps() {
        let root = tempfile::tempdir().unwrap();
        fixture_desktop(root.path(), "code", "Visual Studio Code", "code %F", "");
        fixture_desktop(
            root.path(),
            "hidden",
            "Hidden",
            "hidden",
            "NoDisplay=true\n",
        );
        std::fs::write(
            root.path().join("link.desktop"),
            "[Desktop Entry]\nType=Link\nName=A Link\nURL=https://example.com\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("i18n.desktop"),
            "[Desktop Entry]\nType=Application\nName[de]=Bearbeiter\nName=Editor\nExec=ed\n\
             [Desktop Action New]\nName=New Window\n",
        )
        .unwrap();
        let apps = enumerate(&[root.path().to_path_buf()], Platform::Linux);
        let names: Vec<(&str, Option<&str>)> = apps
            .iter()
            .map(|a| (a.name.as_str(), a.app_id.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("Editor", Some("i18n")),
                ("Visual Studio Code", Some("code"))
            ]
        );
    }

    #[test]
    fn windows_shortcuts_skip_uninstallers() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("Blender Foundation");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Blender.lnk"), b"L").unwrap();
        std::fs::write(dir.join("Uninstall Blender.lnk"), b"L").unwrap();
        let apps = enumerate(&[root.path().to_path_buf()], Platform::Windows);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Blender");
    }

    #[test]
    fn the_cached_list_follows_installs_removals_and_updates() {
        let root = tempfile::tempdir().unwrap();
        let roots = [root.path().to_path_buf()];
        let paint = fixture_app(root.path(), "Paint", "com.example.paint", "Paint", false);
        let list = || enumerate_cached(&roots, Platform::MacOS);
        assert_eq!(list().len(), 1);
        assert_eq!(
            list(),
            enumerate(&roots, Platform::MacOS),
            "a hit is the same list"
        );
        // Installed.
        fixture_app(root.path(), "Write", "com.example.write", "Write", true);
        assert_eq!(list().len(), 2);
        // Updated in place: a new Info.plist (a later modification time).
        let plist = paint.join("Contents/Info.plist");
        let text = std::fs::read_to_string(&plist)
            .unwrap()
            .replace("1.0.0", "2.0.0");
        std::fs::write(&plist, text).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&plist)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let apps = list();
        let p = apps
            .iter()
            .find(|a| a.app_id.as_deref() == Some("com.example.paint"));
        assert_eq!(p.and_then(|a| a.version.as_deref()), Some("2.0.0"));
        // Removed.
        std::fs::remove_dir_all(&paint).unwrap();
        assert_eq!(list().len(), 1);
    }

    #[test]
    fn duplicates_collapse_first_root_wins() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fixture_app(
            a.path(),
            "Blender",
            "org.blenderfoundation.blender",
            "Blender",
            false,
        );
        fixture_app(
            b.path(),
            "Blender",
            "org.blenderfoundation.blender",
            "Blender",
            false,
        );
        let apps = enumerate(
            &[a.path().to_path_buf(), b.path().to_path_buf()],
            Platform::MacOS,
        );
        assert_eq!(apps.len(), 1);
        assert!(apps[0].path.starts_with(a.path()));
    }
}
