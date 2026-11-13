// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::{
    path::Path,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use cua_driver_core::{
    recording::RecordingSession,
    recording_tools::{GetRecordingStateTool, StartRecordingTool, StopRecordingTool},
    tool::Tool,
    video::set_video_backend_factory,
    video_ffmpeg::FfmpegVideoBackendFactory,
};
use serde_json::{json, Value};

const CHILD_MODE: &str = "CUA_RECORDING_ERROR_TEST_CHILD";

#[tokio::test]
async fn stop_recording_reports_encoder_exit() {
    if std::env::var_os(CHILD_MODE).is_none() {
        run_in_child_process("stop_recording_reports_encoder_exit", "exit");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (response, state) = record_with_encoder(directory.path()).await;
    assert_eq!(response["isError"], true);
    assert!(
        response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ffmpeg exited with code 23"),
        "{response}"
    );
    assert_eq!(state["enabled"], false);
    assert!(state["last_video_path"].is_null());
    assert!(state["last_error"]
        .as_str()
        .unwrap()
        .contains("ffmpeg exited with code 23"));
}

#[tokio::test]
async fn stop_recording_reports_shutdown_timeout() {
    if std::env::var_os(CHILD_MODE).is_none() {
        run_in_child_process("stop_recording_reports_shutdown_timeout", "timeout");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (response, state) = record_with_encoder(directory.path()).await;
    assert_eq!(response["isError"], true);
    assert!(
        response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ffmpeg shutdown timed out"),
        "{response}"
    );
    assert_eq!(state["enabled"], false);
    assert!(state["last_video_path"].is_null());
    assert!(state["last_error"]
        .as_str()
        .unwrap()
        .contains("ffmpeg shutdown timed out"));
}

/// #3713: a stalled desktop grab delayed ffmpeg's clean shutdown past the old
/// three-second deadline, so a finalized video was reported as a failure.
#[tokio::test]
async fn stop_recording_waits_for_a_slow_clean_encoder_shutdown() {
    if std::env::var_os(CHILD_MODE).is_none() {
        run_in_child_process(
            "stop_recording_waits_for_a_slow_clean_encoder_shutdown",
            "slow",
        );
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (response, state) = record_with_encoder(directory.path()).await;
    assert_ne!(response["isError"], true, "{response}");
    let video = directory.path().join("recording.mp4");
    assert_eq!(
        state["last_video_path"].as_str().map(Path::new),
        Some(video.as_path()),
        "{state}"
    );
    assert_eq!(std::fs::read(&video).unwrap(), b"finalized");
    assert!(state["last_error"].is_null(), "{state}");
}

/// #3713: Chocolatey's ffmpeg.exe is a shim around the real encoder. Killing
/// only the shim at the deadline left the encoder running, still writing the
/// file the caller had just been told failed.
#[cfg(windows)]
#[tokio::test]
async fn shutdown_timeout_stops_the_encoder_behind_a_shim() {
    if std::env::var_os(CHILD_MODE).is_none() {
        run_in_child_process("shutdown_timeout_stops_the_encoder_behind_a_shim", "shim");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let (response, _) = record_with_encoder(directory.path()).await;
    // The fixture encoder ignores `q` for 60 s; returning well before that
    // means stop did not wait for it to exit on its own.
    assert!(started.elapsed() < Duration::from_secs(40), "{response}");
    assert!(
        response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ffmpeg shutdown timed out"),
        "{response}"
    );
    let pid = std::fs::read_to_string(directory.path().join("recording.mp4.encoder-pid")).unwrap();
    let tasks = Command::new("tasklist")
        .args(["/NH", "/FO", "CSV", "/FI", &format!("PID eq {pid}")])
        .output()
        .unwrap();
    assert!(tasks.status.success(), "{tasks:?}");
    assert!(
        !String::from_utf8_lossy(&tasks.stdout).contains(&format!("\"{pid}\"")),
        "encoder {pid} outlived the shutdown timeout"
    );
}

fn run_in_child_process(test_name: &str, mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    let encoder = directory.path().join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    let compilation = Command::new("rustc")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/failing_encoder.rs"
        ))
        .arg("-o")
        .arg(&encoder)
        .output()
        .unwrap();
    assert!(compilation.status.success(), "{compilation:?}");
    let path = std::env::join_paths(
        std::iter::once(directory.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_MODE, mode)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

async fn record_with_encoder(output_dir: &Path) -> (Value, Value) {
    set_video_backend_factory(Box::new(FfmpegVideoBackendFactory));
    let session = Arc::new(RecordingSession::new());
    let started = StartRecordingTool::new(session.clone())
        .invoke(json!({"output_dir": output_dir, "record_video": true}))
        .await;
    assert_eq!(
        started.structured_content.as_ref().unwrap()["video_active"],
        true,
        "{started:?}"
    );
    let stopped = StopRecordingTool::new(session.clone())
        .invoke(json!({}))
        .await;
    let state = GetRecordingStateTool::new(session).invoke(json!({})).await;
    (
        serde_json::to_value(stopped).unwrap(),
        state.structured_content.unwrap(),
    )
}
