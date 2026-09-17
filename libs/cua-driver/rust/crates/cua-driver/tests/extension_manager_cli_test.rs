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

fn fixture_archive(directory: &Path, version: &str) -> PathBuf {
    let payload: &[u8] = if cfg!(windows) {
        b"@exit /b 0\r\n"
    } else {
        b"#!/bin/sh\nexit 0\n"
    };
    let entrypoint = if cfg!(windows) {
        "bin/cua-perception.cmd"
    } else {
        "bin/cua-perception"
    };
    let digest = format!("{:x}", Sha256::digest(payload));
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "id": "cua-perception",
        "version": version,
        "driver_version": format!("={}", env!("CARGO_PKG_VERSION")),
        "protocol_version": 1,
        "target": current_target(),
        "entrypoint": entrypoint,
        "files": [{"path": entrypoint, "sha256": digest, "executable": true}],
        "models": [],
        "components": [{
            "name": "cua-perception", "version": version, "license": "Apache-2.0",
            "notice": "Copyright Cua contributors", "source_uri": "https://github.com/trycua/cua",
            "source_revision": "integration-fixture"
        }],
        "license": "Apache-2.0",
        "source": "https://github.com/trycua/cua",
        "corresponding_source_uri": "https://github.com/trycua/cua",
        "corresponding_source_revision": "integration-fixture",
        "provenance": "integration test fixture",
        "health_args": ["health"],
        "self_test_args": ["self-test"]
    }))
    .unwrap();
    let path = directory.join(format!("cua-perception-{version}.tar.gz"));
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, "extension.json", &manifest);
    append(&mut builder, entrypoint, payload);
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
fn developer_lifecycle_is_explicit_previewed_and_removable() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let archive = fixture_archive(temp.path(), "1.2.3");

    let list = run(&home, &["extension", "list", "--json"]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_json: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list_json[0]["id"], "cua-perception");
    assert_eq!(list_json[0]["installed"], false);

    let rejected = run(
        &home,
        &[
            "extension",
            "install",
            "cua-perception",
            "--archive",
            archive.to_str().unwrap(),
        ],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--allow-unsigned-local"));

    let inspect = run(
        &home,
        &[
            "extension",
            "inspect",
            "cua-perception",
            "--archive",
            archive.to_str().unwrap(),
            "--allow-unsigned-local",
            "--json",
        ],
    );
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let preview: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(preview["trust"], "developer-unsigned-local");
    assert_eq!(preview["license"], "Apache-2.0");
    assert_eq!(preview["mutation_performed"], false);
    assert!(!home.join("extensions/cua-perception").exists());

    let install = run(
        &home,
        &[
            "extension",
            "install",
            "cua-perception",
            "--archive",
            archive.to_str().unwrap(),
            "--allow-unsigned-local",
        ],
    );
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    assert!(String::from_utf8_lossy(&install.stdout).contains("developer-unsigned-local"));

    let status = run(
        &home,
        &[
            "extension",
            "status",
            "cua-perception",
            "--self-test",
            "--json",
        ],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active_version"], "1.2.3");
    assert_eq!(status_json["trust"], "developer-unsigned-local");
    assert_eq!(status_json["healthy"], true);

    let update_archive = fixture_archive(temp.path(), "1.2.4");
    let update = run(
        &home,
        &[
            "extension",
            "update",
            "cua-perception",
            "--archive",
            update_archive.to_str().unwrap(),
            "--allow-unsigned-local",
        ],
    );
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let updated_status = run(&home, &["extension", "status", "cua-perception", "--json"]);
    let updated_json: Value = serde_json::from_slice(&updated_status.stdout).unwrap();
    assert_eq!(updated_json["active_version"], "1.2.4");

    let remove = run(&home, &["extension", "remove", "cua-perception"]);
    assert!(
        remove.status.success(),
        "{}",
        String::from_utf8_lossy(&remove.stderr)
    );
    assert!(!home.join("extensions/cua-perception").exists());
}

#[test]
fn signed_catalog_rejects_forgery_before_state_creation() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let archive = fixture_archive(temp.path(), "1.2.3");
    let archive_bytes = fs::read(&archive).unwrap();
    let catalog = json!({
        "payload": {
            "schema_version": 1, "catalog_version": 1, "expires_unix": 4102444800_u64,
            "publisher_id": "cua", "publisher_name": "Cua",
            "key_id": "cua-extension-ed25519-2026-01", "extension_id": "cua-perception",
            "version": "1.2.3", "target": current_target(),
            "archive": archive.file_name().unwrap().to_str().unwrap(),
            "archive_size": archive_bytes.len(),
            "archive_sha256": format!("{:x}", Sha256::digest(&archive_bytes)),
            "manifest_sha256": "00".repeat(32), "license": "Apache-2.0",
            "source": "https://github.com/trycua/cua",
            "corresponding_source_uri": "https://github.com/trycua/cua",
            "corresponding_source_revision": "integration-fixture",
            "provenance": "forged test catalog"
        },
        "signature_algorithm": "ed25519",
        "signature": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="
    });
    let catalog_path = temp.path().join("catalog.json");
    fs::write(&catalog_path, serde_json::to_vec_pretty(&catalog).unwrap()).unwrap();
    let output = run(
        &home,
        &[
            "extension",
            "install",
            "cua-perception",
            "--catalog",
            catalog_path.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("signature verification failed"));
    assert!(!home.join("extensions").exists());
}

#[test]
fn help_and_generated_surfaces_describe_the_secure_contract() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let help = String::from_utf8_lossy(&run(&home, &["--help"]).stdout).into_owned();
    assert!(help.contains("SIGNED TARGET-SPECIFIC CATALOGS"));
    assert!(help.contains("extension inspect"));
    assert!(help.contains("extension remove"));
    assert!(help.contains("--allow-unsigned-local"));

    let manifest = String::from_utf8_lossy(&run(&home, &["manifest"]).stdout).to_lowercase();
    assert!(manifest.contains("signed, target-specific"));
    assert!(manifest.contains("publisher identity"));
    assert!(manifest.contains("developer-only unsigned"));

    let docs_output = run(&home, &["dump-docs", "--type", "commands"]);
    let docs = String::from_utf8_lossy(&docs_output.stdout).to_lowercase();
    assert!(docs.contains("original-model, and converted-model hashes"));
    assert!(docs.contains("component licenses/notices"));
    assert!(docs.contains("license, source, and provenance"));
}

#[test]
fn cli_rejects_ambiguous_extension_arguments() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    for args in [
        vec!["extension", "list", "extra"],
        vec!["extension", "status", "--json", "--json"],
        vec!["extension", "install", "cua-perception", "--archive"],
        vec![
            "extension",
            "install",
            "cua-perception",
            "--archive",
            "x",
            "--catalog",
            "y",
            "--allow-unsigned-local",
        ],
        vec!["extension", "path", "cua-perception"],
    ] {
        let output = run(&home, &args);
        assert!(!output.status.success(), "accepted {args:?}");
    }
}
