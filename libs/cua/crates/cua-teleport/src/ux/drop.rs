// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drag payloads: what was dropped on a Space tile, window or viewer.
//!
//! A drag from Finder or the Dock arrives as paths (Tauri, AppKit) or as
//! `file://` URIs (`text/uri-list`, `public.file-url`); a browser link
//! arrives as an `http(s)` URL. [`parse`] sorts them into apps (a macOS
//! `.app` bundle, a Linux `.desktop` entry, a Windows `.lnk`), plain files
//! or folders, and URLs. An app drop opens "Teleport an app…" for that app;
//! files and folders keep the drop zone's file transfer.

use serde::{Deserialize, Serialize};

use super::apps::{HostApp, read_app_bundle, read_desktop_entry, read_shortcut};

/// Most items one drop may carry (a drop is a gesture, not a bulk import).
pub const MAX_DROP_ITEMS: usize = 256;

/// A parsed drop.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropPayload {
    /// App bundles, `.desktop` entries or shortcuts (absolute paths).
    pub apps: Vec<String>,
    /// Files and folders (absolute paths).
    pub files: Vec<String>,
    /// Web URLs.
    pub urls: Vec<String>,
    /// Entries that were none of these (relative paths, other schemes).
    pub ignored: Vec<String>,
}

/// What a drop means for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropKind {
    /// Nothing usable.
    Empty,
    /// At least one app: teleport the first one.
    App,
    /// Files or folders only: the file transfer.
    Files,
    /// URLs only.
    Url,
}

impl DropPayload {
    /// What the drop means: an app wins, then files, then URLs.
    pub fn kind(&self) -> DropKind {
        if !self.apps.is_empty() {
            DropKind::App
        } else if !self.files.is_empty() {
            DropKind::Files
        } else if !self.urls.is_empty() {
            DropKind::Url
        } else {
            DropKind::Empty
        }
    }
}

/// Parses dropped items: paths, `file://` URIs, `http(s)` URLs, or whole
/// `text/uri-list` blobs (one per line, `#` comments skipped). Order is
/// kept; duplicates are dropped; at most [`MAX_DROP_ITEMS`] are read.
pub fn parse<S: AsRef<str>>(items: &[S]) -> DropPayload {
    let mut out = DropPayload::default();
    let mut seen = std::collections::HashSet::new();
    let lines = items
        .iter()
        .flat_map(|i| i.as_ref().lines().map(str::to_string).collect::<Vec<_>>())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(MAX_DROP_ITEMS);
    for line in lines {
        if !seen.insert(line.clone()) {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            out.urls.push(line);
            continue;
        }
        let path = if lower.starts_with("file://") {
            match file_uri_to_path(&line) {
                Some(p) => p,
                None => {
                    out.ignored.push(line);
                    continue;
                }
            }
        } else {
            line.clone()
        };
        if !is_absolute(&path) {
            out.ignored.push(line);
            continue;
        }
        let trimmed = trim_trailing_separators(&path);
        if is_app_path(&trimmed) {
            out.apps.push(trimmed);
        } else {
            out.files.push(trimmed);
        }
    }
    out
}

/// Whether `path` names an app: a `.app` bundle, a `.desktop` entry, or a
/// `.lnk` shortcut.
pub fn is_app_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".app") || lower.ends_with(".desktop") || lower.ends_with(".lnk")
}

/// Reads a dropped app's metadata (only the bundle's `Info.plist`, the
/// `.desktop` file, or the shortcut's name).
pub fn read_dropped_app(path: &str) -> Option<HostApp> {
    let p = std::path::Path::new(path);
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".app") {
        read_app_bundle(p)
    } else if lower.ends_with(".desktop") {
        read_desktop_entry(p)
    } else if lower.ends_with(".lnk") {
        read_shortcut(p)
    } else {
        None
    }
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
        || (p.len() > 2
            && p.as_bytes()[1] == b':'
            && (p.as_bytes()[2] == b'\\' || p.as_bytes()[2] == b'/'))
        || p.starts_with(r"\\")
}

fn trim_trailing_separators(p: &str) -> String {
    let t = p.trim_end_matches(['/', '\\']);
    if t.is_empty() {
        p.to_string()
    } else {
        t.to_string()
    }
}

/// `file:///a%20b/` -> `/a b/` (and `file://localhost/...`,
/// `file:///C:/...`). `None` for another host.
pub fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = &uri["file://".len()..];
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let decoded = percent_decode(rest)?;
    // `file:///C:/x` is a Windows drive path.
    let b = decoded.as_bytes();
    if b.len() > 3 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_alphabetic() {
        return Some(decoded[1..].to_string());
    }
    Some(decoded)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases the TypeScript parser also runs (`testdata/drops.json`).
    #[test]
    fn shared_cases() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("testdata/drops.json")).unwrap();
        for case in v["cases"].as_array().unwrap() {
            let items: Vec<String> = serde_json::from_value(case["items"].clone()).unwrap();
            let p = parse(&items);
            let kind = serde_json::to_value(p.kind()).unwrap();
            assert_eq!(kind, case["kind"], "{items:?}");
            for k in ["apps", "files", "urls", "ignored"] {
                let got = serde_json::to_value(&p).unwrap()[k].clone();
                assert_eq!(got, case[k], "{k} of {items:?}");
            }
        }
    }

    #[test]
    fn finder_and_dock_drops_become_apps() {
        let p = parse(&[
            "file:///Applications/Visual%20Studio%20Code.app/",
            "/Applications/Blender.app",
            "/Users/me/project",
            "file:///Users/me/notes.txt",
        ]);
        assert_eq!(
            p.apps,
            [
                "/Applications/Visual Studio Code.app",
                "/Applications/Blender.app"
            ]
        );
        assert_eq!(p.files, ["/Users/me/project", "/Users/me/notes.txt"]);
        assert_eq!(p.kind(), DropKind::App);
    }

    #[test]
    fn files_and_folders_keep_the_file_path() {
        let p = parse(&["/tmp/a.txt", "/tmp/dir/"]);
        assert_eq!(p.files, ["/tmp/a.txt", "/tmp/dir"]);
        assert_eq!(p.kind(), DropKind::Files);
    }

    #[test]
    fn uri_lists_urls_and_junk() {
        let p = parse(&[
            "# comment\r\nhttps://example.com/x\r\nfile://localhost/usr/share/applications/code.desktop\r\n",
            "file://otherhost/share/x",
            "relative/path",
            "https://example.com/x",
        ]);
        assert_eq!(p.urls, ["https://example.com/x"]);
        assert_eq!(p.apps, ["/usr/share/applications/code.desktop"]);
        assert_eq!(p.ignored, ["file://otherhost/share/x", "relative/path"]);
        assert_eq!(parse::<&str>(&[]).kind(), DropKind::Empty);
        assert_eq!(parse(&["https://a.b"]).kind(), DropKind::Url);
    }

    #[test]
    fn windows_paths() {
        let p = parse(&[
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Blender.lnk",
            "file:///C:/Users/me/doc.blend",
        ]);
        assert_eq!(p.apps.len(), 1);
        assert_eq!(p.files, ["C:/Users/me/doc.blend"]);
    }

    #[test]
    fn bad_escapes_are_ignored_and_items_are_bounded() {
        assert_eq!(parse(&["file:///a%zz"]).ignored.len(), 1);
        let many: Vec<String> = (0..1000).map(|i| format!("/tmp/{i}")).collect();
        assert_eq!(parse(&many).files.len(), MAX_DROP_ITEMS);
    }

    #[test]
    fn a_dropped_fixture_bundle_reads_its_metadata() {
        let root = tempfile::tempdir().unwrap();
        let b = crate::ux::testing::fixture_app(
            root.path(),
            "Blender",
            "org.blenderfoundation.blender",
            "Blender",
            false,
        );
        let app = read_dropped_app(b.to_str().unwrap()).unwrap();
        assert_eq!(app.app_id.as_deref(), Some("org.blenderfoundation.blender"));
        assert!(read_dropped_app("/tmp/x.txt").is_none());
    }
}
