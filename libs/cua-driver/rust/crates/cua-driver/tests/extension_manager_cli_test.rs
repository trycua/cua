use flate2::{write::GzEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1")
        .output()
        .expect("run cua-driver extension command")
}

fn current_target() -> String {
    let suffix = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(all(target_os = "windows", target_env = "msvc")) {
        "pc-windows-msvc"
    } else if cfg!(all(target_os = "linux", target_env = "musl")) {
        "unknown-linux-musl"
    } else if cfg!(all(target_os = "linux", target_env = "gnu")) {
        "unknown-linux-gnu"
    } else {
        panic!("unsupported extension-manager test target")
    };
    format!("{}-{suffix}", std::env::consts::ARCH)
}

fn fixture_archive(directory: &Path) -> PathBuf {
    let payload = b"fixture-worker";
    let digest = format!("{:x}", Sha256::digest(payload));
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "id": "local-prototype",
        "version": "1.2.3",
        "driver_version": format!("={}", env!("CARGO_PKG_VERSION")),
        "protocol_version": 1,
        "target": current_target(),
        "entrypoint": "bin/local-extension",
        "files": [{
            "path": "bin/local-extension",
            "sha256": digest,
            "executable": true
        }]
    }))
    .unwrap();
    let path = directory.join("local-extension.tar.gz");
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, "extension.json", &manifest);
    append(&mut builder, "bin/local-extension", payload);
    builder.finish().unwrap();
    path
}

fn append(builder: &mut tar::Builder<GzEncoder<fs::File>>, path: &str, bytes: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(bytes))
        .unwrap();
}

#[test]
#[cfg(not(windows))]
fn status_and_path_report_exact_active_version() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");

    let list = run(&home, &["extension", "list", "--json"]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_json: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list_json[0]["id"], "local-prototype");
    assert_eq!(list_json[0]["installed"], false);
    let list_detail = list_json[0]["detail"].as_str().unwrap();
    assert!(list_detail.contains("unsigned, untrusted"));
    assert!(list_detail.contains("archive-provided hashes do not authenticate"));
    let plain_list = run(&home, &["extension", "list"]);
    let plain_list = String::from_utf8_lossy(&plain_list.stdout);
    assert!(plain_list.contains("unsigned, untrusted"));
    assert!(plain_list.contains("archive-provided hashes do not authenticate"));

    let absent = run(&home, &["extension", "status", "local-prototype", "--json"]);
    assert!(
        absent.status.success(),
        "{}",
        String::from_utf8_lossy(&absent.stderr)
    );
    let absent_json: Value = serde_json::from_slice(&absent.stdout).unwrap();
    assert_eq!(absent_json["installed"], false);
    assert_eq!(absent_json["healthy"], true);

    let archive = fixture_archive(temp.path());
    let install = run(
        &home,
        &[
            "extension",
            "install",
            "local-prototype",
            "--archive",
            archive.to_str().unwrap(),
        ],
    );
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    let install_stdout = String::from_utf8_lossy(&install.stdout);
    assert!(install_stdout.contains("unsigned, untrusted local code"));
    assert!(install_stdout.contains("self-asserted"));
    assert!(install_stdout.contains("do not establish provenance"));

    let status = run(&home, &["extension", "status", "local-prototype", "--json"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active_version"], "1.2.3");
    assert_eq!(status_json["healthy"], true);
    let status_detail = status_json["detail"].as_str().unwrap();
    assert!(status_detail.contains("unsigned, untrusted"));
    assert!(status_detail.contains("archive-provided hashes do not authenticate"));
    let plain_status = run(&home, &["extension", "status", "local-prototype"]);
    let plain_status = String::from_utf8_lossy(&plain_status.stdout);
    assert!(plain_status.contains("unsigned, untrusted"));
    assert!(plain_status.contains("archive-provided hashes do not authenticate"));

    let path = run(&home, &["extension", "path", "local-prototype"]);
    assert!(
        path.status.success(),
        "{}",
        String::from_utf8_lossy(&path.stderr)
    );
    let actual = String::from_utf8(path.stdout).unwrap();
    assert_eq!(
        Path::new(actual.trim()),
        home.join("extensions/local-prototype/versions/1.2.3")
    );
}

#[test]
fn help_and_manifests_state_the_untrusted_prototype_contract() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");

    let help = run(&home, &["--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("UNSIGNED, UNTRUSTED LOCAL CODE PROTOTYPE"));
    assert!(help.contains("self-asserted"));
    assert!(help.contains("unsupported on Windows"));
    assert!(!help.contains("extension uninstall"));

    let manifest = run(&home, &["manifest"]);
    let manifest = String::from_utf8_lossy(&manifest.stdout).to_lowercase();
    assert!(manifest.contains("unsigned, untrusted local-code"));
    assert!(manifest.contains("self-asserted"));
    assert!(manifest.contains("uninstall is intentionally unavailable"));

    let docs = run(&home, &["dump-docs", "--type", "commands"]);
    let docs = String::from_utf8_lossy(&docs.stdout).to_lowercase();
    assert!(docs.contains("unsigned, untrusted local extension prototype"));
    assert!(docs.contains("self-asserted"));
    assert!(docs.contains("uninstall is intentionally absent"));
}

#[test]
fn cli_rejects_ambiguous_extension_arguments() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    for args in [
        vec!["extension", "list", "extra"],
        vec!["extension", "status", "--json", "--json"],
        vec!["extension", "install", "local-prototype", "--archive"],
        vec!["extension", "uninstall", "local-prototype"],
    ] {
        let output = run(&home, &args);
        assert!(!output.status.success(), "accepted {args:?}");
    }
}

#[cfg(windows)]
#[test]
fn windows_refuses_extension_mutation() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let archive = fixture_archive(temp.path());
    let output = run(
        &home,
        &[
            "extension",
            "install",
            "local-prototype",
            "--archive",
            archive.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported on Windows"));
}
