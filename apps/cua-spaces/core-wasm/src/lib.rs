// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces app core in the webview: `call(method, argsJson)`
//! returns the result as JSON, or throws the core's error.
//! `src/core/index.ts` wraps it; `src/model/*.ts` expose typed functions.

use wasm_bindgen::prelude::*;

/// Calls a core method (`cua_spaces_app_core::dispatch::METHODS`).
#[wasm_bindgen]
pub fn call(method: &str, args: &str) -> Result<String, JsError> {
    cua_spaces_app_core::dispatch::call(method, args).map_err(|e| JsError::new(&e.to_string()))
}

/// Every method `call` answers, as JSON.
#[wasm_bindgen]
pub fn methods() -> String {
    format!(
        "[{}]",
        cua_spaces_app_core::dispatch::METHODS
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// A parity host backed by a JS function `(method, argsJson) => resultJson`:
/// the webview's `src/model/*.ts` layer.
struct JsHost<'a>(&'a js_sys::Function);

impl cua_spaces_app_core::parity::Host for JsHost<'_> {
    fn call(&self, method: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
        let r = self
            .0
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(method),
                &JsValue::from_str(&args.to_string()),
            )
            .map_err(|e| format!("{method}: {e:?}"))?;
        let text = r
            .as_string()
            .ok_or_else(|| format!("{method}: the host returned a non-string"))?;
        serde_json::from_str(&text).map_err(|e| format!("{method}: {e}"))
    }
}

/// Replays the parity flow `name` (its JSON) through `host` and returns the
/// transcript as JSON.
#[wasm_bindgen(js_name = runFlow)]
pub fn run_flow(name: &str, flow: &str, host: &js_sys::Function) -> Result<String, JsError> {
    cua_spaces_app_core::parity::run(name, flow, &JsHost(host))
        .map(|t| t.to_string())
        .map_err(|e| JsError::new(&e))
}

/// The parity flows: `[{name, flow, golden}]` as JSON.
#[wasm_bindgen]
pub fn flows() -> String {
    serde_json::Value::Array(
        cua_spaces_app_core::parity::FLOWS
            .iter()
            .map(|(name, flow, golden)| {
                serde_json::json!({ "name": name, "flow": flow, "golden": golden })
            })
            .collect(),
    )
    .to_string()
}
