// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's own views (start a task, the outbox, approvals, Queue and Steer, dictation, diffs, push) replayed
//! through the phone's entry point, `cua_spaces_mobile_core::call` (the function the React Native module calls), against
//! the golden the core's own test (`cua-spaces-app-core/tests/phone_golden.rs`) pins. Same flows, same answers.

use serde_json::{Value, json};

fn path(name: &str) -> String {
    format!(
        "{}/../../../libs/cua/crates/cua-spaces-app-core/parity/phone/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn call(method: &str, args: Value) -> Result<Value, String> {
    let out = cua_spaces_mobile_core::call(method, &args.to_string())?;
    serde_json::from_str(&out).map_err(|e| e.to_string())
}

fn replay() -> Value {
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
fn the_phone_flows_match_the_core_golden_through_the_mobile_entry_point() {
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(path("golden.json")).unwrap()).unwrap();
    assert_eq!(golden, replay());
}
