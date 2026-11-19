//! The runner's own tests (`runner/test/*.test.mjs`), under `cargo test`:
//! the guest-side runner is JavaScript, and what it decides (a rejected API
//! key ends a turn in seconds, the guest MCP session ends with the run) is
//! checked against fake agents and a fake provider. Skipped, saying so, where
//! `node` is not installed.

use std::path::Path;
use std::process::Command;

#[test]
fn the_runner_node_tests_pass() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("runner/test");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".test.mjs"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no runner tests in {}", dir.display());
    let out = match Command::new("node").arg("--test").args(&files).output() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("skipped: could not run node ({e})");
            return;
        }
    };
    assert!(
        out.status.success(),
        "node --test failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
