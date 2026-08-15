//! Focused coverage for the read-only skill-path CLI shortcuts.

use std::path::Path;
use std::process::{Command, Output};

fn run_driver(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .output()
        .expect("run cua-driver")
}

#[test]
fn singular_skill_prints_the_same_path_as_plural_skills_path() {
    let home = tempfile::tempdir().expect("create isolated Cua Driver home");
    let expected = home.path().join("skills").join("cua-driver");

    let singular = run_driver(home.path(), &["skill"]);
    assert!(
        singular.status.success(),
        "singular skill command failed: {}",
        String::from_utf8_lossy(&singular.stderr)
    );
    assert_eq!(
        String::from_utf8(singular.stdout).expect("UTF-8 singular path"),
        format!("{}\n", expected.display())
    );

    let plural = run_driver(home.path(), &["skills", "path"]);
    assert!(
        plural.status.success(),
        "plural skills path command failed: {}",
        String::from_utf8_lossy(&plural.stderr)
    );
    assert_eq!(
        String::from_utf8(plural.stdout).expect("UTF-8 plural path"),
        format!("{}\n", expected.display())
    );
}

#[test]
fn bare_plural_skills_keeps_its_status_behavior() {
    let home = tempfile::tempdir().expect("create isolated Cua Driver home");
    let output = run_driver(home.path(), &["skills"]);

    assert!(
        output.status.success(),
        "bare plural skills command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 skills status");
    assert!(
        stdout.contains("Local skill pack: not installed"),
        "unexpected status output: {stdout}"
    );
    assert!(
        stdout.contains("Agent links:"),
        "unexpected status output: {stdout}"
    );
}
