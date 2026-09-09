//! Native exact-window video evidence. See the window-recording fixture README.
#![cfg(target_os = "macos")]

use cua_driver_testkit::{Driver, McpDriver};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, sleep},
    time::{Duration, Instant},
};

const RECORDING_CASES: &[(&str, &str)] = &[
    ("stop", "stop"),
    ("repeat_1", "stop"),
    ("repeat_2", "stop"),
    ("minimize", "minimize"),
    ("close", "close"),
    ("resize", "resize"),
    ("disconnect", "disconnect"),
];

struct Fixture {
    child: Child,
    replies: Receiver<Value>,
    identity: Value,
}

impl Fixture {
    fn launch() -> Self {
        let binary = std::env::var_os("CUA_WINDOW_RECORDING_FIXTURE")
            .expect("set CUA_WINDOW_RECORDING_FIXTURE to the source-built AppKit fixture");
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("launch synthetic window fixture");
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut fixture = Self {
            child,
            replies,
            identity: Value::Null,
        };
        fixture.identity = fixture
            .replies
            .recv_timeout(Duration::from_secs(10))
            .expect("fixture ready");
        assert_eq!(
            fixture.identity["pid"].as_u64(),
            Some(u64::from(fixture.child.id())),
            "fixture must identify its own child process"
        );
        fixture
    }

    fn command(&mut self, command: &str) -> Value {
        writeln!(self.child.stdin.as_mut().unwrap(), "{command}").unwrap();
        self.child.stdin.as_mut().unwrap().flush().unwrap();
        let reply = self
            .replies
            .recv_timeout(Duration::from_secs(10))
            .expect("fixture command ack");
        assert_eq!(reply["command"], command, "{reply}");
        reply
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn call(driver: &mut McpDriver, method: &str, args: Value) -> Value {
    let response = driver.call(method, args);
    assert!(
        !response.is_error(),
        "{method}: {} / {}",
        response.text(),
        response.raw
    );
    response.structured().clone()
}

fn wait_finalized(driver: &mut McpDriver) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = call(driver, "get_recording_state", json!({}));
        if state["status"]["active"] == false && state["status"]["finalized"] == true {
            assert!(state["status"]["error"].is_null(), "{state}");
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "native finalization timed out: {state}"
        );
        sleep(Duration::from_millis(100));
    }
}

fn descriptor_count(pid: u64) -> usize {
    let output = Command::new("/usr/sbin/lsof")
        .args(["-n", "-a", "-p", &pid.to_string(), "-Ff"])
        .output()
        .expect("inspect only the attested test daemon's descriptors");
    assert!(output.status.success(), "could not inspect test daemon");
    let count = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix('f')?.parse::<u32>().ok())
        .count();
    assert!(count > 0, "test daemon has no observable descriptors");
    count
}

fn verify_video(output_dir: &Path, identity: &Value, state: &Value) {
    let mut files: Vec<_> = fs::read_dir(output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["recording.mp4", "session.json"],
        "unexpected trajectory/cursor artifacts"
    );
    let session: Value =
        serde_json::from_slice(&fs::read(output_dir.join("session.json")).unwrap()).unwrap();
    assert_eq!(session["status"]["finalized"], true, "{session}");
    assert_eq!(
        session["status"]["termination_reason"],
        state["status"]["termination_reason"]
    );
    let video = output_dir.join("recording.mp4");
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_frames",
            "-of",
            "json",
        ])
        .arg(&video)
        .output()
        .expect("ffprobe must be installed in the authorized test environment");
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let probe: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = probe["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1, "unexpected audio or extra video stream");
    assert_eq!(streams[0]["codec_type"], "video");
    assert_eq!(streams[0]["codec_name"], "h264");
    let frames = probe["frames"].as_array().expect("decoded frame metadata");
    let timestamps: Vec<f64> = frames
        .iter()
        .map(|frame| {
            frame["best_effort_timestamp_time"]
                .as_str()
                .expect("frame timestamp")
                .parse()
                .expect("numeric frame timestamp")
        })
        .collect();
    assert!(timestamps.len() >= 3, "too few native frames");
    assert!(timestamps.iter().all(|timestamp| timestamp.is_finite()));
    assert!(timestamps.windows(2).all(|pair| pair[1] > pair[0]));
    assert!(timestamps.last().unwrap() - timestamps[0] >= 1.5);
    for dimension in ["width", "height"] {
        let pixels = (identity[dimension].as_f64().unwrap() * identity["scale"].as_f64().unwrap())
            .ceil() as u64;
        let expected = pixels.div_ceil(2) * 2;
        assert_eq!(streams[0][dimension].as_u64(), Some(expected));
        assert_eq!(state["info"][dimension].as_u64(), Some(expected));
    }
    // Decode every frame into RGB. A desktop crop or application-wide
    // filter reveals the red sibling; exact-window capture retains the marker.
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(&video)
        .args([
            "-vf",
            "scale=32:24",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .expect("decode native MP4");
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    const FRAME: usize = 32 * 24 * 3;
    assert!(decoded.stdout.len() >= FRAME * 3, "too few decoded samples");
    assert_eq!(decoded.stdout.len() % FRAME, 0);
    assert_eq!(decoded.stdout.len() / FRAME, timestamps.len());
    let distinct: std::collections::HashSet<_> = decoded.stdout.chunks_exact(FRAME).collect();
    assert!(distinct.len() > 1, "recording contains only a frozen frame");
    for frame in decoded.stdout.chunks_exact(FRAME) {
        let green = frame
            .chunks_exact(3)
            .filter(|p| p[1] > 170 && p[0] < 80 && p[2] < 80)
            .count();
        let red = frame
            .chunks_exact(3)
            .filter(|p| p[0] > 170 && p[1] < 80 && p[2] < 80)
            .count();
        assert!(green > 600, "target green field absent: {green}");
        assert_eq!(red, 0, "sibling/desktop pixels leaked into window video");
        let center = &frame[(12 * 32 + 16) * 3..][..3];
        assert!(
            center[2] > 170 && center[0] < 80 && center[1] < 80,
            "blue target marker absent: {center:?}"
        );
    }
}

fn host_run_authorized(vm: Option<&str>, host: Option<&str>) -> Result<bool, &'static str> {
    match (vm, host) {
        (Some("1"), None) => Ok(false),
        (None, Some("1")) => Ok(true),
        _ => Err("select exactly one: disposable VM or explicitly authorized host diagnostics"),
    }
}

#[test]
fn recording_environment_requires_one_explicit_opt_in() {
    assert_eq!(host_run_authorized(Some("1"), None), Ok(false));
    assert_eq!(host_run_authorized(None, Some("1")), Ok(true));
    for (vm, host) in [
        (None, None),
        (Some("1"), Some("1")),
        (Some("0"), None),
        (None, Some("true")),
    ] {
        assert!(host_run_authorized(vm, host).is_err());
    }
}

#[test]
#[ignore = "requires explicitly authorized GUI environment, installed daemon, AppKit fixture and ffmpeg"]
fn exact_window_recording_isolation_and_lifecycle() {
    let host = host_run_authorized(
        std::env::var("CUA_WINDOW_RECORDING_DISPOSABLE_VM")
            .ok()
            .as_deref(),
        std::env::var("CUA_WINDOW_RECORDING_HOST_AUTHORIZED")
            .ok()
            .as_deref(),
    )
    .expect("native recording environment must be explicitly selected");
    assert!(
        std::env::var_os("CUA_E2E_RECORDINGS_ROOT").is_none(),
        "unset legacy trajectory recording root"
    );
    let root = PathBuf::from(
        std::env::var_os("CUA_WINDOW_RECORDING_OUTPUT_ROOT").expect("set fresh evidence root"),
    );
    assert!(root.is_absolute(), "evidence root must be absolute");
    fs::create_dir_all(&root).unwrap();
    for name in ["test-environment.json", "native-diagnostics.json"]
        .into_iter()
        .chain(RECORDING_CASES.iter().map(|(name, _)| *name))
    {
        assert!(
            fs::symlink_metadata(root.join(name))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "use a fresh output root; never overwrite evidence"
        );
    }
    let socket = PathBuf::from(
        std::env::var_os("CUA_E2E_MACOS_DAEMON_SOCKET")
            .expect("select an explicit test daemon socket; never use the released daemon default"),
    );
    assert!(socket.is_absolute(), "daemon socket must be absolute");
    let expected_sha =
        std::env::var("CUA_E2E_SOURCE_SHA").expect("set the exact candidate source SHA");
    assert!(
        expected_sha.len() == 40 && expected_sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "candidate source SHA must be a full commit"
    );
    if host {
        assert_eq!(
            fs::canonicalize(&socket).expect("test daemon socket must exist"),
            fs::canonicalize(&root).unwrap().join("driver.sock"),
            "host diagnostics require a dedicated driver.sock inside the evidence root"
        );
        let binary = std::env::var_os("CUA_TEST_DRIVER_BIN")
            .expect("host diagnostics require the explicitly installed local Driver");
        assert_eq!(
            fs::canonicalize(binary).unwrap(),
            Path::new("/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"),
            "do not connect the host recording test through the released Driver"
        );
    }
    let mut driver =
        McpDriver::spawn_macos_daemon_proxy().expect("installed authorized daemon must be running");
    let config = call(&mut driver, "get_config", json!({}));
    assert_eq!(config["source_sha"].as_str(), Some(expected_sha.as_str()));
    assert_eq!(config["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    let permissions = call(&mut driver, "check_permissions", json!({"prompt": false}));
    assert_eq!(permissions["source"]["attribution"], "driver-daemon");
    let daemon_pid = permissions["source"]["pid"].as_u64().unwrap();
    serde_json::to_writer_pretty(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("test-environment.json"))
            .expect("never overwrite test-environment evidence"),
        &json!({
            "environment": if host { "explicitly-authorized-host" } else { "disposable-vm" },
            "source_sha": expected_sha,
            "version": config["version"],
        }),
    )
    .unwrap();
    let initial = call(&mut driver, "get_recording_state", json!({}));
    assert_ne!(
        initial["enabled"], true,
        "another recording owns the daemon"
    );
    let descriptors_before_warmup = descriptor_count(daemon_pid);
    let mut descriptor_samples = Vec::new();
    let mut native_scales = Vec::new();
    for &(case, ending) in RECORDING_CASES {
        let mut fixture = Fixture::launch();
        native_scales.push(fixture.identity["scale"].clone());
        let output = root.join(case);
        assert!(
            !output.exists(),
            "use a fresh output root; never overwrite evidence"
        );
        let target = json!({"kind": "window", "pid": fixture.identity["pid"], "window_id": fixture.identity["window_id"]});
        let windows = call(
            &mut driver,
            "list_windows",
            json!({"pid": fixture.identity["pid"]}),
        );
        assert!(
            windows["windows"].as_array().unwrap().iter().any(|window| {
                window["window_id"] == fixture.identity["window_id"]
                    && window["is_on_screen"] == true
            }),
            "fixture identity absent from public list_windows: {windows}"
        );
        let before_start = fixture.command("state");
        assert_eq!(before_start["sibling_visible"], true);
        assert_eq!(before_start["sibling_adjacent"], true);
        let args = json!({"output_dir": output, "record_video": true, "target": target});
        if ending == "stop" {
            let mut invalid = args.clone();
            invalid["target"]["pid"] = json!(2147483647);
            assert!(
                driver.call("start_recording", invalid).is_error(),
                "wrong PID accepted"
            );
        }
        let started = call(&mut driver, "start_recording", args.clone());
        assert_eq!(started["status"]["active"], true, "{started}");
        assert_eq!(started["info"]["backend"], "screencapturekit_window");
        assert_eq!(
            fixture.command("state")["ui_state"],
            before_start["ui_state"],
            "{case}: starting capture changed focus, window order, or the real cursor"
        );
        if ending == "stop" {
            assert!(
                driver.call("start_recording", args).is_error(),
                "busy start accepted"
            );
            assert_eq!(
                call(&mut driver, "get_recording_state", json!({}))["status"]["active"],
                true
            );
        }
        let occluded = fixture.command("occlude");
        for field in [
            "sibling_visible",
            "sibling_covers_target",
            "sibling_in_front",
        ] {
            assert_eq!(
                occluded[field], true,
                "fixture did not establish occlusion: {occluded}"
            );
        }
        sleep(Duration::from_secs(1));
        let moved = fixture.command("move");
        assert_eq!(moved["x"].as_f64(), Some(190.0));
        sleep(Duration::from_secs(2));
        assert_eq!(
            call(&mut driver, "get_recording_state", json!({}))["status"]["active"],
            true,
            "movement or occlusion ended capture"
        );
        let before_end = fixture.command("state");
        if ending == "stop" {
            call(&mut driver, "stop_recording", json!({}));
        } else if ending == "disconnect" {
            drop(driver);
            driver =
                McpDriver::spawn_macos_daemon_proxy().expect("reconnect after owner disconnect");
        } else {
            let changed = fixture.command(ending);
            match ending {
                "minimize" => assert_eq!(changed["miniaturized"], true),
                "close" => assert_eq!(changed["visible"], false),
                "resize" => assert_eq!(changed["width"].as_f64(), Some(360.0)),
                _ => unreachable!(),
            }
        }
        let state = wait_finalized(&mut driver);
        if matches!(ending, "stop" | "disconnect") {
            assert_eq!(
                fixture.command("state")["ui_state"],
                before_end["ui_state"],
                "{case}: ending capture changed focus, window order, or the real cursor"
            );
        }
        let reason = state["status"]["termination_reason"]
            .as_str()
            .expect("termination reason");
        match ending {
            "minimize" => assert!(matches!(reason, "window_not_visible" | "window_resized")),
            "close" => assert!(
                matches!(
                    reason,
                    "window_closed" | "window_not_visible" | "window_resized"
                ),
                "unexpected close transition: {reason}"
            ),
            "resize" => assert_eq!(reason, "window_resized"),
            "stop" | "disconnect" => assert_eq!(reason, "stopped"),
            _ => unreachable!(),
        }
        verify_video(&output, &fixture.identity, &state);
        eprintln!("verified native window recording: {case} ({reason})");
        let stopped_again = call(&mut driver, "stop_recording", json!({}));
        assert_eq!(stopped_again["status"]["finalized"], true);
        descriptor_samples.push(descriptor_count(daemon_pid));
    }
    serde_json::to_writer_pretty(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("native-diagnostics.json"))
            .unwrap(),
        &json!({
            "descriptors_before_warmup": descriptors_before_warmup,
            "warmup_case": "stop",
            "descriptor_samples": descriptor_samples,
            "native_scales": native_scales,
        }),
    )
    .unwrap();
    let baseline = descriptor_samples[0];
    assert!(
        descriptor_samples
            .iter()
            .all(|sample| *sample <= baseline + 2),
        "descriptor growth after recorder warmup: {descriptor_samples:?}"
    );
}
