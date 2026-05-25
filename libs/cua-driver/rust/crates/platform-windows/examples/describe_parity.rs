//! Parity check for the `describe` CLI subcommand.

use std::process::Command;

#[cfg(target_os = "windows")]
fn main() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/cua-driver.exe");

    // 1. Known tool — describe `click`.
    let out = Command::new(&exe)
        .args(["describe", "click"])
        .output()
        .expect("run cua-driver describe click");
    assert!(out.status.success(), "describe click failed: {:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("name: click\n"),
        "describe should start with 'name: click\\n', got: {:?}", &stdout[..stdout.len().min(40)]);
    assert!(stdout.contains("\ndescription:\n"), "describe missing description: block");
    assert!(stdout.contains("\ninput_schema:\n"), "describe missing input_schema: block");
    // The JSON schema after should be pretty-printed (multi-line).
    let schema_section = stdout.split("\ninput_schema:\n").nth(1).unwrap_or("");
    assert!(schema_section.contains("\n  "), "input_schema should be pretty-printed JSON");
    println!("describe known-tool OK");

    // 2. Unknown tool — exit code 64, stderr lists tools alphabetically.
    let out2 = Command::new(&exe)
        .args(["describe", "this_tool_does_not_exist"])
        .output()
        .expect("run cua-driver describe missing");
    assert_eq!(out2.status.code(), Some(64), "exit code should be 64 (EX_USAGE), got: {:?}", out2.status.code());
    let stderr = String::from_utf8_lossy(&out2.stderr);
    assert!(stderr.contains("Unknown tool: this_tool_does_not_exist"),
        "stderr missing 'Unknown tool:' line: {stderr:?}");
    assert!(stderr.contains("Available tools:"),
        "stderr missing 'Available tools:' header");
    // Parse the listed tools (each on its own indented line) and verify sort.
    let listed: Vec<&str> = stderr.lines()
        .filter(|l| l.starts_with("  ") && !l.trim().is_empty())
        .map(|l| l.trim())
        .collect();
    let mut sorted = listed.clone(); sorted.sort();
    assert_eq!(listed, sorted, "available tools should be sorted alphabetically");
    println!("describe unknown-tool OK ({} tools listed)", listed.len());

    println!("\n✅ PASS: describe known/unknown paths + Swift sort verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
