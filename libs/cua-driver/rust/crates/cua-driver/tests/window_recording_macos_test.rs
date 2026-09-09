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
        let identity = replies
            .recv_timeout(Duration::from_secs(10))
            .expect("fixture ready");
        Self {
            child,
            replies,
            identity,
        }
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
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(&video)
        .output()
        .expect("ffprobe must be installed in the disposable VM");
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let probe: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = probe["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1, "unexpected audio or extra video stream");
    assert_eq!(streams[0]["codec_type"], "video");
    for dimension in ["width", "height"] {
        let expected =
            (identity[dimension].as_f64().unwrap() * identity["scale"].as_f64().unwrap()) as u64;
        assert_eq!(streams[0][dimension].as_u64(), Some(expected));
        assert_eq!(state["info"][dimension].as_u64(), Some(expected));
    }
    // Decode every half-second into RGB. A desktop crop or application-wide
    // filter reveals the red sibling; exact-window capture retains the marker.
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&video)
        .args([
            "-vf",
            "fps=2,scale=32:24",
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

#[test]
#[ignore = "requires disposable Lume GUI VM, authorized installed daemon, AppKit fixture and ffmpeg"]
fn exact_window_recording_isolation_and_lifecycle() {
    assert_eq!(
        std::env::var("CUA_WINDOW_RECORDING_DISPOSABLE_VM").as_deref(),
        Ok("1"),
        "VM-only test"
    );
    assert!(
        std::env::var_os("CUA_E2E_RECORDINGS_ROOT").is_none(),
        "unset legacy trajectory recording root"
    );
    let root = PathBuf::from(
        std::env::var_os("CUA_WINDOW_RECORDING_OUTPUT_ROOT").expect("set fresh evidence root"),
    );
    fs::create_dir_all(&root).unwrap();
    let mut driver =
        McpDriver::spawn_macos_daemon_proxy().expect("installed authorized daemon must be running");
    let initial = call(&mut driver, "get_recording_state", json!({}));
    assert_ne!(
        initial["enabled"], true,
        "another recording owns the daemon"
    );
    for ending in ["stop", "minimize", "close", "resize", "disconnect"] {
        let mut fixture = Fixture::launch();
        let output = root.join(ending);
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
        let reason = state["status"]["termination_reason"]
            .as_str()
            .expect("termination reason");
        match ending {
            "minimize" => assert_eq!(reason, "window_not_visible"),
            "close" => assert!(matches!(reason, "window_closed" | "window_not_visible")),
            "resize" => assert_eq!(reason, "window_resized"),
            "stop" | "disconnect" => assert_eq!(reason, "stopped"),
            _ => unreachable!(),
        }
        verify_video(&output, &fixture.identity, &state);
        let stopped_again = call(&mut driver, "stop_recording", json!({}));
        assert_eq!(stopped_again["status"]["finalized"], true);
    }
}
