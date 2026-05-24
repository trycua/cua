//! Parity check for `cua-driver status` + `cua-driver stop` CLI subcommands.

use std::process::Command;

#[cfg(target_os = "windows")]
fn main() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/cua-driver.exe");

    // Assume a daemon is running for these tests.

    // 1. status — should match Swift's 3-line output format.
    let out = Command::new(&exe).arg("status").output().expect("status");
    assert!(out.status.success(), "status should succeed when daemon is running");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.len() >= 2, "status should print at least 2 lines, got: {stdout:?}");
    assert_eq!(lines[0], "cua-driver daemon is running",
        "first line wrong: {:?}", lines[0]);
    assert!(lines[1].trim_start().starts_with("socket:"),
        "second line should start with 'socket:': {:?}", lines[1]);
    if lines.len() >= 3 {
        assert!(lines[2].trim_start().starts_with("pid:"),
            "third line should start with 'pid:': {:?}", lines[2]);
    }
    println!("status OK");

    // 2. stop — should produce silent stdout on success (Swift parity).
    let out2 = Command::new(&exe).arg("stop").output().expect("stop");
    assert!(out2.status.success(), "stop should succeed when daemon is running");
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.trim().is_empty(),
        "Swift's stop is silent on success; got stdout: {stdout2:?}");
    println!("stop OK (silent stdout, matches Swift)");

    // 3. status after stop — exit code 1, stderr "not running".
    let out3 = Command::new(&exe).arg("status").output().expect("status2");
    assert_eq!(out3.status.code(), Some(1), "status after stop should exit 1");
    let stderr3 = String::from_utf8_lossy(&out3.stderr);
    assert!(stderr3.contains("cua-driver daemon is not running"),
        "stderr should say 'not running': {stderr3:?}");
    println!("status-not-running OK");

    // 4. stop on dead daemon — exit 1, stderr "not running".
    let out4 = Command::new(&exe).arg("stop").output().expect("stop2");
    assert_eq!(out4.status.code(), Some(1), "stop on dead daemon should exit 1");
    let stderr4 = String::from_utf8_lossy(&out4.stderr);
    assert!(stderr4.contains("cua-driver daemon is not running"),
        "stderr should say 'not running': {stderr4:?}");
    println!("stop-not-running OK");

    println!("\n✅ PASS: status + stop output + exit codes match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
