use std::{
    path::PathBuf,
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
use serde_json::json;

#[tokio::test]
#[ignore]
async fn real_encoder_short_lifetime_observations() {
    assert!(cfg!(target_os = "windows"));
    let root = PathBuf::from(std::env::var_os("CUA_RECORDING_DIAGNOSTIC_ROOT").unwrap());
    std::fs::create_dir_all(&root).unwrap();
    set_video_backend_factory(Box::new(FfmpegVideoBackendFactory));
    for sequence in 0..500_u64 {
        let output = root.join(format!("observation-{sequence:04}"));
        std::fs::create_dir_all(&output).unwrap();
        let session = Arc::new(RecordingSession::new());
        let hold_ms = (sequence * 37) % 201;
        let started = StartRecordingTool::new(session.clone())
            .invoke(json!({"output_dir": output, "record_video": true}))
            .await;
        std::fs::write(
            output.join("start.json"),
            serde_json::to_vec_pretty(&started).unwrap(),
        )
        .unwrap();
        assert_eq!(
            started.structured_content.as_ref().unwrap()["video_active"],
            true,
            "{started:?}"
        );
        std::thread::sleep(Duration::from_millis(hold_ms));
        let clock = Instant::now();
        let stopped = StopRecordingTool::new(session.clone())
            .invoke(json!({}))
            .await;
        let elapsed_ms = clock.elapsed().as_millis();
        let state = GetRecordingStateTool::new(session).invoke(json!({})).await;
        let stopped = serde_json::to_value(stopped).unwrap();
        let state = state.structured_content.unwrap();
        let row = json!({"sequence": sequence, "hold_ms": hold_ms, "stop_elapsed_ms": elapsed_ms, "stop": stopped, "state": state});
        std::fs::write(
            output.join("observation.json"),
            serde_json::to_vec_pretty(&row).unwrap(),
        )
        .unwrap();
        eprintln!(
            "recording observation {sequence} hold_ms={hold_ms} stop_elapsed_ms={elapsed_ms}"
        );
        assert_ne!(stopped["isError"], true, "{row}");
        assert!(state["last_video_path"].is_string(), "{row}");
    }
}
