use std::{process::Command, sync::Arc};

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
    let (response, state) = record_with_encoder().await;
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
    let (response, state) = record_with_encoder().await;
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

async fn record_with_encoder() -> (Value, Value) {
    set_video_backend_factory(Box::new(FfmpegVideoBackendFactory));
    let session = Arc::new(RecordingSession::new());
    let directory = tempfile::tempdir().unwrap();
    let started = StartRecordingTool::new(session.clone())
        .invoke(json!({"output_dir": directory.path(), "record_video": true}))
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
