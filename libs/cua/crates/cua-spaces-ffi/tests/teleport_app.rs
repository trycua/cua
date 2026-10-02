// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Teleport an app…" through the SDK surface (feature `teleport`): the
//! catalog over fixture app directories, search, recents, drag payloads,
//! dropped bundles, window-drag events classified from fixture windows,
//! and plan JSON round trips.
//!
//! Host-safe: every app root is a temp directory of fixture bundles, the
//! teleport host is a `FakeHost`, and the real machine's windows, event tap
//! and thumbnails are asserted refused (`CUA_ENV_TEST_SANDBOX=1`).

use std::path::Path;
use std::sync::Arc;

use cua_sdk::CuaError;
use cua_spaces_ffi::{
    Teleport, TeleportCapability, TeleportCatalogOptions, TeleportMove, TeleportWindowDragListener,
};
use cua_teleport::FakeHost;

fn teleport() -> Arc<Teleport> {
    // SAFETY: set before any thread of this test binary reads it.
    unsafe { std::env::set_var("CUA_ENV_TEST_SANDBOX", "1") };
    Teleport::with_host(Arc::new(FakeHost::new()))
}

fn bundle(root: &Path, name: &str, id: &str) {
    let b = root.join(format!("{name}.app/Contents"));
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(
        b.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundleShortVersionString</key><string>2.0</string>
</dict></plist>"#
        ),
    )
    .unwrap();
}

/// A `.desktop` entry, the way Linux lists installed apps.
fn desktop_entry(root: &Path, name: &str, desktop_id: &str) {
    std::fs::write(
        root.join(format!("{desktop_id}.desktop")),
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec={desktop_id}\n"),
    )
    .unwrap();
}

/// The same four apps as `.app` bundles (what macOS lists, and what a drop
/// names) and as what the catalog lists elsewhere: `.desktop` entries on
/// Linux, Start Menu `.lnk` shortcuts on Windows.
fn fixture_apps() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (name, bundle_id, desktop_id) in [
        ("Visual Studio Code", "com.microsoft.VSCode", "code"),
        ("Firefox", "org.mozilla.firefox", "firefox"),
        ("Blender", "org.blenderfoundation.blender", "blender"),
        (
            "Fixture Notes",
            "com.example.fixture-notes",
            "com.example.fixture-notes",
        ),
    ] {
        bundle(root.path(), name, bundle_id);
        if cfg!(windows) {
            std::fs::write(root.path().join(format!("{name}.lnk")), b"").unwrap();
        } else if !cfg!(target_os = "macos") {
            desktop_entry(root.path(), name, desktop_id);
        }
    }
    root
}

/// An unknown app's id: its bundle or desktop id, or on Windows (a
/// shortcut carries only a name) the slug of its name.
const NOTES_ID: &str = if cfg!(windows) {
    "fixture-notes"
} else {
    "com.example.fixture-notes"
};

fn options(root: &Path, arch: Option<&str>) -> TeleportCatalogOptions {
    TeleportCatalogOptions {
        roots: Some(vec![root.to_string_lossy().into_owned()]),
        space_os: Some("linux".into()),
        space_arch: arch.map(str::to_string),
        recents_path: Some(root.join("recents.json").to_string_lossy().into_owned()),
    }
}

#[tokio::test]
async fn the_catalog_lists_every_app_with_capabilities_recents_first() {
    let t = teleport();
    let root = fixture_apps();
    let all = t
        .catalog(Some(options(root.path(), Some("aarch64"))))
        .await
        .unwrap();
    let rows: Vec<(&str, TeleportCapability)> =
        all.iter().map(|e| (e.id.as_str(), e.capability)).collect();
    assert_eq!(
        rows,
        [
            ("firefox", TeleportCapability::Full),
            ("vscode", TeleportCapability::InstallOnly),
            ("blender", TeleportCapability::Unsupported),
            (NOTES_ID, TeleportCapability::Unsupported),
        ]
    );
    let blender = all.iter().find(|e| e.id == "blender").unwrap();
    assert!(blender.reason.as_deref().unwrap().contains("aarch64"));
    let vs = all.iter().find(|e| e.id == "vscode").unwrap();
    assert_eq!(
        vs.moves,
        [TeleportMove::AppOnly, TeleportMove::AppWithFiles]
    );
    assert_eq!(vs.install_source.as_deref(), Some("manifest"));
    assert_eq!(vs.install_id.as_deref(), Some("vscode"));
    assert_eq!(vs.launch_bin.as_deref(), Some("code"));

    // x86_64 enables Blender; recents move a row to the top.
    t.record_recent(
        "blender".into(),
        Some(
            root.path()
                .join("recents.json")
                .to_string_lossy()
                .into_owned(),
        ),
    )
    .unwrap();
    let all = t
        .catalog(Some(options(root.path(), Some("x86_64"))))
        .await
        .unwrap();
    assert_eq!(all[0].id, "blender");
    assert_eq!(all[0].capability, TeleportCapability::InstallOnly);
    assert!(all[0].last_used_ms.is_some());

    let hits = t.search_catalog(all.clone(), "visual studio".into());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "vscode");
    assert_eq!(t.search_catalog(all, "".into()).len(), 4);
}

#[tokio::test]
async fn the_real_machine_is_refused_in_tests() {
    let t = teleport();
    let e = t.catalog(None).await.unwrap_err();
    assert!(matches!(e, CuaError::PermissionDenied(_)), "{e}");
    assert!(matches!(
        t.list_windows(),
        Err(CuaError::PermissionDenied(_))
    ));
    assert!(matches!(
        t.capture_window_thumbnail(1, None),
        Err(CuaError::PermissionDenied(_))
    ));
    struct Nope;
    impl TeleportWindowDragListener for Nope {
        fn on_event(&self, _: cua_spaces_ffi::TeleportWindowDragEvent) {}
    }
    assert!(t.start_window_drag(Arc::new(Nope)).is_err());
    assert_eq!(t.window_drag_supported(), cfg!(target_os = "macos"));
}

#[test]
fn drops_and_dropped_bundles() {
    let t = teleport();
    let root = fixture_apps();
    let app = root.path().join("Visual Studio Code.app");
    // A file URI names a Windows drive path as `file:///C:/...`, and the
    // parser gives it back with forward slashes.
    let app_str = app.to_string_lossy().replace('\\', "/");
    let uri_path = if app_str.starts_with('/') {
        app_str.clone()
    } else {
        format!("/{app_str}")
    };
    let uri = format!("file://{}/", uri_path.replace(' ', "%20"));
    let d = t.parse_drop(vec![
        uri,
        "/tmp/project".into(),
        "https://example.com".into(),
    ]);
    assert_eq!(d.kind, "app");
    assert_eq!(d.apps, [app_str]);
    assert_eq!(d.files, ["/tmp/project"]);
    assert_eq!(d.urls, ["https://example.com"]);
    assert_eq!(t.parse_drop(vec!["/tmp/a.txt".into()]).kind, "files");

    let e = t.catalog_entry_for_path(d.apps[0].clone(), None).unwrap();
    assert_eq!(
        (e.id.as_str(), e.capability),
        ("vscode", TeleportCapability::InstallOnly)
    );
    assert!(matches!(
        t.catalog_entry_for_path("/tmp/a.txt".into(), None),
        Err(CuaError::InvalidArgument(_))
    ));
    let w = t.catalog_entry_for_name("Firefox".into(), None).unwrap();
    assert_eq!(w.capability, TeleportCapability::Full);
    assert!(matches!(
        t.catalog_entry_for_name(
            "x".into(),
            Some(TeleportCatalogOptions {
                space_os: Some("plan9".into()),
                ..Default::default()
            })
        ),
        Err(CuaError::InvalidArgument(_))
    ));
}
