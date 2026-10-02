// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The app core's parity flows, replayed through this app's
//! `app_core_call` Tauri command: the same transcripts the Rust core, the
//! webview (wasm) and the SwiftUI app (UniFFI) produce.

use cua_spaces_app_core::parity::{check_all, JsonHost, FLOWS};
use cua_spaces_lib::app_core::app_core_call;

#[test]
fn every_flow_matches_its_golden_through_the_tauri_command() {
    let host =
        JsonHost(|method: &str, args: &str| app_core_call(method.to_string(), args.to_string()));
    let results = check_all(&host);
    assert_eq!(results.len(), FLOWS.len());
    for (name, ok, _) in results {
        assert!(ok.is_ok(), "{name}: {ok:?}");
    }
}

#[test]
fn the_tray_and_notch_geometry_are_the_cores() {
    assert_eq!(cua_spaces_lib::tray::status_text(3), "3 Spaces");
    let m = cua_spaces_lib::geometry::LogicalRect::new(0.0, 0.0, 1512.0, 982.0);
    assert!(cua_spaces_lib::geometry::looks_notched(m));
}
