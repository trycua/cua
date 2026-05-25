//! Parity check for `cua-driver dump-docs`.

use std::process::Command;

#[cfg(target_os = "windows")]
fn main() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/cua-driver.exe");

    // 1. Default: --type=all → {cli, mcp}.
    let out = Command::new(&exe).arg("dump-docs").output().expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(v.get("cli").is_some(), "--type=all should have 'cli' key");
    assert!(v.get("mcp").is_some(), "--type=all should have 'mcp' key");
    assert!(v.pointer("/mcp/version").is_some(), "mcp.version missing");
    let tools = v.pointer("/mcp/tools").and_then(|t| t.as_array()).expect("mcp.tools array");
    assert!(!tools.is_empty(), "mcp.tools should not be empty");
    println!("dump-docs (default) OK — {} tools", tools.len());

    // 2. --type=mcp → just {version, tools: [...]}.
    let out2 = Command::new(&exe).args(["dump-docs", "--type", "mcp"]).output().expect("run");
    let v2: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out2.stdout)).expect("valid JSON");
    assert!(v2.get("cli").is_none(), "--type=mcp should NOT have 'cli' key");
    assert!(v2.get("version").is_some(), "--type=mcp top-level should have version");
    assert!(v2.get("tools").is_some(), "--type=mcp top-level should have tools");
    println!("dump-docs --type=mcp OK");

    // 3. --type=cli → CLI section only (stub on Rust).
    let out3 = Command::new(&exe).args(["dump-docs", "--type", "cli"]).output().expect("run");
    let v3: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out3.stdout)).expect("valid JSON");
    assert!(v3.get("mcp").is_none(), "--type=cli should NOT have 'mcp' key");
    assert!(v3.get("version").is_some(), "--type=cli should have version");
    println!("dump-docs --type=cli OK");

    // 4. --pretty: should have multi-line output.
    let out4 = Command::new(&exe).args(["dump-docs", "--pretty"]).output().expect("run");
    let stdout4 = String::from_utf8_lossy(&out4.stdout);
    assert!(stdout4.lines().count() > 10, "--pretty should produce multi-line output");
    println!("dump-docs --pretty OK ({} lines)", stdout4.lines().count());

    println!("\n✅ PASS: dump-docs --type cli/mcp/all + --pretty all work");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
