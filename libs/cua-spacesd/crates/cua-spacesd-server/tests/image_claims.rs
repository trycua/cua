// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every Space image lists each feature this server reports on its OS.
//!
//! `cua-spacesd doctor --strict` turns a supported feature that the image's
//! claims leave out (`capabilities.unclaimed`) into a failed release gate.
//! A new feature therefore has to be claimed (required, optional or not
//! applicable) by every image at once, not only by the Linux one.

use std::path::{Path, PathBuf};

use cua_spacesd_client::manifest::Manifest;
use cua_spacesd_server::{filesystem::runas, privilege, process, server, services};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn strings(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|l| l.as_array())
        .map(|l| {
            l.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The feature claims of an image definition: `claims` in `image.json`,
/// the top level in the hand-written VM manifests.
fn claims(rel: &str) -> Manifest {
    let path = repo_root().join(rel);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let doc: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
    let claims = doc.get("claims").unwrap_or(&doc);
    Manifest {
        features_required: strings(claims, "features_required"),
        features_optional: strings(claims, "features_optional"),
        features_not_applicable: strings(claims, "features_not_applicable"),
        ..Manifest::default()
    }
}

/// Reported (supported) on every OS, whatever the guest has installed.
fn everywhere() -> Vec<&'static str> {
    vec![
        "fs_watch",
        "tunnel.forward",
        "hotspot",
        process::services::FEATURE,
        runas::MULTIPART_FEATURE,
        process::EXEC_LEDGER_FEATURE,
        server::RELAY_ATTACH_FEATURE,
        services::teleport::WIPE_FEATURE,
    ]
}

/// Also reported on Linux and macOS guests.
fn unix() -> Vec<&'static str> {
    let mut v = everywhere();
    v.extend([
        runas::RUN_AS_FEATURE,
        runas::XATTR_FEATURE,
        privilege::PROCESS_RUN_AS_FEATURE,
        services::volume::FEATURE,
        services::volume::BINDS_FEATURE,
    ]);
    v
}

#[test]
fn every_image_claims_the_features_spacesd_reports() {
    let images = [
        ("libs/images/linux/image.json", unix()),
        ("libs/images/omarchy/image.json", unix()),
        ("libs/images/bench/osworld/image.json", unix()),
        ("libs/images/bench/web/image.json", unix()),
        ("libs/images/macos/image.json", unix()),
        ("scripts/images/manifests/windows-2022.json", everywhere()),
    ];
    let mut missing = Vec::new();
    for (rel, features) in images {
        let m = claims(rel);
        for f in features {
            if !m.lists(f) {
                missing.push(format!("{rel}: {f}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "unclaimed features fail `doctor --strict` (capabilities.unclaimed); \
         add them to features_required, features_optional or \
         features_not_applicable:\n  {}",
        missing.join("\n  ")
    );
}
