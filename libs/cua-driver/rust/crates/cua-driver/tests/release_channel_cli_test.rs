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
    use cua_driver_testkit::Driver;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pacman");
        // A successful imposter on PATH must never establish package ownership.
        std::fs::write(
            &path,
            "#!/bin/sh\n: > \"$HOME/fake-pacman-called\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.path().join("release-channel"), "nightly\n").unwrap();
        std::fs::create_dir(root.path().join(".cua-driver")).unwrap();
        root
    }

    fn command(
        executable: &std::path::Path,
        root: &std::path::Path,
        args: &[&str],
    ) -> std::process::Output {
        Command::new(executable)
            .args(args)
            .env("PATH", root)
            .env("CUA_DRIVER_RS_HOME", root)
            .env("HOME", root)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .output()
            .unwrap()
    }

    fn managed_executable() -> std::path::PathBuf {
        let executable = std::path::PathBuf::from(
            std::env::var_os("PACMAN_TEST_MANAGED_EXECUTABLE")
                .expect("native test requires PACMAN_TEST_MANAGED_EXECUTABLE"),
        );
        let owned = Command::new("/usr/bin/pacman")
            .args(["-Qoq", "--"])
            .arg(std::fs::canonicalize(&executable).expect("resolve native candidate"))
            .output()
            .expect("native test requires real /usr/bin/pacman");
        assert!(owned.status.success(), "candidate must be pacman-owned");
        assert!(!owned.stdout.is_empty(), "pacman must identify its owner");
        executable
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
    #[ignore = "requires a real pacman-owned PACMAN_TEST_MANAGED_EXECUTABLE in a disposable Linux guest"]
    fn managed_cli_checks_and_apply_return_package_guidance() {
        let executable = managed_executable();
        let root = fixture();
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
            let output = command(&executable, root.path(), &args);
            assert!(!output.status.success());
            let state = serde_json::from_slice(&output.stdout).unwrap();
            assert_unavailable(&state);
        }
        let text = command(&executable, root.path(), &["update", "--apply"]);
        assert!(!text.status.success());
        assert!(String::from_utf8_lossy(&text.stdout).contains("sudo pacman -Syu"));
        assert_eq!(std::fs::read_to_string(&cache_path).unwrap(), cache);
        let preference = root.path().join("release-channel");
        for saved in [None, Some("nightly\n"), Some("broken\n")] {
            match saved {
                Some(value) => std::fs::write(&preference, value).unwrap(),
                None => std::fs::remove_file(&preference).unwrap(),
            }
            for args in [
                vec!["channel", "status"],
                vec!["channel", "set", "stable"],
                vec!["channel", "set", "nightly"],
            ] {
                let text = command(&executable, root.path(), &args);
                assert!(!text.status.success());
                assert!(text.stdout.is_empty());
                assert!(String::from_utf8_lossy(&text.stderr).contains("sudo pacman -Syu"));
                let mut json_args = args;
                json_args.push("--json");
                let json = command(&executable, root.path(), &json_args);
                assert!(!json.status.success());
                let state: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
                assert_eq!(
                    state.get("selected_channel"),
                    Some(&serde_json::Value::Null)
                );
                for field in ["current_version", "current_channel"] {
                    assert!(!state[field].as_str().unwrap().is_empty(), "{state}");
                }
                assert!(state["error"]
                    .as_str()
                    .unwrap()
                    .contains("sudo pacman -Syu"));
                assert_eq!(std::fs::read_to_string(&preference).ok().as_deref(), saved);
                assert_eq!(std::fs::read_to_string(&cache_path).unwrap(), cache);
            }
        }
        std::fs::remove_file(&cache_path).unwrap();
        let uncached = command(&executable, root.path(), &["check-update", "--json"]);
        assert!(!uncached.status.success());
        assert_unavailable(&serde_json::from_slice(&uncached.stdout).unwrap());
        assert!(
            !cache_path.exists(),
            "managed checks must not create an upstream cache"
        );
        assert!(!root.path().join("fake-pacman-called").exists());
    }

    #[test]
    fn fake_path_pacman_cannot_disable_unmanaged_channel_switching() {
        let executable = std::env::var_os("PACMAN_TEST_UNMANAGED_EXECUTABLE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_cua-driver").into());
        let root = fixture();
        let output = command(
            &executable,
            root.path(),
            &["channel", "set", "stable", "--json"],
        );
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
        let output = command(&executable, root.path(), &["check-update", "--json"]);
        assert!(output.status.success());
        let state: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(state["update_available"], true);
        assert_eq!(state["cache_hit"], true);
        assert_eq!(state["latest_version"], "999.0.0");
        assert!(state["error"].is_null());
        assert!(!root.path().join("fake-pacman-called").exists());
    }

    #[test]
    #[ignore = "requires a real pacman-owned candidate and CUA_TEST_DRIVER_BIN in a disposable Linux guest"]
    fn managed_mcp_check_returns_same_unavailable_state() {
        let executable = managed_executable();
        assert_eq!(
            std::path::PathBuf::from(
                std::env::var_os("CUA_TEST_DRIVER_BIN").expect("set testkit binary override")
            ),
            executable,
            "testkit must start the exact packaged candidate (including symlink entry points)"
        );
        let root = fixture();
        let mut driver = cua_driver_testkit::McpDriver::spawn_with_env(&[
            ("PATH", root.path().to_str().unwrap()),
            ("CUA_DRIVER_RS_HOME", root.path().to_str().unwrap()),
            ("HOME", root.path().to_str().unwrap()),
        ])
        .expect("start test daemon and MCP proxy");
        let result = driver.call("check_for_update", serde_json::json!({}));
        assert!(result.text().starts_with("Update check unavailable:"));
        assert!(
            result.text().contains("sudo pacman -Syu"),
            "{:?}",
            result.raw
        );
        assert_unavailable(&result.raw["result"]["structuredContent"]);
        assert!(!root.path().join(".cua-driver/version_check.json").exists());
        assert!(!root.path().join("fake-pacman-called").exists());
    }
}
