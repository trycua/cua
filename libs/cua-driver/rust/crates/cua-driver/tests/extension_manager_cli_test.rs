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
    let suffix = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        "linux" => "unknown-linux-gnu",
        other => other,
    };
    format!("{}-{suffix}", std::env::consts::ARCH)
}

fn fixture_archive(directory: &Path) -> PathBuf {
    let payload = b"fixture-worker";
    let digest = format!("{:x}", Sha256::digest(payload));
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "id": "perception",
        "version": "1.2.3",
        "driver_version": format!("={}", env!("CARGO_PKG_VERSION")),
        "protocol_version": 1,
        "target": current_target(),
        "entrypoint": "bin/cua-perception",
        "files": [{
            "path": "bin/cua-perception",
            "sha256": digest,
            "executable": true
        }]
    }))
    .unwrap();
    let path = directory.join("perception.tar.gz");
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, "extension.json", &manifest);
    append(&mut builder, "bin/cua-perception", payload);
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
    assert_eq!(list_json[0]["id"], "perception");
    assert_eq!(list_json[0]["installed"], false);

    let absent = run(&home, &["extension", "status", "perception", "--json"]);
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
            "perception",
            "--archive",
            archive.to_str().unwrap(),
        ],
    );
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );

    let status = run(&home, &["extension", "status", "perception", "--json"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active_version"], "1.2.3");
    assert_eq!(status_json["healthy"], true);

    let path = run(&home, &["extension", "path", "perception"]);
    assert!(
        path.status.success(),
        "{}",
        String::from_utf8_lossy(&path.stderr)
    );
    let actual = String::from_utf8(path.stdout).unwrap();
    assert_eq!(
        Path::new(actual.trim()),
        home.join("extensions/perception/versions/1.2.3")
    );
}
