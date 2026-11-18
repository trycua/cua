// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The mobile binding is one more host of the core's parity flows: every flow
//! replays through `cua_spaces_mobile_core::call`, the function the C ABI (and
//! so the React Native module) calls, and must match the goldens the Mac, web
//! and Tauri apps match.

use cua_spaces_app_core::parity::{FLOWS, JsonHost, check_all};

#[test]
fn every_parity_flow_matches_its_golden_through_the_mobile_entry_point() {
    let host = JsonHost(|m: &str, a: &str| cua_spaces_mobile_core::call(m, a));
    let results = check_all(&host);
    assert_eq!(results.len(), FLOWS.len());
    for (name, ok, _) in results {
        assert!(ok.is_ok(), "{name}: {ok:?}");
    }
}

#[test]
fn the_method_list_matches_the_core() {
    let m: Vec<String> = serde_json::from_str(&cua_spaces_mobile_core::methods()).unwrap();
    assert_eq!(
        m.len(),
        cua_spaces_app_core::dispatch::METHODS.len()
            + cua_spaces_mobile_core::device::METHODS.len()
            + cua_spaces_mobile_core::store::METHODS.len()
    );
}
