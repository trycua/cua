use std::process::Command;

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .output()
        .expect("run cua-driver")
}

#[test]
fn channel_cli_persists_and_reports_the_selected_channel() {
    let home = tempfile::tempdir().expect("temp home");

    let initial = run(home.path(), &["channel", "status", "--json"]);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial: serde_json::Value = serde_json::from_slice(&initial.stdout).expect("initial json");
    assert_eq!(initial["selected_channel"], "stable");

    let changed = run(home.path(), &["channel", "set", "nightly", "--json"]);
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("release-channel")).expect("saved preference"),
        "nightly\n"
    );

    let status = run(home.path(), &["channel", "status", "--json"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status json");
    assert_eq!(status["selected_channel"], "nightly");
}

#[test]
fn channel_cli_fails_closed_on_invalid_saved_state() {
    let home = tempfile::tempdir().expect("temp home");
    std::fs::write(home.path().join("release-channel"), "broken\n").expect("invalid state");

    let output = run(home.path(), &["channel", "status", "--json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected `stable` or `nightly`"),
        "{stderr}"
    );
    assert!(stderr.contains("channel set stable"), "{stderr}");
}
