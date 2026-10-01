//! Canonical desktop rows for the optional perception loop.
//!
//! The documented loop is `get_window_state` (retain `capture_id`) ->
//! `parse_visual_regions` -> one capture-bound `click` with the same
//! `capture_id` -> reobserve. Two kinds of row drive it.
//!
//! The contract rows run against the shared web harness in Electron, which
//! every supported desktop runner builds, and need no published artifact or
//! model. Their installed extension is a deterministic, developer-only unsigned
//! worker compiled from `support/perception_swatch_worker.rs`. It speaks the
//! real framed worker protocol, runs inside Driver's normal worker containment,
//! receives the exact PNG Driver retained for the capture, and derives regions
//! from those pixels by finding the harness's solid color swatches. These rows
//! certify Driver's capture, parse, action binding, and single-use refusal
//! contract on each desktop.
//!
//! The published-catalog row installs the released `cua-perception` extension
//! from its signed `cua-perception-v<version>` release catalog, the documented
//! user path, and requires `publisher-verified` trust. The released OmniParser
//! model reads the "Cancel" label on the visual-only Tk canvas fixture, which
//! exposes no accessibility tree, and one capture-bound click selects that
//! card. The row downloads the pinned release assets (about 425 MB) and needs
//! a Python with Tk.
//!
//! Each fixture's loopback journal is the delivery oracle, independent of the
//! Driver response. macOS runs a dedicated instance of the installed,
//! TCC-authorized app with an isolated extension home, so the shared daemon's
//! state is never changed.
//!
//! ```text
//! cargo test -p cua-driver-e2e --test perception_capture_loop_test -- \
//!   --ignored --nocapture --test-threads=1
//! ```

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, native_readonly_case, recording_evidence, shared_web_route, CaseSpec, Delivery,
    DisplayServer, DriverRoute, Evidence, Observation, OracleKind, Platform, Scope, Targeting,
};
use cua_driver_testkit::{
    driver_binary, ensure_driver_binary, harness_app, spawn_in_job, Driver, FixtureJournal,
    McpDriver, ToolResponse,
};
use flate2::{write::GzEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const WORKER_SOURCE: &str = include_str!("support/perception_swatch_worker.rs");
const EXTENSION_VERSION: &str = "0.0.0-e2e-swatch";
const TARGET_REGION: &str = "swatch-drag-source";
const FIXTURE_TITLE: &str = "CuaTestHarness Electron";

// ---------------------------------------------------------------- extension

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
        panic!("unsupported perception E2E target")
    };
    format!("{}-{suffix}", std::env::consts::ARCH)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn worker_file_name() -> &'static str {
    if cfg!(windows) {
        "cua-perception.exe"
    } else {
        "cua-perception"
    }
}

/// Compile the swatch worker with plain `rustc`. Linux containment cannot
/// expose a host dynamic loader to an installed worker, and release Windows
/// builds link the C runtime statically, so both link it statically here.
fn compile_worker(directory: &Path) -> PathBuf {
    let source = directory.join("perception_swatch_worker.rs");
    fs::write(&source, WORKER_SOURCE).expect("write swatch worker source");
    let worker = directory.join(worker_file_name());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut command = Command::new(rustc);
    command.arg(&source).args(["--edition=2021", "-O"]);
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        command.args(["-C", "target-feature=+crt-static"]);
    }
    if cfg!(target_os = "linux") {
        if let Some(path) = std::env::var_os("CUA_TEST_GLIBC_STATIC_LIB") {
            let mut native_path = std::ffi::OsString::from("native=");
            native_path.push(path);
            command.arg("-L").arg(native_path);
        }
    }
    let output = command
        .arg("-o")
        .arg(&worker)
        .output()
        .expect("run rustc for the swatch worker");
    assert!(
        output.status.success(),
        "swatch worker compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    worker
}

fn append(builder: &mut tar::Builder<GzEncoder<fs::File>>, path: &str, bytes: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(bytes))
        .expect("append extension archive entry");
}

/// Assemble the developer-only archive accepted by
/// `cua-driver extension install --archive <tar.gz> --allow-unsigned-local`.
/// The model, runtime, and dictionary entries are inert placeholders that
/// satisfy the runtime contract; the swatch worker never loads them.
fn swatch_extension_archive(directory: &Path) -> PathBuf {
    let worker = fs::read(compile_worker(directory)).expect("read compiled swatch worker");
    let entrypoint = format!("bin/{}", worker_file_name());
    let runtime_name = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let runtime = b"e2e placeholder runtime; never loaded".to_vec();
    let model = b"e2e placeholder model; never loaded".to_vec();
    let dictionary = b"e2e placeholder dictionary".to_vec();
    let model_manifest = b"{}".to_vec();
    let contract = serde_json::to_vec_pretty(&json!({
        "$schema": "runtime-contract.schema.json",
        "schemaVersion": 1,
        "target": current_target(),
        "protocolVersion": 1,
        "worker": {"name": worker_file_name(), "sha256": sha256(&worker)},
        "runtime": {"name": runtime_name, "sha256": sha256(&runtime)},
        "modelManifest": {"name": "model-manifest.json", "sha256": sha256(&model_manifest)},
        "models": [
            {"name": "icon.onnx", "role": "icon-detect", "sha256": sha256(&model)},
            {"name": "ocr-det.onnx", "role": "ocr-detect", "sha256": sha256(&model)},
            {"name": "ocr-rec.onnx", "role": "ocr-recognize", "sha256": sha256(&model)}
        ],
        "dictionary": {"name": "dictionary.txt", "role": "ocr-dictionary", "sha256": sha256(&dictionary)},
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
        (
            "LICENSES/NOTICE.txt".to_owned(),
            b"Cua E2E swatch worker; test support only.\n".to_vec(),
            false,
        ),
        (
            "LICENSES/model.txt".to_owned(),
            b"No model is distributed with the E2E swatch worker.\n".to_vec(),
            false,
        ),
        (
            "SOURCE/swatch-worker.rs".to_owned(),
            WORKER_SOURCE.as_bytes().to_vec(),
            false,
        ),
    ];
    let hash_of = |path: &str| {
        files
            .iter()
            .find(|(candidate, _, _)| candidate == path)
            .map(|(_, bytes, _)| sha256(bytes))
            .expect("archive file is declared")
    };
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "id": "cua-perception",
        "version": EXTENSION_VERSION,
        "driver_version": format!("={}", env!("CARGO_PKG_VERSION")),
        "protocol_version": 1,
        "target": current_target(),
        "entrypoint": entrypoint,
        "files": files.iter().map(|(path, bytes, executable)| {
            json!({"path": path, "sha256": sha256(bytes), "executable": executable})
        }).collect::<Vec<_>>(),
        "models": [{
            "path": "models/icon.onnx",
            "revision": "e2e-swatch-v1",
            "original_sha256": hash_of("models/icon.onnx"),
            "conversion_sha256": hash_of("models/icon.onnx"),
            "license_file": {"path": "LICENSES/model.txt", "sha256": hash_of("LICENSES/model.txt")}
        }],
        "components": [{
            "name": "cua-perception-e2e-swatch-worker",
            "version": EXTENSION_VERSION,
            "license": "MIT",
            "notice": "Cua E2E swatch worker; test support only",
            "source_uri": "https://github.com/trycua/cua",
            "source_revision": "e2e-swatch",
            "notice_file": {"path": "LICENSES/NOTICE.txt", "sha256": hash_of("LICENSES/NOTICE.txt")}
        }],
        "corresponding_source_file": {
            "path": "SOURCE/swatch-worker.rs",
            "sha256": hash_of("SOURCE/swatch-worker.rs")
        },
        "license": "MIT",
        "source": "https://github.com/trycua/cua",
        "corresponding_source_uri": "https://github.com/trycua/cua",
        "corresponding_source_revision": "e2e-swatch",
        "provenance": "cua-driver-e2e deterministic swatch worker",
        "health_args": [],
        "self_test_args": []
    }))
    .unwrap();

    let archive = directory.join("cua-perception-e2e-swatch.tar.gz");
    let encoder = GzEncoder::new(
        fs::File::create(&archive).expect("create extension archive"),
        Compression::default(),
    );
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, "extension.json", &manifest);
    for (path, bytes, _) in &files {
        append(&mut builder, path, bytes);
    }
    builder
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .expect("finish extension archive");
    archive
}

/// Driver binary that owns the extension lifecycle for the daemon under test.
/// macOS certifies the installed app; other desktops run the source build.
fn lifecycle_binary() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        installed_macos_binary()
    }
    #[cfg(not(target_os = "macos"))]
    {
        driver_binary()
    }
}

fn run_extension_command(binary: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary)
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdin(Stdio::null())
        .output()
        .expect("run cua-driver extension command")
}

fn install_swatch_extension(binary: &Path, home: &Path, scratch: &Path) {
    let archive = swatch_extension_archive(scratch);
    let archive = archive.to_str().expect("UTF-8 archive path");
    let install = run_extension_command(
        binary,
        home,
        &[
            "extension",
            "install",
            "cua-perception",
            "--archive",
            archive,
            "--allow-unsigned-local",
        ],
    );
    assert!(
        install.status.success(),
        "developer-unsigned extension install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    let status = run_extension_command(
        binary,
        home,
        &["extension", "status", "cua-perception", "--json"],
    );
    assert!(
        status.status.success(),
        "extension status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("extension status JSON");
    assert_eq!(status["installed"], true, "extension status: {status}");
    assert_eq!(
        status["trust"], "developer-unsigned-local",
        "the E2E worker must never be represented as publisher-verified: {status}"
    );
    assert_eq!(status["active_version"], EXTENSION_VERSION);
}

/// Fresh private directory used as `CUA_DRIVER_RS_HOME` for one row. Windows
/// uses a real child of the user profile, like the reviewed extension lanes,
/// so worker containment never sees a short-name temporary path.
fn extension_home() -> tempfile::TempDir {
    let builder = {
        let mut builder = tempfile::Builder::new();
        builder.prefix("cua-perception-e2e-home-");
        builder
    };
    #[cfg(target_os = "windows")]
    let home = builder
        .tempdir_in(std::env::var_os("USERPROFILE").expect("USERPROFILE is required on Windows"));
    #[cfg(not(target_os = "windows"))]
    let home = builder.tempdir();
    home.expect("create isolated extension home")
}

// ---------------------------------------------------------------- driver

/// One MCP connection to a Driver whose extension root is `home`.
struct PerceptionDriver {
    driver: McpDriver,
    #[cfg(target_os = "macos")]
    _daemon: MacosDaemon,
}

impl PerceptionDriver {
    fn start(label: &str, home: &Path) -> Self {
        #[cfg(target_os = "macos")]
        {
            let daemon = MacosDaemon::start(home);
            let driver = McpDriver::spawn_daemon_proxy_named(&daemon.socket, label)
                .expect("connect to the dedicated installed Driver daemon");
            Self {
                driver,
                _daemon: daemon,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let home = home.to_str().expect("UTF-8 extension home");
            let driver = McpDriver::spawn_named_with_env(label, &[("CUA_DRIVER_RS_HOME", home)])
                .expect("start the source-built Driver with an isolated extension home");
            Self { driver }
        }
    }
}

#[cfg(target_os = "macos")]
fn installed_macos_binary() -> PathBuf {
    let binary = PathBuf::from(
        std::env::var_os("CUA_E2E_INSTALLED_DRIVER_BIN")
            .expect("CUA_E2E_INSTALLED_DRIVER_BIN must identify the installed, TCC-authorized app"),
    );
    assert!(binary.is_file(), "installed Driver is missing: {binary:?}");
    binary
}

/// A second instance of the installed app, sharing its TCC grants but not its
/// extension home, socket, or PID file. The canonical shared daemon is left
/// untouched; this instance is stopped when the row ends.
#[cfg(target_os = "macos")]
struct MacosDaemon {
    binary: PathBuf,
    socket: String,
    _socket_dir: tempfile::TempDir,
}

#[cfg(target_os = "macos")]
impl MacosDaemon {
    fn start(home: &Path) -> Self {
        let binary = installed_macos_binary();
        let app = binary
            .ancestors()
            .nth(3)
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
            .unwrap_or_else(|| panic!("installed Driver is not inside an app bundle: {binary:?}"))
            .to_path_buf();
        // Unix socket paths are short on macOS.
        let socket_dir = tempfile::Builder::new()
            .prefix("cua-p-")
            .tempdir_in("/tmp")
            .expect("create daemon socket directory");
        let socket = socket_dir.path().join("d.sock").display().to_string();
        let pid_file = socket_dir.path().join("d.pid").display().to_string();
        let log = home.join("daemon.log");
        let status = Command::new("open")
            .args(["-n", "-g"])
            .arg("--env")
            .arg(format!("CUA_DRIVER_RS_HOME={}", home.display()))
            .args(["--env", "CUA_DRIVER_RS_TELEMETRY_ENABLED=false"])
            .arg("--stderr")
            .arg(&log)
            .arg(&app)
            .args(["--args", "--socket", &socket, "--pid-file", &pid_file])
            .args([
                "serve",
                "--permission-mode",
                "unrestricted",
                "--dangerously-bypass-approvals",
                "--no-permissions-gate",
                "--no-overlay",
            ])
            .status()
            .expect("launch dedicated installed Driver instance");
        assert!(status.success(), "open failed for {app:?}: {status}");
        let daemon = Self {
            binary,
            socket,
            _socket_dir: socket_dir,
        };
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let output = Command::new(&daemon.binary)
                .args(["--socket", &daemon.socket, "status"])
                .stdin(Stdio::null())
                .output();
            if output.as_ref().is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .contains("permission mode: unrestricted")
            }) {
                return daemon;
            }
            if Instant::now() >= deadline {
                panic!(
                    "dedicated installed Driver did not become ready: status={:?} log={}",
                    output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()),
                    fs::read_to_string(&log).unwrap_or_default()
                );
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacosDaemon {
    fn drop(&mut self) {
        let _ = Command::new(&self.binary)
            .args(["--socket", &self.socket, "stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ---------------------------------------------------------------- fixture

struct Fixture {
    pid: u32,
    window_id: u64,
    journal: FixtureJournal,
}

fn electron_command() -> Command {
    #[cfg(target_os = "windows")]
    let (path, args) = (
        harness_app("harness-electron", "CuaTestHarness.Electron.exe"),
        vec![
            "--no-sandbox",
            "--disable-gpu",
            "--force-renderer-accessibility",
        ],
    );
    #[cfg(target_os = "macos")]
    let (path, args) = (
        harness_app(
            "harness-electron",
            "CuaTestHarness.Electron.app/Contents/MacOS/Electron",
        ),
        vec!["--force-renderer-accessibility"],
    );
    #[cfg(target_os = "linux")]
    let (path, args) = (
        harness_app("harness-electron", "CuaTestHarness.Electron"),
        vec![
            "--no-sandbox",
            "--disable-gpu",
            "--force-renderer-accessibility",
        ],
    );
    assert!(
        path.exists(),
        "the Electron harness is required but was not staged at {path:?}"
    );
    let mut command = Command::new(path);
    command
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}

fn launch_electron(driver: &mut McpDriver) -> Fixture {
    let journal = FixtureJournal::start();
    let cdp_port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("allocate a loopback CDP port")
        .port();
    let window_ids = |driver: &mut McpDriver| {
        driver.call("list_windows", json!({})).structured()["windows"]
            .as_array()
            .map(|windows| {
                windows
                    .iter()
                    .filter_map(|window| window["window_id"].as_u64())
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default()
    };
    let before = window_ids(driver);
    let mut command = electron_command();
    command
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string());
    let child = spawn_in_job(&mut command).expect("launch the Electron harness");
    driver.reaper().push(child);

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let windows = driver.call("list_windows", json!({}));
        let found = windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| {
                window["window_id"]
                    .as_u64()
                    .is_some_and(|id| !before.contains(&id))
                    && window["title"]
                        .as_str()
                        .is_some_and(|title| title.contains(FIXTURE_TITLE))
            })
            .and_then(|window| {
                Some((
                    window["pid"].as_u64()? as u32,
                    window["window_id"].as_u64()?,
                ))
            });
        if let Some((pid, window_id)) = found {
            driver.reaper().track_pid(pid);
            // The page publishes its DOM state only after it has loaded.
            while Instant::now() < deadline {
                if journal.text("drag-status").as_deref() == Some("drag_status=idle") {
                    return Fixture {
                        pid,
                        window_id,
                        journal,
                    };
                }
                thread::sleep(Duration::from_millis(100));
            }
            panic!("Electron harness page never published its initial state");
        }
        thread::sleep(Duration::from_millis(250));
    }
    panic!("Electron harness window did not appear");
}

fn capture_window(driver: &mut McpDriver, fixture: &Fixture) -> (ToolResponse, String) {
    let state = driver.call(
        "get_window_state",
        json!({"pid": fixture.pid as i64, "window_id": fixture.window_id}),
    );
    assert!(
        !state.is_error(),
        "get_window_state failed: {}",
        state.text()
    );
    let capture_id = state.structured()["capture_id"]
        .as_str()
        .unwrap_or_else(|| panic!("window observation omitted capture_id: {}", state.text()))
        .to_owned();
    (state, capture_id)
}

fn parse_regions(driver: &mut McpDriver, capture_id: &str) -> ToolResponse {
    driver.call(
        "parse_visual_regions",
        json!({
            "capture_id": capture_id,
            "options": {"kinds": ["icon"], "min_confidence": 0.5, "max_regions": 16}
        }),
    )
}

/// Validate one parse result against the retained capture and return the
/// target swatch's screenshot-pixel center.
fn target_point(parsed: &ToolResponse, fixture: &Fixture, capture_id: &str) -> (f64, f64) {
    assert!(
        !parsed.is_error(),
        "parse_visual_regions failed: {}; structured={}",
        parsed.text(),
        parsed.structured()
    );
    let result = parsed.structured();
    assert_eq!(result["schema"], "cua.visual_regions_v1");
    assert_eq!(result["capture"]["capture_id"], capture_id);
    assert_eq!(result["capture"]["source"]["kind"], "window");
    assert_eq!(result["capture"]["source"]["pid"], fixture.pid);
    assert_eq!(result["capture"]["source"]["window_id"], fixture.window_id);
    assert_eq!(result["parser"]["extension_version"], EXTENSION_VERSION);
    assert_eq!(result["parser"]["backend"], "deterministic_fixture");
    let width = result["capture"]["screenshot"]["width"]
        .as_u64()
        .expect("screenshot width");
    let height = result["capture"]["screenshot"]["height"]
        .as_u64()
        .expect("screenshot height");
    let region = result["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|region| region["id"] == TARGET_REGION)
        .unwrap_or_else(|| panic!("the swatch worker found no {TARGET_REGION}: {result}"));
    let bounds = &region["bounds"];
    let (x, y, w, h) = (
        bounds["x"].as_u64().unwrap(),
        bounds["y"].as_u64().unwrap(),
        bounds["width"].as_u64().unwrap(),
        bounds["height"].as_u64().unwrap(),
    );
    assert!(
        w > 0 && h > 0 && x + w <= width && y + h <= height,
        "region lies outside its {width}x{height} capture: {region}"
    );
    eprintln!(
        "[perception] capture={capture_id} {width}x{height} {TARGET_REGION}=({x},{y}) {w}x{h} space={}",
        result["capture"]["action_coordinate_space"]
    );
    (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0)
}

fn wait_for_drag_status_change(journal: &FixtureJournal) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = journal.text("drag-status").unwrap_or_default();
        if status != "drag_status=idle" && !status.is_empty() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "the capture-bound click never reached the parsed swatch: {}",
            journal.snapshot()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------- rows

/// A default installation has no extension. Parsing a real, retained window
/// capture returns the typed `not_installed` refusal and never installs,
/// downloads, or launches a worker.
#[test]
#[ignore]
fn parse_visual_regions_without_extension_returns_not_installed() {
    let case = native_readonly_case(
        "electron",
        "parse_visual_regions_not_installed",
        Targeting::NotApplicable,
        DriverRoute::WindowState,
        vec![OracleKind::Protocol],
    );
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        let home = extension_home();
        let mut session = PerceptionDriver::start(&cell_id, home.path());
        let driver = &mut session.driver;
        *evidence = recording_evidence(driver.recording_dir());
        let fixture = launch_electron(driver);
        driver.start_behavior_recording();

        let (_, capture_id) = capture_window(driver, &fixture);
        let parsed = parse_regions(driver, &capture_id);
        assert!(parsed.is_error(), "parse without an extension succeeded");
        assert_eq!(
            parsed.structured()["code"],
            "not_installed",
            "unexpected refusal: {}",
            parsed.structured()
        );
        assert!(
            !home.path().join("extensions").exists(),
            "parsing must never install the extension"
        );
        Observation::delivered(vec![OracleKind::Protocol], Evidence::default())
    });
}

/// With the extension installed: parse a fresh window capture, click the
/// parsed swatch once with the same `capture_id`, verify the fixture state,
/// prove the consumed capture is refused, then reobserve and reparse.
#[test]
#[ignore]
fn capture_bound_click_from_parsed_region_is_state_verified_and_single_use() {
    let route = shared_web_route(
        Platform::current(),
        DisplayServer::current(),
        "left_click",
        Targeting::Px,
        Delivery::Foreground,
    )
    .expect("foreground pixel click route");
    let case = CaseSpec::delivered(
        format!(
            "{}-electron-perception-capture-click-px-foreground",
            std::env::consts::OS
        ),
        "electron",
        "electron",
        "perception_capture_click",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Window,
        route,
        vec![OracleKind::FixtureState, OracleKind::Protocol],
    );
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        let home = extension_home();
        let scratch = tempfile::tempdir().expect("create extension scratch directory");
        install_swatch_extension(&lifecycle_binary(), home.path(), scratch.path());

        let mut session = PerceptionDriver::start(&cell_id, home.path());
        let driver = &mut session.driver;
        *evidence = recording_evidence(driver.recording_dir());
        let fixture = launch_electron(driver);
        let raised = driver.call(
            "bring_to_front",
            json!({"pid": fixture.pid as i64, "window_id": fixture.window_id}),
        );
        eprintln!("[perception] bring_to_front: {}", raised.text());
        thread::sleep(Duration::from_millis(500));
        driver.start_behavior_recording();

        // 1. Observe and retain the native capture.
        let (_, capture_id) = capture_window(driver, &fixture);
        // 2. Parse that exact capture.
        let parsed = parse_regions(driver, &capture_id);
        let (x, y) = target_point(&parsed, &fixture, &capture_id);
        assert_eq!(
            fixture.journal.text("drag-status").as_deref(),
            Some("drag_status=idle"),
            "parsing must not change fixture state"
        );

        // 3. One capture-bound click derived from the parsed region.
        let click_args = json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.window_id,
            "x": x,
            "y": y,
            "capture_id": capture_id,
            "delivery_mode": "foreground"
        });
        let click = driver.call("click", click_args.clone());
        assert!(
            !click.is_error(),
            "capture-bound click failed: {}; structured={}",
            click.text(),
            click.structured()
        );
        let delivered = wait_for_drag_status_change(&fixture.journal);
        eprintln!(
            "[perception] click route={:?} effect={:?} fixture={delivered}",
            click.action_route(),
            click.action_effect()
        );

        // 4. The capture is single-use: the same capture_id is refused.
        let reused = driver.call("click", click_args);
        assert!(
            reused.is_error(),
            "a consumed capture authorized a second click: {}",
            reused.text()
        );
        assert_eq!(
            reused.structured()["code"],
            "capture_not_found",
            "consumed capture used an unexpected refusal: {}",
            reused.structured()
        );
        assert_eq!(reused.structured()["effect"], "refused");

        // 5. Reobserve: a fresh capture parses under its own identity.
        let (_, fresh_capture_id) = capture_window(driver, &fixture);
        assert_ne!(fresh_capture_id, capture_id);
        let reparsed = parse_regions(driver, &fresh_capture_id);
        target_point(&reparsed, &fixture, &fresh_capture_id);
        Observation::delivered(
            vec![OracleKind::FixtureState, OracleKind::Protocol],
            Evidence::default(),
        )
    });
}

// ---------------------------------------------------------------- published

/// The signed release the published-catalog row installs. The Driver verifies
/// the catalog signature and archive digest itself; these pins also stop the
/// row from silently certifying a replaced release asset.
const PUBLISHED_VERSION: &str = "0.2.1";
/// `(target, catalog SHA-256, archive SHA-256)` from the release `SHA256SUMS`.
const PUBLISHED_ASSETS: &[(&str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "d20c1e1cbf5d90cfa85b956100846c2c7a43c387fdc5b344d75a15f91498aa42",
        "5fbcf59c15fc5cd8beca6ac4aa5533c34359054810aceb12c9e26d7a45e750ab",
    ),
    (
        "x86_64-pc-windows-msvc",
        "bfb2300ff9da054d522e73351f7b337ba6157ebbe30f029a95a471fc3d13a042",
        "6709579aa3f938e8200c0a51b1abe450d4fda51b09e97517e0cb4c272b5bff37",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "853759659a7a9d77aa246bc6f083c6e939a9ce92899f1acd55c80e9345e1070b",
        "b4d76b626df9ce1a48b8036f7313412525295d0cb818d82ecbe3d9cca5fd1d08",
    ),
];
/// Driver's capture lifetime (`CaptureRegistryConfig::default().ttl`).
const CAPTURE_TTL: Duration = Duration::from_secs(60);
/// The canvas card the row clicks. "Save" is read unreliably by OCR on 1x
/// macOS displays; "Cancel" reads on every certified desktop.
const CANVAS_TARGET_LABEL: &str = "Cancel";
const CANVAS_TARGET_ID: &str = "cancel";

fn file_sha256(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// Download one release asset unless a copy with the pinned digest is cached.
fn fetch_release_asset(directory: &Path, name: &str, expected_sha256: &str) {
    let path = directory.join(name);
    if file_sha256(&path).as_deref() == Some(expected_sha256) {
        eprintln!("[perception-published] cached {name}");
        return;
    }
    let url = format!(
        "https://github.com/trycua/cua/releases/download/cua-perception-v{PUBLISHED_VERSION}/{name}"
    );
    let partial = directory.join(format!("{name}.partial"));
    let started = Instant::now();
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "4",
            "--retry-all-errors",
            "--output",
        ])
        .arg(&partial)
        .arg(&url)
        .stdin(Stdio::null())
        .status()
        .expect("run curl to download the published cua-perception release");
    assert!(status.success(), "downloading {url} failed: {status}");
    let actual = file_sha256(&partial).expect("hash downloaded release asset");
    assert_eq!(
        actual, expected_sha256,
        "{name} from the cua-perception-v{PUBLISHED_VERSION} release does not match its pinned SHA-256"
    );
    fs::rename(&partial, &path).expect("move verified release asset into place");
    eprintln!(
        "[perception-published] downloaded {name} in {:.1}s",
        started.elapsed().as_secs_f64()
    );
}

/// Directory holding this target's published catalog next to its archive, the
/// layout `extension install --catalog` expects. `CUA_E2E_PERCEPTION_CACHE_DIR`
/// lets a runner keep the ~425 MB archive between rows.
fn published_catalog() -> PathBuf {
    let target = current_target();
    let (_, catalog_sha256, archive_sha256) = PUBLISHED_ASSETS
        .iter()
        .find(|(candidate, _, _)| *candidate == target)
        .unwrap_or_else(|| {
            panic!("cua-perception-v{PUBLISHED_VERSION} publishes no archive for {target}")
        });
    let directory = std::env::var_os("CUA_E2E_PERCEPTION_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("cua-perception-release-{PUBLISHED_VERSION}"))
        });
    fs::create_dir_all(&directory).expect("create published release cache");
    let stem = format!("cua-perception-{PUBLISHED_VERSION}-{target}");
    fetch_release_asset(&directory, &format!("{stem}.tar.gz"), archive_sha256);
    let catalog = format!("{stem}.catalog.json");
    fetch_release_asset(&directory, &catalog, catalog_sha256);
    directory.join(catalog)
}

/// Install the published extension through the documented user path and
/// require that Driver reports it as publisher-verified and healthy.
fn install_published_extension(binary: &Path, home: &Path) -> Value {
    let catalog = published_catalog();
    let catalog = catalog.to_str().expect("UTF-8 catalog path");
    let install = run_extension_command(
        binary,
        home,
        &[
            "extension",
            "install",
            "cua-perception",
            "--catalog",
            catalog,
        ],
    );
    assert!(
        install.status.success(),
        "published extension install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    let status = run_extension_command(
        binary,
        home,
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
        "extension status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("extension status JSON");
    assert_eq!(status["installed"], true, "extension status: {status}");
    assert_eq!(status["healthy"], true, "extension status: {status}");
    assert_eq!(
        status["trust"], "publisher-verified",
        "extension status: {status}"
    );
    assert_eq!(
        status["active_version"], PUBLISHED_VERSION,
        "extension status: {status}"
    );
    status
}

fn canvas_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/apps/cross-platform/visual-only-canvas/main.py")
}

/// The first Python that can import Tk. `CUA_E2E_TK_PYTHON` overrides the
/// search; otherwise Windows tries the `py` launcher and then `python`.
fn tk_python() -> Vec<String> {
    let mut candidates: Vec<Vec<String>> = Vec::new();
    if let Some(python) = std::env::var_os("CUA_E2E_TK_PYTHON") {
        candidates.push(vec![python.to_string_lossy().into_owned()]);
    } else if cfg!(windows) {
        candidates.push(vec!["py".into(), "-3".into()]);
        candidates.push(vec!["python".into()]);
    } else {
        candidates.push(vec!["python3".into()]);
    }
    let mut failures = Vec::new();
    for candidate in &candidates {
        let probe = Command::new(&candidate[0])
            .args(&candidate[1..])
            .args(["-c", "import tkinter; print(tkinter.TkVersion)"])
            .stdin(Stdio::null())
            .output();
        match probe {
            Ok(output) if output.status.success() => {
                eprintln!(
                    "[perception-published] Tk {} from {candidate:?}",
                    String::from_utf8_lossy(&output.stdout).trim()
                );
                return candidate.clone();
            }
            Ok(output) => failures.push(format!(
                "{candidate:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => failures.push(format!("{candidate:?}: {error}")),
        }
    }
    panic!(
        "the published-catalog row needs Python with Tk for the visual-only canvas fixture \
         (provision it or set CUA_E2E_TK_PYTHON): {failures:?}"
    );
}

struct CanvasFixture {
    pid: u32,
    window_id: u64,
    journal: FixtureJournal,
}

fn launch_canvas(driver: &mut McpDriver, label: &str) -> CanvasFixture {
    let python = tk_python();
    let journal = FixtureJournal::start();
    let title = format!("Cua Visual-Only Canvas [{label}]");
    let mut command = Command::new(&python[0]);
    command
        .args(&python[1..])
        .arg(canvas_fixture_path())
        .args(["--journal-url", journal.url(), "--title", &title])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = spawn_in_job(&mut command).expect("launch the visual-only canvas fixture");
    driver.reaper().push(child);
    let deadline = Instant::now() + Duration::from_secs(30);
    while journal.snapshot()["ready"].as_bool() != Some(true) {
        assert!(
            Instant::now() < deadline,
            "the canvas fixture never published readiness"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let state = journal.snapshot();
    let pid = state["pid"]
        .as_u64()
        .filter(|pid| *pid > 0)
        .unwrap_or_else(|| panic!("canvas fixture journal has no pid: {state}"))
        as u32;
    driver.reaper().track_pid(pid);
    assert!(
        state["selected"].is_null() && state["action_count"] == 0,
        "canvas fixture did not start idle: {state}"
    );
    let (window_id, _) = driver
        .find_window(pid as i64, &title)
        .unwrap_or_else(|| panic!("the exact canvas fixture window for pid {pid} did not appear"));
    CanvasFixture {
        pid,
        window_id,
        journal,
    }
}

fn capture_canvas(driver: &mut McpDriver, fixture: &CanvasFixture) -> (String, Instant) {
    let state = driver.call(
        "get_window_state",
        json!({"pid": fixture.pid as i64, "window_id": fixture.window_id}),
    );
    let captured_at = Instant::now();
    assert!(
        !state.is_error(),
        "get_window_state failed: {}",
        state.text()
    );
    let capture_id = state.structured()["capture_id"]
        .as_str()
        .unwrap_or_else(|| panic!("window observation omitted capture_id: {}", state.text()))
        .to_owned();
    (capture_id, captured_at)
}

fn parse_text_regions(driver: &mut McpDriver, capture_id: &str) -> ToolResponse {
    driver.call(
        "parse_visual_regions",
        json!({
            "capture_id": capture_id,
            "options": {"kinds": ["text"], "min_confidence": 0.3, "max_regions": 100}
        }),
    )
}

/// Validate a model-backed parse against its capture and the published parser.
fn validated_published_parse(
    parsed: &ToolResponse,
    fixture: &CanvasFixture,
    capture_id: &str,
) -> Value {
    assert!(
        !parsed.is_error(),
        "parse_visual_regions failed: {}; structured={}",
        parsed.text(),
        parsed.structured()
    );
    let result = parsed.structured().clone();
    assert_eq!(result["schema"], "cua.visual_regions_v1");
    assert_eq!(result["capture"]["capture_id"], capture_id);
    assert_eq!(result["capture"]["source"]["kind"], "window");
    assert_eq!(result["capture"]["source"]["pid"], fixture.pid);
    assert_eq!(result["capture"]["source"]["window_id"], fixture.window_id);
    assert_eq!(result["parser"]["extension_id"], "cua-perception");
    assert_eq!(result["parser"]["extension_version"], PUBLISHED_VERSION);
    assert_ne!(
        result["parser"]["backend"], "deterministic_fixture",
        "the published row must run the released model backend: {}",
        result["parser"]
    );
    assert_eq!(
        result["capture"]["action_coordinate_space"]["kind"], "screenshot_pixels",
        "region centers are clicked in screenshot pixels: {}",
        result["capture"]
    );
    result
}

fn normalized_label(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Text regions whose OCR reads exactly the target label.
fn label_regions<'a>(parse: &'a Value, label: &str) -> Vec<&'a Value> {
    let wanted = normalized_label(label);
    parse["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|region| region["kind"] == "text")
        .filter(|region| {
            region["text"]
                .as_str()
                .is_some_and(|text| normalized_label(text) == wanted)
        })
        .collect()
}

fn region_texts(parse: &Value) -> Vec<String> {
    parse["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|region| region["text"].as_str().map(str::to_owned))
        .collect()
}

/// Screenshot-pixel center of a region that must lie inside its capture.
fn region_center(parse: &Value, region: &Value) -> (f64, f64) {
    let width = parse["capture"]["screenshot"]["width"]
        .as_f64()
        .expect("screenshot width");
    let height = parse["capture"]["screenshot"]["height"]
        .as_f64()
        .expect("screenshot height");
    let bounds = &region["bounds"];
    let value = |key: &str| bounds[key].as_f64().expect("numeric region bounds");
    let (x, y, w, h) = (value("x"), value("y"), value("width"), value("height"));
    assert!(
        w > 0.0 && h > 0.0 && x >= 0.0 && y >= 0.0 && x + w <= width && y + h <= height,
        "region lies outside its {width}x{height} capture: {region}"
    );
    (x + w / 2.0, y + h / 2.0)
}

fn canvas_click_args(fixture: &CanvasFixture, x: f64, y: f64, capture_id: &str) -> Value {
    json!({
        "pid": fixture.pid as i64,
        "window_id": fixture.window_id,
        "x": x,
        "y": y,
        "capture_id": capture_id,
        "delivery_mode": "foreground"
    })
}

fn assert_capture_refused(response: &ToolResponse, code: &str, what: &str) {
    assert!(
        response.is_error(),
        "{what} authorized a click: {}",
        response.text()
    );
    assert_eq!(
        response.structured()["code"],
        code,
        "{what} used an unexpected refusal: {}",
        response.structured()
    );
    assert_eq!(response.structured()["effect"], "refused", "{what}");
}

fn wait_for_canvas_selection(journal: &FixtureJournal) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = journal.snapshot();
        if state["selected"] == CANVAS_TARGET_ID && state["action_count"] == 1 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn assert_canvas_selected_once(journal: &FixtureJournal, after: &str) {
    thread::sleep(Duration::from_millis(750));
    let state = journal.snapshot();
    assert!(
        state["selected"] == CANVAS_TARGET_ID && state["action_count"] == 1,
        "{after} changed the fixture: {state}"
    );
}

fn write_published_evidence(cell_id: &str, value: &Value) {
    let Some(root) = std::env::var_os("CUA_E2E_RESULTS_FILE")
        .map(PathBuf::from)
        .and_then(|results| results.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let directory = root.join("perception-published");
    fs::create_dir_all(&directory).expect("create published-catalog evidence directory");
    let path = directory.join(format!("{cell_id}.json"));
    fs::write(&path, serde_json::to_vec_pretty(value).unwrap())
        .unwrap_or_else(|error| panic!("write {path:?}: {error}"));
}

/// The released extension, installed from its signed catalog, parses a live
/// capture of a custom-painted surface with no accessibility tree. One
/// capture-bound click on the OCR'd "Cancel" label selects that card in the
/// fixture's own journal. The consumed capture and an unused expired capture
/// are refused without effect, and a fresh capture reparses.
#[test]
#[ignore]
fn published_extension_parses_canvas_and_capture_bound_click_is_state_verified() {
    let route = shared_web_route(
        Platform::current(),
        DisplayServer::current(),
        "left_click",
        Targeting::Px,
        Delivery::Foreground,
    )
    .expect("foreground pixel click route");
    let case = CaseSpec::delivered(
        format!(
            "{}-tk-perception-published-capture-click-px-foreground",
            std::env::consts::OS
        ),
        "visual-only-canvas",
        "tk",
        "perception_published_capture_click",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Window,
        route,
        vec![OracleKind::FixtureState, OracleKind::Protocol],
    );
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        let home = extension_home();
        let status = install_published_extension(&lifecycle_binary(), home.path());

        let mut session = PerceptionDriver::start(&cell_id, home.path());
        let driver = &mut session.driver;
        *evidence = recording_evidence(driver.recording_dir());
        let fixture = launch_canvas(driver, &cell_id);
        let raised = driver.call(
            "bring_to_front",
            json!({"pid": fixture.pid as i64, "window_id": fixture.window_id}),
        );
        eprintln!("[perception-published] bring_to_front: {}", raised.text());
        thread::sleep(Duration::from_millis(500));

        // Start the worker on a capture that is never clicked, so it can
        // prove expiry at the end of the row.
        let (expiring_capture, expiring_at) = capture_canvas(driver, &fixture);
        let warm_started = Instant::now();
        let warm = parse_text_regions(driver, &expiring_capture);
        validated_published_parse(&warm, &fixture, &expiring_capture);
        let warm_parse_ms = warm_started.elapsed().as_millis() as u64;
        driver.start_behavior_recording();

        // 1. Observe and retain the native capture.
        let (capture_id, captured_at) = capture_canvas(driver, &fixture);
        // 2. The released model parses that exact capture.
        let parse_started = Instant::now();
        let parsed = parse_text_regions(driver, &capture_id);
        let parse_ms = parse_started.elapsed().as_millis() as u64;
        let parse = validated_published_parse(&parsed, &fixture, &capture_id);
        let matches = label_regions(&parse, CANVAS_TARGET_LABEL);
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one OCR region reading {CANVAS_TARGET_LABEL:?}; read {:?}",
            region_texts(&parse)
        );
        let region = matches[0].clone();
        let (x, y) = region_center(&parse, &region);
        let state = fixture.journal.snapshot();
        assert!(
            state["selected"].is_null() && state["action_count"] == 0,
            "parsing must not change fixture state: {state}"
        );

        // 3. One capture-bound click derived from the parsed region.
        let click_age = captured_at.elapsed();
        assert!(
            click_age < CAPTURE_TTL,
            "the loop took {click_age:?}, beyond the {CAPTURE_TTL:?} capture lifetime"
        );
        let args = canvas_click_args(&fixture, x, y, &capture_id);
        let click = driver.call("click", args.clone());
        assert!(
            !click.is_error(),
            "capture-bound click failed: {}; structured={}",
            click.text(),
            click.structured()
        );
        // 4. The fixture's own journal, which Driver never reads.
        let delivered = wait_for_canvas_selection(&fixture.journal);
        let summary = json!({
            "cell_id": cell_id,
            "extension": {
                "version": status["active_version"],
                "trust": status["trust"],
                "publisher_key_id": status["publisher_key_id"],
                "catalog_version": status["catalog_version"],
            },
            "parser": parse["parser"],
            "screenshot": parse["capture"]["screenshot"],
            "regions_parsed": parse["regions"].as_array().map(Vec::len),
            "texts": region_texts(&parse),
            "target_region": region,
            "click_point": {"x": x, "y": y},
            "warm_parse_ms": warm_parse_ms,
            "parse_ms": parse_ms,
            "capture_age_at_click_ms": click_age.as_millis() as u64,
            "click_route": click.action_route(),
            "click_effect": click.action_effect(),
            "fixture_state": fixture.journal.snapshot(),
        });
        write_published_evidence(&cell_id, &summary);
        eprintln!("[perception-published] {summary}");
        assert!(
            delivered,
            "the click on OCR region {CANVAS_TARGET_LABEL:?} at ({x:.0},{y:.0}) never selected \
             the card: {}",
            fixture.journal.snapshot()
        );

        // 5. The consumed capture is refused.
        let reused = driver.call("click", args);
        assert_capture_refused(&reused, "capture_not_found", "a consumed capture");
        assert_canvas_selected_once(&fixture.journal, "a refused consumed capture");

        // 6. Reobserve: a fresh capture parses under its own identity and
        // still shows the target.
        let (fresh_capture_id, _) = capture_canvas(driver, &fixture);
        assert_ne!(fresh_capture_id, capture_id);
        let reparsed = parse_text_regions(driver, &fresh_capture_id);
        let reparse = validated_published_parse(&reparsed, &fixture, &fresh_capture_id);
        assert!(
            !label_regions(&reparse, CANVAS_TARGET_LABEL).is_empty(),
            "the reobserved canvas no longer reads {CANVAS_TARGET_LABEL:?}: {:?}",
            region_texts(&reparse)
        );
        eprintln!(
            "[perception-published] reobserved {fresh_capture_id}: {:?}",
            region_texts(&reparse)
        );

        // 7. An unused capture past its lifetime is refused.
        let remaining = CAPTURE_TTL.saturating_sub(expiring_at.elapsed());
        thread::sleep(remaining + Duration::from_secs(2));
        let expired = driver.call(
            "click",
            canvas_click_args(&fixture, x, y, &expiring_capture),
        );
        assert_capture_refused(&expired, "capture_expired", "an expired capture");
        assert_canvas_selected_once(&fixture.journal, "a refused expired capture");
        Observation::delivered(
            vec![OracleKind::FixtureState, OracleKind::Protocol],
            Evidence::default(),
        )
    });
}

// ---------------------------------------------------------------- hermetic

fn png_bytes(image: image::RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

fn worker_exchange(worker: &Path, png: &[u8]) -> Vec<Value> {
    use base64::Engine as _;
    use std::io::Write as _;
    let frame = |value: Value| {
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
        framed.extend(bytes);
        framed
    };
    let mut input = frame(json!({
        "protocol": "cua-perception/1", "request_id": "health-1", "method": "health", "params": {}
    }));
    input.extend(frame(json!({
        "protocol": "cua-perception/1", "request_id": "parse-1", "method": "parse",
        "params": {"capture_id": "capture_e2e_1", "image": {
            "media_type": "image/png", "width": 1, "height": 1, "byte_length": png.len(),
            "data_base64": base64::engine::general_purpose::STANDARD.encode(png)
        }}
    })));
    let mut child = Command::new(worker)
        .args([
            "--extension-id",
            "cua-perception",
            "--extension-version",
            EXTENSION_VERSION,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run swatch worker");
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let mut frames = Vec::new();
    let mut rest = output.stdout.as_slice();
    while rest.len() >= 4 {
        let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        frames.push(serde_json::from_slice(&rest[4..4 + length]).unwrap());
        rest = &rest[4 + length..];
    }
    frames
}

/// The swatch worker's PNG decoder and detector locate the harness swatches in
/// pixels, ignore thin same-color decoys, and echo the exact PNG digest.
#[test]
fn swatch_worker_detects_harness_swatches_from_png_pixels() {
    let scratch = tempfile::tempdir().unwrap();
    let worker = compile_worker(scratch.path());
    let image = image::RgbaImage::from_fn(640, 400, |x, y| {
        let pixel = if (100..320).contains(&x) && (50..146).contains(&y) {
            [0x12, 0x68, 0xd6]
        } else if (340..560).contains(&x) && (50..146).contains(&y) {
            [0x17, 0x8a, 0x38]
        } else if y == 200 || x == 20 {
            // A focus-ring-like outline in a near color must not be chosen.
            [0x10, 0x6a, 0xe0]
        } else if y > 300 {
            [
                (x * 7 % 256) as u8,
                (y * 3 % 256) as u8,
                ((x + y) * 5 % 256) as u8,
            ]
        } else {
            [255, 255, 255]
        };
        image::Rgba([pixel[0], pixel[1], pixel[2], 255])
    });
    let png = png_bytes(image);
    let frames = worker_exchange(&worker, &png);
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(frames[0]["request_id"], "health-1");
    assert_eq!(frames[0]["result"]["ready"], true);
    assert_eq!(
        frames[0]["result"]["identity"]["extension"]["version"],
        EXTENSION_VERSION
    );
    let result = &frames[1]["result"];
    assert_eq!(frames[1]["status"], "ok", "{}", frames[1]);
    assert_eq!(result["capture_id"], "capture_e2e_1");
    assert_eq!(result["image"]["sha256"], sha256(&png));
    assert_eq!(result["image"]["width"], 640);
    assert_eq!(result["image"]["height"], 400);
    assert_eq!(
        result["regions"],
        json!([
            {"id": "swatch-drag-source", "kind": "icon",
             "bounds": {"x": 100, "y": 50, "width": 220, "height": 96},
             "label": "drag-source swatch", "confidence": 0.99, "interactive": true, "reading_order": 0},
            {"id": "swatch-drop-target", "kind": "icon",
             "bounds": {"x": 340, "y": 50, "width": 220, "height": 96},
             "label": "drop-target swatch", "confidence": 0.99, "interactive": true, "reading_order": 1}
        ])
    );

    let blank = png_bytes(image::RgbaImage::from_pixel(32, 32, image::Rgba([255; 4])));
    let frames = worker_exchange(&worker, &blank);
    assert_eq!(frames[1]["status"], "error");
    assert_eq!(frames[1]["error"]["code"], "inference_failed");
}

/// Driver accepts the developer-only swatch archive and runs the installed
/// worker inside its normal containment. Uses the read-only local-image CLI,
/// so it needs no desktop; skipped when the Driver binary is not built.
#[test]
fn swatch_extension_installs_and_parses_through_driver_containment() {
    let binary = driver_binary();
    if !ensure_driver_binary(&binary) {
        return;
    }
    let home = extension_home();
    let scratch = tempfile::tempdir().unwrap();
    install_swatch_extension(&binary, home.path(), scratch.path());

    let image = image::RgbaImage::from_fn(300, 200, |x, y| {
        if (40..150).contains(&x) && (60..108).contains(&y) {
            image::Rgba([0x12, 0x68, 0xd6, 255])
        } else {
            image::Rgba([250, 250, 250, 255])
        }
    });
    let image_path = scratch.path().join("window.png");
    fs::write(&image_path, png_bytes(image)).unwrap();
    let capture_path = scratch.path().join("capture.json");
    fs::write(
        &capture_path,
        br#"{"source":{"kind":"window","pid":123,"window_id":456}}"#,
    )
    .unwrap();
    let output = run_extension_command(
        &binary,
        home.path(),
        &[
            "perception",
            "parse",
            "--image",
            image_path.to_str().unwrap(),
            "--capture",
            capture_path.to_str().unwrap(),
            "--json",
        ],
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "perception parse output is not JSON ({error}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(output.status.success(), "perception parse failed: {result}");
    assert_eq!(result["parser"]["backend"], "deterministic_fixture");
    assert_eq!(result["parser"]["extension_version"], EXTENSION_VERSION);
    assert_eq!(result["local_input"]["action_eligible"], false);
    assert_eq!(result["regions"][0]["id"], TARGET_REGION);
    assert_eq!(
        result["regions"][0]["bounds"],
        json!({"x": 40, "y": 60, "width": 110, "height": 48})
    );
}

/// The published row clicks only an OCR region that reads exactly the target
/// label, so the reobserved "SELECTED: CANCEL" status line never matches.
#[test]
fn label_regions_match_only_the_exact_ocr_label() {
    let region = |text: &str, kind: &str| json!({"kind": kind, "text": text, "bounds": {"x": 1, "y": 1, "width": 10, "height": 10}});
    let parse = json!({"regions": [
        region("Save", "text"),
        region(" cancel ", "text"),
        region("SELECTED: CANCEL", "text"),
        region("Cancel", "icon"),
        region("Canceled", "text"),
    ]});
    let matches = label_regions(&parse, CANVAS_TARGET_LABEL);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["text"], " cancel ");
    assert_eq!(normalized_label("Can-cel!"), "cancel");
}

/// Every pinned release asset is a well-formed SHA-256 for a distinct target.
#[test]
fn published_release_pins_are_well_formed() {
    let mut targets = std::collections::HashSet::new();
    for (target, catalog, archive) in PUBLISHED_ASSETS {
        assert!(targets.insert(*target), "duplicate pin for {target}");
        for digest in [catalog, archive] {
            assert_eq!(digest.len(), 64, "{target}: {digest}");
            assert!(digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
    }
}
