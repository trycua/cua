// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! App icons as PNG bytes, in memory, for pickers.

use cua_icon_cache::{IconCache, IconKey, Lookup, Thumbnails};
use std::path::Path;

/// `path`'s icon through the SDK's one icon cache ([`cua_icon_cache`]),
/// keyed by the app's bundle id (or `.desktop` id, else its path) and
/// version: rendered once per app version on this host, then answered from
/// memory or `$CUA_HOME/cache/icons`. The 32 px PNG for `size` up to 32,
/// else the 64 px one. Every UI asks here instead of keeping its own cache.
pub fn app_icon_png_cached(path: &str, size: u32) -> Option<Vec<u8>> {
    let key = host_icon_key(path);
    let cache = IconCache::shared();
    match cache.lookup(&key) {
        Lookup::Hit(icon) => return Some(icon.for_size(size).to_vec()),
        Lookup::Negative => return None,
        Lookup::Miss => {}
    }
    let source = app_icon_png(path, cua_icon_cache::SIZE_2X);
    cache
        .put(&key, source.as_deref())
        .map(|icon| icon.for_size(size).to_vec())
}

/// A PNG preview of this machine's window `window_id` (at most
/// `max_width` px wide) through the SDK's short-lived preview cache: the
/// window-drag preview source every picker tile and drag uses.
pub fn capture_thumbnail_png_cached(
    window_id: u32,
    max_width: usize,
) -> Result<Option<Vec<u8>>, super::UxError> {
    Thumbnails::shared()
        .get_or_capture(&format!("host:{window_id}:{max_width}"), || {
            super::window::capture_thumbnail_png(window_id, max_width)
        })
        .map(|png| png.map(|p| p.as_ref().clone()))
}

/// This host's app identity for the icon cache: bundle id and version, the
/// `.desktop` id, else the path and its modification time.
pub fn host_icon_key(path: &str) -> IconKey {
    let scope = if cfg!(target_os = "macos") {
        "host-macos"
    } else if cfg!(windows) {
        "host-windows"
    } else {
        "host-linux"
    };
    let p = Path::new(path);
    let lower = path.to_ascii_lowercase();
    let app = if lower.ends_with(".app") || lower.ends_with(".app/") {
        super::apps::read_app_bundle(p)
    } else if lower.ends_with(".desktop") {
        super::apps::read_desktop_entry(p)
    } else {
        None
    };
    match app {
        Some(a) if a.app_id.as_deref().is_some_and(|id| !id.is_empty()) => IconKey::new(
            scope,
            a.app_id.as_deref().unwrap_or_default(),
            a.version.as_deref().unwrap_or_default(),
        ),
        _ => {
            let modified = std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default();
            IconKey::new(scope, &format!("path:{path}"), &modified)
        }
    }
}

/// `path`'s icon (an app bundle, `.desktop` entry, or any file), at most
/// `size` points, as PNG. macOS asks NSWorkspace (so asset-catalog icons
/// work); Linux reads a PNG icon the `.desktop` entry names from the icon
/// theme; Windows returns `None`.
pub fn app_icon_png(path: &str, size: u32) -> Option<Vec<u8>> {
    let size = size.clamp(16, 512);
    #[cfg(target_os = "macos")]
    {
        if path.to_ascii_lowercase().ends_with(".desktop") {
            return linux_icon(path, size);
        }
        super::window_macos::icon_png(path, size as f64)
    }
    #[cfg(not(target_os = "macos"))]
    {
        linux_icon(path, size)
    }
}

/// A `.desktop` entry's `Icon=`: an absolute PNG, or a name looked up in
/// the hicolor theme and pixmaps (largest size not above `size`, else the
/// smallest larger one).
fn linux_icon(path: &str, size: u32) -> Option<Vec<u8>> {
    let app = super::apps::read_desktop_entry(Path::new(path))?;
    let icon = app.icon?;
    if icon.starts_with('/') {
        return icon
            .ends_with(".png")
            .then(|| std::fs::read(&icon).ok())
            .flatten();
    }
    let mut roots = vec![];
    if let Some(h) = crate::host::HostEffects::home_dir(&crate::host::RealHost) {
        roots.push(h.join(".local/share/icons/hicolor"));
    }
    roots.push("/usr/share/icons/hicolor".into());
    let mut best: Option<(u32, std::path::PathBuf)> = None;
    for root in &roots {
        let Ok(dirs) = std::fs::read_dir(root) else {
            continue;
        };
        for d in dirs.flatten() {
            let name = d.file_name().to_string_lossy().into_owned();
            let Some(px) = name.split('x').next().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let f = d.path().join("apps").join(format!("{icon}.png"));
            if !f.is_file() {
                continue;
            }
            let better = match &best {
                None => true,
                Some((b, _)) => {
                    (px <= size && (*b > size || px > *b)) || (px > size && *b > size && px < *b)
                }
            };
            if better {
                best = Some((px, f));
            }
        }
    }
    let file = best
        .map(|(_, f)| f)
        .or_else(|| Some(Path::new("/usr/share/pixmaps").join(format!("{icon}.png"))))?;
    std::fs::read(file).ok()
}

#[cfg(test)]
mod cache_key_tests {
    use super::*;
    use crate::ux::testing::{fixture_app, fixture_desktop};

    #[test]
    fn host_icons_key_on_the_bundle_id_and_version_not_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = fixture_app(
            dir.path(),
            "Slack",
            "com.tinyspeck.slackmacgap",
            "Slack",
            true,
        );
        let other = tempfile::tempdir().unwrap();
        let b = fixture_app(
            other.path(),
            "Slack Copy",
            "com.tinyspeck.slackmacgap",
            "Slack",
            false,
        );
        let key = |p: &Path| host_icon_key(p.to_str().unwrap());
        assert_eq!(key(&a), key(&b), "one app, two paths: one entry");
        assert!(key(&a).as_str().contains("com.tinyspeck.slackmacgap"));
        assert!(key(&a).as_str().ends_with("1.0.0"));
        let d = fixture_desktop(dir.path(), "org.gnome.gedit", "Text Editor", "gedit %U", "");
        assert!(key(&d).as_str().contains("org.gnome.gedit"));
        // No metadata: the path and its modification time.
        let f = dir.path().join("tool");
        std::fs::write(&f, b"x").unwrap();
        assert!(key(&f).as_str().contains("path:"));
    }
}
