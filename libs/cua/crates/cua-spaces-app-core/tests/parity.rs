// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The scripted flows, replayed on the Rust core, against the goldens every
//! other host (the Tauri app, the webview, Swift) must match.
//!
//! `UPDATE_PARITY=1` rewrites the goldens (review the diff: a golden change
//! is a behaviour change in every shell).

use cua_spaces_app_core::parity::{FLOWS, TypedHost, check_all};

#[test]
fn every_flow_matches_its_golden() {
    let results = check_all(&TypedHost);
    assert_eq!(results.len(), FLOWS.len());
    let update = std::env::var("UPDATE_PARITY").is_ok_and(|v| v == "1");
    let mut failures = Vec::new();
    for (name, ok, transcript) in results {
        if update {
            let path = format!("{}/parity/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
            let mut text = serde_json::to_string_pretty(&transcript).unwrap();
            text.push('\n');
            std::fs::write(&path, text).unwrap();
            continue;
        }
        if let Err(e) = ok {
            failures.push(format!("{name}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "parity drift:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_json_entry_point_replays_the_same_flows() {
    // The path the webview's wasm shim and the Tauri command take.
    let host = cua_spaces_app_core::parity::JsonHost(|m: &str, a: &str| {
        cua_spaces_app_core::dispatch::call(m, a).map_err(|e| e.to_string())
    });
    if std::env::var("UPDATE_PARITY").is_ok_and(|v| v == "1") {
        return;
    }
    for (name, ok, _) in check_all(&host) {
        assert!(ok.is_ok(), "{name}: {ok:?}");
    }
}
