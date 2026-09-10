use std::{process::Command, sync::Arc};

use cua_driver_core::{
    recording::RecordingSession,
    recording_tools::{GetRecordingStateTool, StartRecordingTool, StopRecordingTool},
    tool::Tool,
    video::set_video_backend_factory,
    video_ffmpeg::FfmpegVideoBackendFactory,
};
use serde_json::json;

#[tokio::test]
async fn stop_recording_reports_shutdown_timeout() {
    if std::env::var_os("CUA_RECORDING_ERROR_TEST_CHILD").is_none() {
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
            .args([
                "--exact",
                "stop_recording_reports_shutdown_timeout",
                "--nocapture",
            ])
            .env("CUA_RECORDING_ERROR_TEST_CHILD", "timeout")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }
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
    assert_eq!(stopped.is_error, Some(true));
    let response = serde_json::to_value(&stopped).unwrap();
    assert!(
        response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ffmpeg shutdown timed out"),
        "{response}"
    );
    let state = GetRecordingStateTool::new(session).invoke(json!({})).await;
    let state = state.structured_content.unwrap();
    assert_eq!(state["enabled"], false);
    assert!(state["last_video_path"].is_null());
    assert!(state["last_error"]
        .as_str()
        .unwrap()
        .contains("ffmpeg shutdown timed out"));
}
