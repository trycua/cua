//! Canonical desktop rows for the optional perception loop.
//!
//! The documented loop is `get_window_state` (retain `capture_id`) ->
//! `parse_visual_regions` -> one capture-bound `click` with the same
//! `capture_id` -> reobserve. These rows drive it against the shared web
//! harness in Electron, which every supported desktop runner builds.
//!
//! No published artifact or model is required. The installed extension is a
//! deterministic, developer-only unsigned worker compiled from
//! `support/perception_swatch_worker.rs`. It speaks the real framed worker
//! protocol, runs inside Driver's normal worker containment, receives the exact
//! PNG Driver retained for the capture, and derives regions from those pixels by
//! finding the harness's solid color swatches. Model quality is covered by the
//! cua-perception crate; these rows certify Driver's capture, parse, action
//! binding, and single-use refusal contract on each desktop.
//!
//! The fixture's loopback journal is the delivery oracle, independent of the
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
