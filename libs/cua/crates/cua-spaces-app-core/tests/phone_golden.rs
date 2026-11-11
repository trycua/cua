// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's views and state machines, replayed from `parity/phone/flows.json`
//! and compared with `parity/phone/golden.json`. The mobile crate replays the
//! same file through the phone's entry point, so the React Native app, the
//! dispatcher and these goldens agree. `UPDATE_PARITY=1 cargo test -p
//! cua-spaces-app-core --test phone_golden` rewrites the golden; review the diff,
//! a change is a change of the words a phone shows.

use cua_spaces_app_core::dispatch::call_value;
use serde_json::{Value, json};

fn path(name: &str) -> String {
    format!("{}/parity/phone/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Replays every flow and call through `call` (the dispatcher, or a host that wraps it).
pub fn replay(call: &dyn Fn(&str, Value) -> Result<Value, String>) -> Value {
    let flows: Value =
        serde_json::from_str(&std::fs::read_to_string(path("flows.json")).unwrap()).unwrap();
    let mut out = vec![];
    for f in flows["flows"].as_array().unwrap() {
        let mut state = call("outbox.initial", json!({})).unwrap();
        let mut frames = vec![];
        for s in f["steps"].as_array().unwrap() {
            let r = call(
                "outbox.reduce",
                json!({ "state": state, "action": s["action"] }),
            )
            .unwrap();
            state = r["state"].clone();
            let view = call("outbox.view", json!({ "state": state })).unwrap();
            let rows: Vec<String> = view["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| {
                    format!(
                        "{}: {} ({})",
                        x["id"].as_str().unwrap(),
                        x["label"].as_str().unwrap(),
                        x["state"].as_str().unwrap()
                    )
                })
                .collect();
            frames.push(json!({ "step": s["step"], "effects": r["effects"], "rows": rows, "banner": view["banner"] }));
        }
        out.push(json!({ "flow": f["name"], "frames": frames }));
    }
    let mut calls = vec![];
    for c in flows["calls"].as_array().unwrap() {
        let r = call(c["method"].as_str().unwrap(), c["args"].clone())
            .unwrap_or_else(|e| json!({ "error": e }));
        calls.push(json!({ "name": c["name"], "method": c["method"], "result": r }));
    }
    json!({ "flows": out, "calls": calls })
}

#[test]
fn the_phone_flows_match_their_golden() {
    let now = replay(&|m, a| call_value(m, a).map_err(|e| e.to_string()));
    if std::env::var("UPDATE_PARITY").is_ok_and(|v| v == "1") {
        let mut text = serde_json::to_string_pretty(&now).unwrap();
        text.push('\n');
        std::fs::write(path("golden.json"), text).unwrap();
        return;
    }
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(path("golden.json"))
            .expect("run UPDATE_PARITY=1 cargo test -p cua-spaces-app-core --test phone_golden"),
    )
    .unwrap();
    assert_eq!(
        golden, now,
        "a phone view drifted from parity/phone/golden.json"
    );
}
