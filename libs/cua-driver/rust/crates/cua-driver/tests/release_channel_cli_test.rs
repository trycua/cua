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

#[cfg(target_os = "linux")]
mod pacman {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(owned: bool) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pacman");
        let script = if owned {
            "#!/bin/sh\n[ \"$1\" = -Qoq ] && [ \"$2\" = -- ] && [ \"$3\" = \"$PACMAN_TEST_EXECUTABLE\" ]\n"
        } else {
            "#!/bin/sh\nexit 1\n"
        };
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.path().join("release-channel"), "nightly\n").unwrap();
        std::fs::create_dir(root.path().join(".cua-driver")).unwrap();
        root
    }

    fn command(root: &std::path::Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_cua-driver"))
            .args(args)
            .env("PATH", root)
            .env(
                "PACMAN_TEST_EXECUTABLE",
                std::fs::canonicalize(env!("CARGO_BIN_EXE_cua-driver")).unwrap(),
            )
            .env("CUA_DRIVER_RS_HOME", root)
            .env("HOME", root)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .output()
            .unwrap()
    }

    fn assert_unavailable(state: &serde_json::Value) {
        assert_eq!(state["update_available"], false);
        assert_eq!(state["cache_hit"], false);
        for field in [
            "latest_version",
            "selected_channel",
            "install_command",
            "release_notes_url",
        ] {
            assert!(state[field].is_null(), "{state}");
        }
        assert!(state["error"]
            .as_str()
            .unwrap()
            .contains("sudo pacman -Syu"));
    }

    #[test]
    fn managed_cli_checks_and_apply_return_package_guidance() {
        let root = fixture(true);
        // A tempting cached upstream nightly must never be advertised.
        let cache = r#"{"latest_version":"999.0.0-nightly.20260907.1","channel":"nightly","last_checked_unix":9999999999}"#;
        let cache_path = root.path().join(".cua-driver/version_check.json");
        std::fs::write(&cache_path, cache).unwrap();
        for args in [
            vec!["check-update", "--json"],
            vec!["check-update", "--json", "--no-cache"],
            vec!["update", "--json"],
            vec!["update", "--apply", "--json"],
        ] {
            let output = command(root.path(), &args);
            assert!(!output.status.success());
            let state = serde_json::from_slice(&output.stdout).unwrap();
            assert_unavailable(&state);
        }
        let text = command(root.path(), &["update", "--apply"]);
        assert!(!text.status.success());
        assert!(String::from_utf8_lossy(&text.stdout).contains("sudo pacman -Syu"));
        assert_eq!(std::fs::read_to_string(cache_path).unwrap(), cache);
        for channel in ["stable", "nightly"] {
            let output = command(root.path(), &["channel", "set", channel]);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("sudo pacman -Syu"));
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("release-channel")).unwrap(),
            "nightly\n"
        );
    }

    #[test]
    fn pacman_presence_without_ownership_keeps_channel_switching() {
        let root = fixture(false);
        let output = command(root.path(), &["channel", "set", "stable", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let state: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(state["selected_channel"], "stable");
        std::fs::write(
            root.path().join(".cua-driver/version_check.json"),
            r#"{"latest_version":"999.0.0","channel":"stable","last_checked_unix":9999999999}"#,
        )
        .unwrap();
        let output = command(root.path(), &["check-update", "--json"]);
        assert!(output.status.success());
        let state: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(state["update_available"], true);
        assert_eq!(state["cache_hit"], true);
        assert_eq!(state["latest_version"], "999.0.0");
        assert!(state["error"].is_null());
    }

    #[test]
    fn managed_mcp_check_returns_same_unavailable_state() {
        let root = fixture(true);
        let executable = std::fs::canonicalize(env!("CARGO_BIN_EXE_cua-driver")).unwrap();
        let mut driver = cua_driver_testkit::McpDriver::spawn_with_env(&[
            ("PATH", root.path().to_str().unwrap()),
            ("PACMAN_TEST_EXECUTABLE", executable.to_str().unwrap()),
            ("CUA_DRIVER_RS_HOME", root.path().to_str().unwrap()),
            ("HOME", root.path().to_str().unwrap()),
        ])
        .expect("start test daemon and MCP proxy");
        let result = driver.call("check_for_update", serde_json::json!({}));
        assert!(
            result.text().contains("sudo pacman -Syu"),
            "{:?}",
            result.raw
        );
        assert_unavailable(&result.raw["result"]["structuredContent"]);
        assert!(!root.path().join(".cua-driver/version_check.json").exists());
    }
}
