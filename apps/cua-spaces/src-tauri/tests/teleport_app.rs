// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Teleport an app…" through the app's command layer, hermetic: the app
//! catalog reads a fixture `Applications` directory under a temp home (never
//! this machine's apps), the teleport host is a `FakeHost`, and recents go to
//! the temp home.

use cua_spaces_lib::core::{AppCore, CoreConfig};

fn bundle(root: &std::path::Path, name: &str, id: &str) -> std::path::PathBuf {
    let b = root.join(format!("{name}.app"));
    std::fs::create_dir_all(b.join("Contents")).unwrap();
    std::fs::write(
        b.join("Contents/Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>{id}</string>\
             <key>CFBundleName</key><string>{name}</string></dict></plist>"
        ),
    )
    .unwrap();
    b
}

#[tokio::test]
async fn the_catalog_classifies_fixture_apps_and_dropped_bundles() {
    std::env::set_var("CUA_ENV_TEST_SANDBOX", "1");
    let home = tempfile::tempdir().unwrap();
    let apps = home.path().join("Applications");
    bundle(&apps, "Visual Studio Code", "com.microsoft.VSCode");
    let ff = bundle(&apps, "Firefox", "org.mozilla.firefox");
    bundle(&apps, "Fixture Paint", "com.example.paint");
    let core = AppCore::new(CoreConfig::hermetic(home.path()));

    let catalog = core.teleport_app_catalog(None).await.unwrap();
    let rows: Vec<(&str, &str)> = catalog
        .iter()
        .map(|e| (e.id.as_str(), e.capability.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("firefox", "full"),
            ("vscode", "install_only"),
            ("com.example.paint", "unsupported")
        ]
    );

    // A Finder / Dock drop of a bundle resolves to its row.
    let e = core.teleport_entry_for_path(ff.to_str().unwrap()).unwrap();
    assert_eq!(e.id, "firefox");
    assert!(core.teleport_entry_for_path("/tmp/notes.txt").is_err());
    // A dragged window without a bundle resolves by its owner's name.
    assert_eq!(
        core.teleport_entry_for_window(None, "Visual Studio Code")
            .id,
        "vscode"
    );

    // An unknown Space is an error, not a guess.
    assert!(core
        .teleport_app_catalog(Some("direct:127.0.0.1:1"))
        .await
        .is_err());
}
