// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The core's answers the app's tests read.
//!
//! `fixtures/calls.json` lists the `{method, args}` the app makes (and the
//! screenshots show); `fixtures/results.json` holds what the core answers.
//! The Jest tests replay `results.json` (React Native tests cannot load the
//! Rust library), so this test keeps them honest: it fails when the core's
//! answer drifts from the file. `UPDATE_FIXTURES=1 cargo test --test
//! fixtures` rewrites it; review the diff, a change is a change of the words
//! the phone shows.

use serde_json::{Value, json};

fn path(name: &str) -> String {
    format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn compute() -> Value {
    let calls: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(path("calls.json")).unwrap()).unwrap();
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let method = c["method"].as_str().unwrap();
                let args = c["args"].to_string();
                match cua_spaces_mobile_core::call(method, &args) {
                    Ok(result) => json!({ "method": method, "args": c["args"], "result": serde_json::from_str::<Value>(&result).unwrap() }),
                    // A method this core does not have (yet): the phone's `tryCall` gets null for it, so the
                    // fixture records the refusal, and changes when the core gains the method.
                    Err(e) if e.contains("unknown method") => json!({ "method": method, "args": c["args"], "error": e }),
                    Err(e) => panic!("{method}: {e}"),
                }
            })
            .collect(),
    )
}

#[test]
fn the_fixtures_are_what_the_core_answers() {
    let now = compute();
    if std::env::var("UPDATE_FIXTURES").is_ok_and(|v| v == "1") {
        let mut text = serde_json::to_string_pretty(&now).unwrap();
        text.push('\n');
        std::fs::write(path("results.json"), text).unwrap();
        return;
    }
    let stored: Value = serde_json::from_str(
        &std::fs::read_to_string(path("results.json"))
            .expect("run UPDATE_FIXTURES=1 cargo test --test fixtures"),
    )
    .unwrap();
    assert_eq!(
        stored, now,
        "the core's answers drifted from fixtures/results.json"
    );
}
