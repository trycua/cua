// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `openkoalabot-example-scenario --spec <scenario.json> --lane fixture|docker|cloud --out <result.json>`
//!
//! Runs the shared openkoalabots scenario through `openkoalabot-example-core`, headless.
//! Reads `OPENKOALABOTS_SCENARIO_URL`, `OPENKOALABOTS_SCENARIO_TOKEN`,
//! `OPENKOALABOTS_SCENARIO_IMPORT_ROOT` and (cloud) `OPENKOALABOTS_CLOUD_IMAGE`.
//! Exits 0 when every step passed or skipped.

use openkoalabot_example_core::scenario::{Lane, run};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage: openkoalabot-example-scenario --spec <scenario.json> --lane fixture|docker|cloud --out <result.json>"
    );
    std::process::exit(2)
}

fn main() {
    let mut spec = None;
    let mut lane = None;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--spec" => spec = args.next().map(PathBuf::from),
            "--lane" => lane = args.next(),
            "--out" => out = args.next().map(PathBuf::from),
            _ => usage(),
        }
    }
    let (Some(spec), Some(lane)) = (spec, lane) else {
        usage()
    };
    if !["fixture", "docker", "cloud"].contains(&lane.as_str()) {
        usage();
    }
    let parsed: serde_json::Value = match std::fs::read_to_string(&spec)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {e}", spec.display());
            std::process::exit(2)
        }
    };
    let spec_dir = spec.parent().map(PathBuf::from).unwrap_or_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = rt.block_on(run(&parsed, &spec_dir, Lane::from_env(&lane)));
    let (json, ok) = match result {
        Ok(r) => (serde_json::to_value(&r).unwrap(), r.ok),
        Err(e) => (
            serde_json::json!({"impl": "tauri", "lane": lane, "ok": false, "totalMs": 0, "steps": [], "error": e.to_string()}),
            false,
        ),
    };
    let text = serde_json::to_string_pretty(&json).unwrap();
    match out {
        Some(p) => std::fs::write(&p, &text).expect("write --out"),
        None => println!("{text}"),
    }
    std::process::exit(if ok { 0 } else { 1 });
}
