#[cfg(feature = "review-trust-root")]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::{write::GzEncoder, Compression};
#[cfg(feature = "review-trust-root")]
use ring::signature::Ed25519KeyPair;
#[cfg(feature = "review-trust-root")]
use serde::Serialize;
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
    fixture_archive_with_legal_bindings(directory, version, true)
}

fn fixture_archive_with_legal_bindings(
    directory: &Path,
    version: &str,
    legal_bindings: bool,
) -> PathBuf {
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
    let model = b"fixture-model";
    let model_digest = format!("{:x}", Sha256::digest(model));
    let model_license = b"Fixture model license\n";
    let model_license_digest = format!("{:x}", Sha256::digest(model_license));
    let notice = b"Copyright Cua contributors\n";
    let notice_digest = format!("{:x}", Sha256::digest(notice));
    let corresponding_source = b"source revision integration-fixture\n";
    let corresponding_source_digest = format!("{:x}", Sha256::digest(corresponding_source));
    let mut manifest_value = json!({
        "schema_version": 1,
        "id": "cua-perception",
        "version": version,
        "driver_version": format!("={}", env!("CARGO_PKG_VERSION")),
        "protocol_version": 1,
        "target": current_target(),
        "entrypoint": entrypoint,
        "files": [
            {"path": entrypoint, "sha256": digest, "executable": true},
            {"path": "models/parser.bin", "sha256": model_digest},
            {"path": "LICENSES/model.txt", "sha256": model_license_digest},
            {"path": "LICENSES/NOTICE.txt", "sha256": notice_digest},
            {"path": "SOURCE/corresponding-source.txt", "sha256": corresponding_source_digest}
        ],
        "models": [{
            "path": "models/parser.bin", "revision": "model-v1",
            "original_sha256": "11".repeat(32), "conversion_sha256": model_digest,
            "license_file": {"path": "LICENSES/model.txt", "sha256": model_license_digest}
        }],
        "components": [{
            "name": "cua-perception", "version": version, "license": "Apache-2.0",
            "notice": "Copyright Cua contributors", "source_uri": "https://github.com/trycua/cua",
            "source_revision": "integration-fixture",
            "notice_file": {"path": "LICENSES/NOTICE.txt", "sha256": notice_digest}
        }],
        "corresponding_source_file": {"path": "SOURCE/corresponding-source.txt", "sha256": corresponding_source_digest},
        "license": "Apache-2.0",
        "source": "https://github.com/trycua/cua",
        "corresponding_source_uri": "https://github.com/trycua/cua",
        "corresponding_source_revision": "integration-fixture",
        "provenance": "integration test fixture",
        // Hooks now run inside the perception worker's containment boundary,
        // which grants execution to the entrypoint image alone. A scripted
        // fixture entrypoint cannot launch there, so this end-to-end fixture
        // declares no hook; hook behavior is covered by the focused
        // extension-manager and containment unit tests.
        "health_args": [],
        "self_test_args": []
    });
    if !legal_bindings {
        manifest_value["components"][0]
            .as_object_mut()
            .unwrap()
            .remove("notice_file");
    }
    let manifest = serde_json::to_vec_pretty(&manifest_value).unwrap();
    let path = directory.join(format!("cua-perception-{version}.tar.gz"));
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, "extension.json", &manifest);
    append(&mut builder, entrypoint, payload);
    append(&mut builder, "models/parser.bin", model);
    append(&mut builder, "LICENSES/model.txt", model_license);
    append(&mut builder, "LICENSES/NOTICE.txt", notice);
    append(
        &mut builder,
        "SOURCE/corresponding-source.txt",
        corresponding_source,
    );
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

#[cfg(feature = "review-trust-root")]
#[derive(Clone, Serialize)]
struct ReviewCatalogPayload {
    schema_version: u32,
    catalog_version: u64,
    expires_unix: u64,
    publisher_id: String,
    publisher_name: String,
    key_id: String,
    extension_id: String,
    version: String,
    target: String,
    archive: String,
    archive_size: u64,
    archive_sha256: String,
    manifest_sha256: String,
    license: String,
    source: String,
    corresponding_source_uri: String,
    corresponding_source_revision: String,
    provenance: String,
    next_key: Option<Value>,
}

#[cfg(feature = "review-trust-root")]
fn archive_manifest(archive: &Path) -> Vec<u8> {
    let decoder = flate2::read::GzDecoder::new(fs::File::open(archive).unwrap());
    let mut tar = tar::Archive::new(decoder);
    for item in tar.entries().unwrap() {
        let mut item = item.unwrap();
        if item.path().unwrap() == Path::new("extension.json") {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut item, &mut bytes).unwrap();
            return bytes;
        }
    }
    panic!("fixture archive omitted extension.json")
}

#[cfg(feature = "review-trust-root")]
fn review_catalog(
    directory: &Path,
    archive: &Path,
    version: &str,
    catalog_version: u64,
    expires_unix: u64,
    target: String,
    seed: [u8; 32],
) -> PathBuf {
    let archive_bytes = fs::read(archive).unwrap();
    let manifest = archive_manifest(archive);
    let payload = ReviewCatalogPayload {
        schema_version: 1,
        catalog_version,
        expires_unix,
        publisher_id: "cua-review-only".to_owned(),
        publisher_name: "Cua REVIEW ONLY".to_owned(),
        key_id: "review-only-build-override".to_owned(),
        extension_id: "cua-perception".to_owned(),
        version: version.to_owned(),
        target,
        archive: archive.file_name().unwrap().to_str().unwrap().to_owned(),
        archive_size: archive_bytes.len() as u64,
        archive_sha256: format!("{:x}", Sha256::digest(&archive_bytes)),
        manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
        license: "Apache-2.0".to_owned(),
        source: "https://github.com/trycua/cua".to_owned(),
        corresponding_source_uri: "https://github.com/trycua/cua".to_owned(),
        corresponding_source_revision: "integration-fixture".to_owned(),
        provenance: "integration test fixture".to_owned(),
        next_key: None,
    };
    let pair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let signature = BASE64.encode(pair.sign(&serde_json::to_vec(&payload).unwrap()).as_ref());
    let catalog = json!({
        "payload": payload,
        "signature_algorithm": "ed25519",
        "signature": signature,
    });
    let path = directory.join(format!("catalog-{version}-{catalog_version}.json"));
    fs::write(&path, serde_json::to_vec_pretty(&catalog).unwrap()).unwrap();
    path
}

#[cfg(feature = "review-trust-root")]
const REVIEW_SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

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
    assert_eq!(preview["name"], "Cua Perception");
    assert_eq!(preview["trust"], "developer-unsigned-local");
    assert_eq!(preview["signing_key_status"], "unsigned-local");
    assert_eq!(
        preview["artifact_source"],
        archive.to_string_lossy().as_ref()
    );
    assert_eq!(
        preview["destination"],
        home.join("extensions/cua-perception/versions/1.2.3")
            .to_string_lossy()
            .as_ref()
    );
    assert!(preview["download_size"].as_u64().unwrap() > 0);
    assert!(preview["installed_size"].as_u64().unwrap() > 0);
    assert_eq!(preview["license"], "Apache-2.0");
    assert_eq!(preview["models"][0]["revision"], "model-v1");
    assert_eq!(preview["models"][0]["original_sha256"], "11".repeat(32));
    assert_eq!(
        preview["models"][0]["conversion_sha256"],
        preview["files"][1]["sha256"]
    );
    assert_eq!(preview["components"][0]["license"], "Apache-2.0");
    assert_eq!(preview["model_licenses"][0]["model"], "models/parser.bin");
    assert_eq!(preview["authorization"]["request"], "cli-inspect");
    assert_eq!(preview["authorization"]["confirmation_required"], false);
    assert_eq!(preview["authorization"]["mutation_authorized"], false);
    assert_eq!(preview["authorization"]["mutation_performed"], false);
    assert_eq!(preview["mutation_performed"], false);
    assert!(!home.join("extensions/cua-perception").exists());

    let inspect_text = run(
        &home,
        &[
            "extension",
            "inspect",
            "cua-perception",
            "--archive",
            archive.to_str().unwrap(),
            "--allow-unsigned-local",
        ],
    );
    assert!(inspect_text.status.success());
    assert!(String::from_utf8_lossy(&inspect_text.stdout)
        .trim_end()
        .ends_with("Preview complete; no extension state was changed."));
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
    let install_stdout = String::from_utf8_lossy(&install.stdout);
    assert!(install_stdout.contains("developer-unsigned-local"));
    assert!(install_stdout.contains("Signing key status: unsigned-local"));
    assert!(install_stdout.contains("Destination:"));
    assert!(install_stdout.contains("Download size:"));
    assert!(install_stdout.contains("Model: models/parser.bin @ model-v1"));
    assert!(install_stdout.contains("Model license file: LICENSES/model.txt"));
    assert!(install_stdout.contains("mutation_authorized=true"));
    assert!(install_stdout.contains("Installed cua-perception 1.2.3 at "));
    assert!(
        !install_stdout.contains("no extension state was changed"),
        "install must not claim that nothing changed:\n{install_stdout}"
    );

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

#[cfg(not(feature = "review-trust-root"))]
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
            "key_id": "cua-extension-ed25519-2026-09", "extension_id": "cua-perception",
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
fn mcp_inventory_exposes_only_the_confirm_gated_extension_installer() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let listed = run(&home, &["list-tools"]);
    assert!(listed.status.success());
    let tools = String::from_utf8_lossy(&listed.stdout);
    assert!(tools
        .contains("install_extension: Preview or install one Driver-managed optional extension"));

    let described = run(&home, &["describe", "install_extension"]);
    assert!(described.status.success());
    let description = String::from_utf8_lossy(&described.stdout);
    assert!(description.contains("confirm"));
    assert!(description.contains("perception"));
    assert!(!description.contains("catalog"));
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

#[test]
fn cli_parse_errors_do_not_create_extension_state() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let output = run(
        &home,
        &["extension", "install", "cua-perception", "--catalog"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--catalog requires a value"));
    assert!(!home.join("extensions").exists());
}

#[cfg(feature = "review-trust-root")]
#[test]
fn review_signed_lifecycle_is_distinct_fail_closed_and_rollback_safe() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("driver-home");
    let archive = fixture_archive(temp.path(), "1.2.3");
    let catalog = review_catalog(
        temp.path(),
        &archive,
        "1.2.3",
        1,
        4_102_444_800,
        current_target(),
        REVIEW_SEED,
    );

    let inspect = run(
        &home,
        &[
            "extension",
            "inspect",
            "perception",
            "--catalog",
            catalog.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let preview: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(preview["trust"], "review-only-publisher-verified");
    assert_eq!(
        preview["evidence_class"],
        "review-only-not-release-evidence"
    );
    assert_eq!(preview["target"], current_target());
    assert_eq!(preview["version"], "1.2.3");
    assert_eq!(preview["publisher_id"], "cua-review-only");
    assert_eq!(preview["publisher_name"], "Cua REVIEW ONLY");
    assert_eq!(preview["publisher_key_id"], "review-only-build-override");
    assert_eq!(preview["signing_key_algorithm"], "ed25519");
    assert_eq!(preview["signing_key_status"], "verified");
    assert_eq!(preview["publisher_signature_verified"], true);
    assert_eq!(
        preview["destination"],
        home.join("extensions/cua-perception/versions/1.2.3")
            .to_string_lossy()
            .as_ref()
    );
    assert!(preview["download_size"].as_u64().unwrap() > 0);
    assert!(preview["installed_size"].as_u64().unwrap() > 0);
    assert_eq!(preview["models"][0]["revision"], "model-v1");
    assert_eq!(preview["license_notices"][0]["license"], "Apache-2.0");
    assert_eq!(preview["authorization"]["request"], "cli-inspect");
    assert_eq!(preview["authorization"]["mutation_authorized"], false);
    assert_eq!(preview["mutation_performed"], false);

    let install = run(
        &home,
        &[
            "extension",
            "install",
            "perception",
            "--catalog",
            catalog.to_str().unwrap(),
        ],
    );
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    let status = run(&home, &["extension", "status", "perception", "--json"]);
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active_version"], "1.2.3");
    assert_eq!(status_json["trust"], "review-only-publisher-verified");
    assert_eq!(
        status_json["evidence_class"],
        "review-only-not-release-evidence"
    );
    assert!(status_json["detail"]
        .as_str()
        .unwrap()
        .contains("REVIEW ONLY"));

    let update_archive = fixture_archive(temp.path(), "1.2.4");
    let update_catalog = review_catalog(
        temp.path(),
        &update_archive,
        "1.2.4",
        2,
        4_102_444_800,
        current_target(),
        REVIEW_SEED,
    );
    let update = run(
        &home,
        &[
            "extension",
            "update",
            "perception",
            "--catalog",
            update_catalog.to_str().unwrap(),
        ],
    );
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );

    let rollback_archive = fixture_archive(temp.path(), "1.2.2");
    let rollback_catalog = review_catalog(
        temp.path(),
        &rollback_archive,
        "1.2.2",
        3,
        4_102_444_800,
        current_target(),
        REVIEW_SEED,
    );
    let rollback = run(
        &home,
        &[
            "extension",
            "update",
            "perception",
            "--catalog",
            rollback_catalog.to_str().unwrap(),
        ],
    );
    assert!(!rollback.status.success());
    assert!(String::from_utf8_lossy(&rollback.stderr).contains("must be newer"));
    let status = run(&home, &["extension", "status", "perception", "--json"]);
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active_version"], "1.2.4");

    let replay = run(
        &home,
        &[
            "extension",
            "update",
            "perception",
            "--catalog",
            update_catalog.to_str().unwrap(),
        ],
    );
    assert!(!replay.status.success());
    assert!(String::from_utf8_lossy(&replay.stderr).contains("anti-rollback"));
}

#[cfg(feature = "review-trust-root")]
#[test]
fn review_catalog_rejects_wrong_key_tamper_expiry_target_and_missing_legal_files() {
    let temp = TempDir::new().unwrap();
    let archive = fixture_archive(temp.path(), "1.2.3");
    let cases = [
        (
            "wrong-key",
            review_catalog(
                temp.path(),
                &archive,
                "1.2.3",
                11,
                4_102_444_800,
                current_target(),
                [7; 32],
            ),
            "signature verification failed",
        ),
        (
            "expired",
            review_catalog(
                temp.path(),
                &archive,
                "1.2.3",
                12,
                1,
                current_target(),
                REVIEW_SEED,
            ),
            "expired",
        ),
        (
            "target",
            review_catalog(
                temp.path(),
                &archive,
                "1.2.3",
                13,
                4_102_444_800,
                "x86_64-unknown-linux-gnu".to_owned(),
                REVIEW_SEED,
            ),
            "target does not match",
        ),
    ];
    for (name, catalog, expected) in cases {
        let home = temp.path().join(format!("home-{name}"));
        let output = run(
            &home,
            &[
                "extension",
                "install",
                "perception",
                "--catalog",
                catalog.to_str().unwrap(),
            ],
        );
        assert!(!output.status.success(), "accepted {name}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(!home.join("extensions/cua-perception").exists());
    }

    let valid = review_catalog(
        temp.path(),
        &archive,
        "1.2.3",
        14,
        4_102_444_800,
        current_target(),
        REVIEW_SEED,
    );
    let mut tampered: Value = serde_json::from_slice(&fs::read(&valid).unwrap()).unwrap();
    tampered["payload"]["archive_size"] = json!(1);
    fs::write(&valid, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let tamper_home = temp.path().join("home-tamper");
    let output = run(
        &tamper_home,
        &[
            "extension",
            "install",
            "perception",
            "--catalog",
            valid.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("signature verification failed"));

    let incomplete = fixture_archive_with_legal_bindings(temp.path(), "1.2.5", false);
    let incomplete_catalog = review_catalog(
        temp.path(),
        &incomplete,
        "1.2.5",
        15,
        4_102_444_800,
        current_target(),
        REVIEW_SEED,
    );
    let incomplete_home = temp.path().join("home-incomplete");
    let output = run(
        &incomplete_home,
        &[
            "extension",
            "install",
            "perception",
            "--catalog",
            incomplete_catalog.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("digest-bound notice"));
}
