// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The app core (`cua-spaces-app-core`) behind one Tauri command, for
//! callers without the wasm build (tools, tests, future windows). The
//! webview itself runs the same core as wasm; the SwiftUI app binds it
//! through the cua SDK. One core, three routes, the same answers
//! (`tests/app_core_parity.rs`).

/// Calls a core method (`cua_spaces_app_core::dispatch::METHODS`) with
/// JSON arguments and returns JSON.
#[tauri::command]
pub fn app_core_call(method: String, args: String) -> Result<String, String> {
    cua_spaces_app_core::dispatch::call(&method, &args).map_err(|e| e.to_string())
}
