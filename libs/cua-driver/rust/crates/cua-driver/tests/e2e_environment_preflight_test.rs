//! One strict GUI/recording preflight before a canonical E2E lane runs.

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::any::Any;
use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use cua_driver_testkit::e2e::DisplayServer;
use cua_driver_testkit::e2e::{write_environment_from_env, EnvironmentRecord};
use cua_driver_testkit::{driver_binary, harness_app, spawn_in_job, Driver, McpDriver};

fn electron_binary() -> (std::path::PathBuf, Vec<&'static str>) {
    #[cfg(target_os = "windows")]
    {
        (
            harness_app("harness-electron", "CuaTestHarness.Electron.exe"),
            vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
        )
    }
    #[cfg(target_os = "macos")]
    {
        (
            harness_app(
                "harness-electron",
                "CuaTestHarness.Electron.app/Contents/MacOS/Electron",
            ),
            vec!["--force-renderer-accessibility"],
        )
    }
    #[cfg(target_os = "linux")]
    {
        (
            harness_app("harness-electron", "CuaTestHarness.Electron"),
            vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
        )
    }
}

fn spawn_driver() -> McpDriver {
    #[cfg(target_os = "macos")]
    {
        McpDriver::spawn_macos_daemon_proxy_named("environment-preflight")
            .expect("installed CuaDriver daemon is not available")
    }
    #[cfg(not(target_os = "macos"))]
    {
        McpDriver::spawn_named("environment-preflight")
            .expect("source-built cua-driver could not be started")
    }
}

fn has_image(response: &cua_driver_testkit::ToolResponse) -> bool {
    response.raw["result"]["content"]
        .as_array()
        .map(|content| {
            content
                .iter()
                .any(|item| item["type"].as_str() == Some("image"))
        })
        .unwrap_or(false)
        || response.structured()["screenshot_png_base64"]
            .as_str()
            .map(|png| !png.is_empty())
            .unwrap_or(false)
}

fn run_preflight() {
    let driver_path = driver_binary();
    assert!(
        driver_path.is_file(),
        "source-built driver is missing at {}",
        driver_path.display()
    );
    let version = Command::new(&driver_path)
        .arg("--version")
        .output()
        .expect("source-built driver --version failed");
    assert!(
        version.status.success(),
        "source-built driver is not runnable"
    );
    let version = String::from_utf8_lossy(&version.stdout);
    assert!(
        version.contains(env!("CARGO_PKG_VERSION")),
        "driver version mismatch: {version}"
    );

    let (fixture_path, fixture_args) = electron_binary();
    assert!(
        fixture_path.exists(),
        "required preflight fixture is missing at {}",
        fixture_path.display()
    );
    let recordings_root = std::env::var_os("CUA_E2E_RECORDINGS_ROOT")
        .expect("CUA_E2E_RECORDINGS_ROOT is required for canonical E2E");

    let mut driver = spawn_driver();
    let config = driver.call("get_config", serde_json::json!({}));
    assert!(
        !config.is_error(),
        "connected driver get_config failed: {}",
        config.text()
    );
    assert_eq!(
        config.structured()["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "connected driver version does not match the source build"
    );
    let recording_dir = driver
        .recording_dir()
        .expect("preflight video recording did not start")
        .to_path_buf();
    assert!(
        recording_dir.starts_with(&recordings_root),
        "preflight recording escaped the artifact root"
    );

    let before = driver.call("list_windows", serde_json::json!({}));
    assert!(!before.is_error(), "list_windows failed: {}", before.text());
    let before_ids = before.structured()["windows"]
        .as_array()
        .map(|windows| {
            windows
                .iter()
                .filter_map(|window| window["window_id"].as_u64())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut command = Command::new(&fixture_path);
    command
        .args(fixture_args)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = spawn_in_job(&mut command).expect("preflight fixture failed to launch");
    let launched_pid = child.id() as i64;
    driver.reaper().push(child);

    let deadline = Instant::now() + Duration::from_secs(20);
    let (pid, window_id) = loop {
        let response = driver.call("list_windows", serde_json::json!({}));
        if let Some(window) = response.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["window_id"]
                        .as_u64()
                        .map(|id| !before_ids.contains(&id))
                        .unwrap_or(false)
                        && window["title"]
                            .as_str()
                            .unwrap_or("")
                            .contains("CuaTestHarness Electron")
                })
            })
        {
            if let Some(window_id) = window["window_id"].as_u64() {
                break (window["pid"].as_i64().unwrap_or(launched_pid), window_id);
            }
        }
        assert!(
            Instant::now() < deadline,
            "preflight fixture window did not appear"
        );
        std::thread::sleep(Duration::from_millis(200));
    };

    #[cfg(target_os = "linux")]
    {
        if DisplayServer::current() == DisplayServer::X11 {
            let activated = driver.call(
                "bring_to_front",
                serde_json::json!({ "pid": pid, "window_id": window_id }),
            );
            assert!(
                !activated.is_error(),
                "preflight fixture could not be placed on the Linux desktop: {}",
                activated.text()
            );
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = driver.call(
            "get_window_state",
            serde_json::json!({
                "pid": pid,
                "window_id": window_id,
                "capture_mode": "ax"
            }),
        );
        if !state.is_error()
            && state.tree_text().contains("WEB_HARNESS_MARKER_v1")
            && has_image(&state)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "preflight could not read both AX state and screenshot: {}",
            state.text()
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    drop(driver);
    let video = recording_dir.join("recording.mp4");
    assert!(
        std::fs::metadata(&video)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false),
        "preflight video is missing or empty at {}",
        video.display()
    );
    assert!(
        !recording_dir.join("recording-error.txt").exists(),
        "preflight recording reported an error"
    );
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(&video)
        .status()
        .expect("ffprobe is required for canonical E2E");
    assert!(probe.success(), "ffprobe rejected the preflight video");

    let frame = recording_dir.join("preflight-frame.png");
    let extracted = Command::new("ffmpeg")
        .args(["-y", "-sseof", "-0.2", "-i"])
        .arg(&video)
        .args(["-frames:v", "1"])
        .arg(&frame)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("ffmpeg is required for canonical E2E");
    assert!(
        extracted.success(),
        "could not extract preflight video frame"
    );
    let frame = image::open(&frame)
        .expect("preflight video frame is not a readable image")
        .to_rgb8();
    let non_dark_pixels = frame
        .pixels()
        .filter(|pixel| pixel.0.iter().copied().max().unwrap_or(0) > 30)
        .count();
    assert!(
        non_dark_pixels * 1_000 >= frame.pixels().len(),
        "preflight video is effectively blank: {non_dark_pixels}/{} non-dark pixels",
        frame.pixels().len()
    );
}

fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "preflight panicked without a string payload".to_owned())
}

#[test]
#[ignore]
fn canonical_e2e_environment_is_ready() {
    let started = Instant::now();
    let outcome = panic::catch_unwind(AssertUnwindSafe(run_preflight));
    match outcome {
        Ok(()) => write_environment_from_env(&EnvironmentRecord::ready(started.elapsed()))
            .expect("write environment record"),
        Err(payload) => {
            let message = panic_message(&payload);
            write_environment_from_env(&EnvironmentRecord::error(started.elapsed(), message))
                .expect("write failed environment record");
            panic::resume_unwind(payload);
        }
    }
}
