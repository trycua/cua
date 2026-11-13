//! The Swift app names its Application Support folder (where the first-run
//! state, `onboarding.json`, and the settings live) from the bundle's
//! `CuaChannel`; the names must be `CuaChannel`'s (CUA-1776). A Local app
//! that read Stable's folder would show the first run again at every launch
//! in the one case, and share Stable's settings in the other.
//!
//! The Swift source is read straight from the repository; the check is
//! skipped where the app is not there (the crate published on crates.io).

use cua_home::channel::CuaChannel;
use std::path::PathBuf;

fn source() -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../../../apps/cua-spaces-macos/Sources/CuaSpacesMacKit/Models/AppEnvironment.swift",
    );
    std::fs::read_to_string(path).ok()
}

#[test]
fn the_swift_app_support_folder_is_the_channels() {
    let Some(swift) = source() else {
        eprintln!("SKIP: no apps/cua-spaces-macos in this checkout");
        return;
    };
    // `bundleIdentifier = isLocalChannel ? "<local>" : "<stable>"`, and
    // the support folder is that identifier below Application Support.
    let want = format!(
        "isLocalChannel ? \"{}\" : \"{}\"",
        CuaChannel::Local.app_support_dir_name(),
        CuaChannel::Stable.app_support_dir_name()
    );
    assert!(
        swift.contains(&want),
        "AppEnvironment.bundleIdentifier no longer reads `{want}`: the Swift app's support \
         folder (onboarding.json, settings.json) must be CuaChannel::app_support_dir_name"
    );
    assert!(
        swift.contains(".appendingPathComponent(bundleIdentifier, isDirectory: true)"),
        "AppEnvironment.supportDirectory is not built from bundleIdentifier"
    );
    // The first-run state and the settings sit in that folder, nowhere else.
    for file in ["settings.json", "onboarding.json"] {
        assert!(
            swift.contains(&format!(
                "supportDirectory.appendingPathComponent(\"{file}\")"
            )),
            "{file} is not read from the support folder"
        );
    }
    // Both channels' folders differ, and Local's is not inside Stable's.
    assert_ne!(
        CuaChannel::Stable.app_support_dir(),
        CuaChannel::Local.app_support_dir()
    );
}
