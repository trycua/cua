//! Parity check for the `list-tools` CLI subcommand.
//!
//! Runs the binary directly and asserts:
//! - Output is one tool per line as `"name: <first sentence>"` or just `"name"`.
//! - Lines are sorted alphabetically by tool name (matches Swift's sort).
//! - All expected core tools are present.

use std::process::Command;

#[cfg(target_os = "windows")]
fn main() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/cua-driver.exe");
    let out = Command::new(&exe)
        .arg("list-tools")
        .output()
        .expect("run cua-driver list-tools");
    assert!(out.status.success(), "list-tools failed: {:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty(), "no output from list-tools");

    // Names are sorted ascending.
    let names: Vec<&str> = lines.iter().map(|l| l.split(':').next().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "tools are not sorted alphabetically (Swift parity)");

    // All core tools are present.
    let expected = [
        "click", "double_click", "right_click", "type_text", "press_key", "hotkey",
        "scroll", "screenshot", "list_apps", "list_windows", "get_cursor_position",
        "get_screen_size", "launch_app", "move_cursor", "set_agent_cursor_enabled",
        "set_agent_cursor_motion", "get_agent_cursor_state", "set_value",
        "get_config", "set_config", "check_permissions", "get_recording_state",
        "set_recording",
    ];
    for tool in expected {
        assert!(names.iter().any(|n| *n == tool), "missing tool `{tool}` in list-tools output");
    }

    // Each non-empty line should have `name: <summary>` shape OR just `name`.
    for line in &lines {
        if line.contains(':') {
            let after = line.split_once(':').unwrap().1;
            assert!(after.trim_start().chars().next().is_some(),
                "empty description after colon in line: {line:?}");
        }
    }

    println!("\n✅ PASS: list-tools is alphabetically sorted, all core tools present, format matches Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
