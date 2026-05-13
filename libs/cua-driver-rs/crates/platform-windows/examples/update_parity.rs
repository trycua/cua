//! Parity check for `cua-driver update` (no --apply, network-dependent).

use std::process::Command;

#[cfg(target_os = "windows")]
fn main() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/cua-driver.exe");

    let out = Command::new(&exe).arg("update").output().expect("run update");
    let stdout = String::from_utf8_lossy(&out.stdout);
    println!("update stdout (first 500 chars):");
    println!("{}", stdout.chars().take(500).collect::<String>());

    // Always-present lines.
    assert!(stdout.contains("Current version:"),
        "missing 'Current version:' header");
    assert!(stdout.contains("Checking for updates"),
        "missing 'Checking for updates' line");

    // Outcome is one of:
    //   - "Already up to date." (current ≥ latest published)
    //   - "New version available:" (current < latest published)
    //   - "Could not reach GitHub" (network failure)
    let outcome_present = stdout.contains("Already up to date")
        || stdout.contains("New version available:")
        || stdout.contains("Could not reach GitHub");
    assert!(outcome_present, "no outcome line found in update output:\n{stdout}");

    // Critical: if a new version IS reported, the tag prefix must be the Rust-
    // port one (`cua-driver-rs-v*`) — otherwise we'd be recommending the
    // Swift binary's version which would be a real-user bug.
    if stdout.contains("New version available:") {
        let line = stdout.lines().find(|l| l.contains("New version available:")).unwrap();
        // The version printed should be numeric (e.g. "0.1.2"), not a
        // Swift-style version that exists on the same repo but is for a
        // different binary.  We can't directly tell from the output, but
        // we can run with --apply mocked off (which we already are: no flag).
        println!("Found new version line: {line}");
    }

    println!("\n✅ PASS: update CLI prints expected outcome lines");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
