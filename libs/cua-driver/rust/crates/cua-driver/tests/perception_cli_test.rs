use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::{write::GzEncoder, Compression};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn run(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1")
        .output()
        .expect("run cua-driver perception command")
}

fn write_fixture(temp: &TempDir) -> (String, String) {
    let image_path = temp.path().join("input.png");
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255])))
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    fs::write(&image_path, bytes).unwrap();

    let capture_path = temp.path().join("capture.json");
    fs::write(
        &capture_path,
        serde_json::to_vec(&json!({
            "source": {"kind": "window", "pid": 123, "window_id": 456},
            "snapshot_id": "snapshot-local-fixture"
        }))
        .unwrap(),
    )
    .unwrap();
    (
        image_path.to_string_lossy().into_owned(),
        capture_path.to_string_lossy().into_owned(),
    )
}

fn parse_error(output: &Output) -> serde_json::Value {
    assert!(!output.status.success());
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {:?}",
        output.stderr
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    value
}

fn run_parse(home: &std::path::Path, image: &str, capture: &str) -> Output {
    run(
        home,
        &[
            "perception",
            "parse",
            "--image",
            image,
            "--capture",
            capture,
            "--json",
        ],
    )
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
        panic!("unsupported perception CLI test target")
    };
    format!("{}-{suffix}", std::env::consts::ARCH)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

fn compile_fixture_worker(directory: &Path) -> PathBuf {
    let source = directory.join("perception-fixture-worker.rs");
    fs::write(
        &source,
        r###"
use std::io::{self, Read, Write};

fn read_frame() -> String {
    let mut length = [0_u8; 4];
    io::stdin().read_exact(&mut length).unwrap();
    let mut payload = vec![0_u8; u32::from_be_bytes(length) as usize];
    io::stdin().read_exact(&mut payload).unwrap();
    String::from_utf8(payload).unwrap()
}

fn field(payload: &str, name: &str) -> String {
    let marker = format!("\"{name}\":\"");
    let tail = &payload[payload.find(&marker).unwrap() + marker.len()..];
    tail[..tail.find('"').unwrap()].to_owned()
}

fn write_frame(payload: &str) {
    let bytes = payload.as_bytes();
    let mut stdout = io::stdout().lock();
    stdout.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
    stdout.write_all(bytes).unwrap();
    stdout.flush().unwrap();
}

fn argument(name: &str) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    let index = args.iter().position(|value| value == name).unwrap();
    args[index + 1].clone()
}

fn main() {
    let extension_id = argument("--extension-id");
    let extension_version = argument("--extension-version");
    let identity = format!(
        r#"{{"extension":{{"id":"{}","version":"{}"}},"backend":"deterministic_fixture","fixture_sha256":"{}"}}"#,
        extension_id,
        extension_version,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );

    let health = read_frame();
    write_frame(&format!(
        r#"{{"protocol":"cua-perception/1","request_id":"{}","status":"ok","result":{{"ready":true,"protocol":"cua-perception/1","identity":{}}}}}"#,
        field(&health, "request_id"), identity
    ));

    let parse = read_frame();
    write_frame(&format!(
        r#"{{"protocol":"cua-perception/1","request_id":"{}","status":"ok","result":{{"runtime":"fixture_only","identity":{},"regions":[{{"id":"fixture-text","kind":"text","bounds":{{"x":0,"y":0,"width":2,"height":1}},"text":"fixture","confidence":0.99,"interactive":false,"reading_order":0}}]}}}}"#,
        field(&parse, "request_id"), identity
    ));
}
"###,
    )
    .unwrap();

    let worker = directory.join(if cfg!(windows) {
        "cua-perception.exe"
    } else {
        "cua-perception"
    });
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg(&source)
        .arg("--edition=2021")
        .arg("-o")
        .arg(&worker)
        .output()
        .expect("compile deterministic perception fixture worker");
    assert!(
        output.status.success(),
        "fixture worker compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    worker
}

fn fixture_extension_archive(directory: &Path) -> PathBuf {
    let version = "1.0.0";
    let worker_path = compile_fixture_worker(directory);
    let worker = fs::read(&worker_path).unwrap();
    let worker_name = worker_path.file_name().unwrap().to_str().unwrap();
    let entrypoint = format!("bin/{worker_name}");
    let runtime_name = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let runtime = b"fixture-runtime".to_vec();
    let model = b"fixture-model".to_vec();
    let dictionary = b"fixture-dictionary".to_vec();
    let model_manifest = b"{}".to_vec();
    let notice = b"deterministic fixture notice\n".to_vec();
    let model_license = b"deterministic fixture model license\n".to_vec();
    let source = b"deterministic fixture source\n".to_vec();
    let contract = serde_json::to_vec_pretty(&json!({
        "$schema": "runtime-contract.schema.json",
        "schemaVersion": 1,
        "target": current_target(),
        "protocolVersion": 1,
        "worker": {"name": worker_name, "sha256": sha256(&worker)},
        "runtime": {"name": runtime_name, "sha256": sha256(&runtime)},
        "models": [
            {"name": "icon.onnx", "role": "icon-detect", "sha256": sha256(&model)},
            {"name": "ocr-det.onnx", "role": "ocr-detect", "sha256": sha256(&model)},
            {"name": "ocr-rec.onnx", "role": "ocr-recognize", "sha256": sha256(&model)}
        ],
        "dictionary": {
            "name": "dictionary.txt",
            "role": "ocr-dictionary",
            "sha256": sha256(&dictionary)
        },
        "rejectMismatch": true
    }))
    .unwrap();
    let files = vec![
        (entrypoint.clone(), worker, true),
        (
            "models/model-manifest.json".to_owned(),
            model_manifest,
            false,
        ),
        (format!("runtime/{runtime_name}"), runtime, false),
        ("models/icon.onnx".to_owned(), model.clone(), false),
        ("models/ocr-det.onnx".to_owned(), model.clone(), false),
        ("models/ocr-rec.onnx".to_owned(), model, false),
        ("models/dictionary.txt".to_owned(), dictionary, false),
        ("metadata/runtime-contract.json".to_owned(), contract, false),
        ("LICENSES/NOTICE.txt".to_owned(), notice, false),
        ("LICENSES/model.txt".to_owned(), model_license, false),
        ("SOURCE/fixture.txt".to_owned(), source, false),
    ];
    let file_json = files
        .iter()
        .map(|(path, bytes, executable)| {
            json!({"path": path, "sha256": sha256(bytes), "executable": executable})
        })
        .collect::<Vec<_>>();
    let file_hash = |path: &str| {
        let (_, bytes, _) = files
            .iter()
            .find(|(candidate, _, _)| candidate == path)
            .unwrap();
        sha256(bytes)
    };
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "id": "cua-perception",
        "version": version,
        "driver_version": format!("={}", env!("CARGO_PKG_VERSION")),
        "protocol_version": 1,
        "target": current_target(),
        "entrypoint": entrypoint,
        "files": file_json,
        "models": [{
            "path": "models/icon.onnx",
            "revision": "fixture-v1",
            "original_sha256": file_hash("models/icon.onnx"),
            "conversion_sha256": file_hash("models/icon.onnx"),
            "license_file": {"path": "LICENSES/model.txt", "sha256": file_hash("LICENSES/model.txt")}
        }],
        "components": [{
            "name": "cua-perception-fixture",
            "version": version,
            "license": "Apache-2.0",
            "notice": "deterministic CLI integration fixture",
            "source_uri": "https://github.com/trycua/cua",
            "source_revision": "integration-fixture",
            "notice_file": {"path": "LICENSES/NOTICE.txt", "sha256": file_hash("LICENSES/NOTICE.txt")}
        }],
        "corresponding_source_file": {"path": "SOURCE/fixture.txt", "sha256": file_hash("SOURCE/fixture.txt")},
        "license": "Apache-2.0",
        "source": "https://github.com/trycua/cua",
        "corresponding_source_uri": "https://github.com/trycua/cua",
        "corresponding_source_revision": "integration-fixture",
        "provenance": "deterministic CLI integration fixture",
        "health_args": [],
        "self_test_args": []
    }))
    .unwrap();

    let archive = directory.join("cua-perception-fixture.tar.gz");
    let encoder = GzEncoder::new(fs::File::create(&archive).unwrap(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, "extension.json", &manifest);
    for (path, bytes, _) in files {
        append(&mut builder, &path, &bytes);
    }
    builder.finish().unwrap();
    archive
}

#[test]
fn installed_fixture_parses_local_input_without_action_authority() {
    let temp = TempDir::new().unwrap();
    let archive = fixture_extension_archive(temp.path());
    let install = run(
        temp.path(),
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
        "extension install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let (image, capture) = write_fixture(&temp);
    let output = run_parse(temp.path(), &image, &capture);
    assert!(
        output.status.success(),
        "perception parse failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(value["local_input"]["action_authority"], "none");
    assert_eq!(value["local_input"]["action_eligible"], false);
    assert_eq!(
        value["local_input"]["snapshot_id"],
        "snapshot-local-fixture"
    );
    let capture_id = value["capture"]["capture_id"].as_str().unwrap();
    assert!(capture_id.starts_with("local_png_"));
    assert!(!capture_id.starts_with("capture_"));
    assert_eq!(value["capture"]["source"]["kind"], "window");
    assert_eq!(value["capture"]["source"]["pid"], 123);
    assert_eq!(value["capture"]["source"]["window_id"], 456);
    assert_eq!(value["capture"]["screenshot"]["width"], 2);
    assert_eq!(value["capture"]["screenshot"]["height"], 1);
    assert_eq!(value["capture"]["screenshot"]["mime_type"], "image/png");
    assert_eq!(
        value["capture"]["screenshot"]["sha256"],
        value["local_input"]["sha256"]
    );
    assert!(value["capture"]["screenshot"]["reference"]
        .as_str()
        .unwrap()
        .starts_with("local-png-sha256:"));
    assert_eq!(value["parser"]["extension_id"], "cua-perception");
    assert_eq!(value["parser"]["extension_version"], "1.0.0");
    assert_eq!(value["parser"]["backend"], "deterministic_fixture");
    assert_eq!(value["parser"]["runtime"], "fixture_only");
    assert_eq!(value["regions"][0]["id"], "fixture-text");
    assert_eq!(value["regions"][0]["kind"], "text");
    assert_eq!(value["regions"][0]["text"], "fixture");
    assert_eq!(
        value["regions"][0]["bounds"],
        json!({"x": 0, "y": 0, "width": 2, "height": 1})
    );
}

#[test]
fn help_documents_local_parse_as_non_actionable() {
    let temp = TempDir::new().unwrap();
    let output = run(temp.path(), &["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("perception parse --image <png> --capture <capture.json> --json"));
    assert!(stdout.contains("no Driver action authority"));

    let manifest = run(temp.path(), &["manifest"]);
    assert!(manifest.status.success());
    let manifest_json: serde_json::Value = serde_json::from_slice(&manifest.stdout).unwrap();
    let perception = manifest_json["subcommands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == "perception")
        .expect("manifest perception command");
    assert!(perception["description"]
        .as_str()
        .unwrap()
        .contains("without creating Driver capture or action authority"));

    let docs = run(temp.path(), &["dump-docs", "--type", "commands"]);
    assert!(docs.status.success());
    let docs_text = String::from_utf8_lossy(&docs.stdout);
    assert!(docs_text.contains("local-input mode does not create a Driver capture"));
}

#[test]
fn valid_local_input_without_extension_returns_not_installed() {
    let temp = TempDir::new().unwrap();
    let (image, capture) = write_fixture(&temp);
    let output = run_parse(temp.path(), &image, &capture);
    assert_eq!(parse_error(&output)["error"]["code"], "not_installed");
    assert!(!temp.path().join("extensions").exists());
}

#[test]
fn invalid_metadata_and_png_fail_before_worker_launch() {
    let temp = TempDir::new().unwrap();
    let (image, capture) = write_fixture(&temp);
    fs::write(
        &capture,
        br#"{"source":{"kind":"window","pid":0,"window_id":1}}"#,
    )
    .unwrap();
    let invalid_metadata = run_parse(temp.path(), &image, &capture);
    assert_eq!(
        parse_error(&invalid_metadata)["error"]["code"],
        "invalid_capture_metadata"
    );

    let (_, capture) = write_fixture(&temp);
    fs::write(&image, b"not a png").unwrap();
    let invalid_png = run_parse(temp.path(), &image, &capture);
    assert_eq!(parse_error(&invalid_png)["error"]["code"], "invalid_png");
}

#[test]
fn invalid_installed_extension_fails_as_artifact_invalid() {
    let temp = TempDir::new().unwrap();
    let (image, capture) = write_fixture(&temp);
    let extension = temp.path().join("extensions/cua-perception");
    fs::create_dir_all(&extension).unwrap();
    fs::write(extension.join("active.json"), b"not json").unwrap();
    let output = run_parse(temp.path(), &image, &capture);
    assert_eq!(parse_error(&output)["error"]["code"], "artifact_invalid");
}

#[test]
fn parser_rejects_missing_json_or_paths() {
    let temp = TempDir::new().unwrap();
    for args in [
        vec!["perception", "parse", "--json"],
        vec!["perception", "parse", "--image", "input.png", "--json"],
        vec![
            "perception",
            "parse",
            "--image",
            "input.png",
            "--capture",
            "capture.json",
        ],
    ] {
        assert!(
            !run(temp.path(), &args).status.success(),
            "accepted {args:?}"
        );
    }
}

#[test]
fn argument_failure_has_an_exact_json_envelope() {
    let temp = TempDir::new().unwrap();
    let output = run(temp.path(), &["perception", "parse", "--json"]);
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "{\"ok\":false,\"error\":{\"code\":\"invalid_arguments\",\"message\":\"perception parse requires --image <png>\",\"retryable\":false}}\n"
    );
}

#[test]
fn rejects_unknown_nested_source_fields_and_malformed_timestamp() {
    let temp = TempDir::new().unwrap();
    let (image, capture) = write_fixture(&temp);
    fs::write(
        &capture,
        br#"{"source":{"kind":"window","pid":123,"window_id":456,"extra":true}}"#,
    )
    .unwrap();
    let unknown = run_parse(temp.path(), &image, &capture);
    assert_eq!(
        parse_error(&unknown)["error"]["code"],
        "invalid_capture_metadata"
    );

    fs::write(
        &capture,
        br#"{"source":{"kind":"window","pid":123,"window_id":456},"captured_at":"yesterday"}"#,
    )
    .unwrap();
    let timestamp = run_parse(temp.path(), &image, &capture);
    let error = parse_error(&timestamp);
    assert_eq!(error["error"]["code"], "invalid_capture_metadata");
    assert_eq!(
        error["error"]["message"],
        "captured_at must be an RFC3339 timestamp"
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlink_and_fifo_inputs_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let (image, capture) = write_fixture(&temp);
    let image_link = temp.path().join("image-link.png");
    symlink(&image, &image_link).unwrap();
    let symlink_output = run_parse(temp.path(), image_link.to_str().unwrap(), capture.as_str());
    assert_eq!(
        parse_error(&symlink_output)["error"]["code"],
        "input_open_failed"
    );

    let fifo = temp.path().join("capture.fifo");
    let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let fifo_output = run_parse(temp.path(), &image, fifo.to_str().unwrap());
    assert_eq!(
        parse_error(&fifo_output)["error"]["code"],
        "input_not_regular_file"
    );
}
