use cua_driver_core::tool::ToolRegistry;
use serde_json::{json, Value};

pub async fn assert_recording_wired(registry: ToolRegistry) {
    let directory = tempfile::tempdir().unwrap();
    let video_error = match cua_driver_core::video::start_video(&directory.path().join("probe.mp4"))
    {
        Ok(video) => {
            let _ = video.stop();
            String::new()
        }
        Err(error) => error.to_string(),
    };
    let started = registry
        .invoke("start_recording", json!({"output_dir": directory.path()}))
        .await;
    assert_ne!(started.is_error, Some(true), "{started:?}");
    let result = registry
        .invoke("set_config", json!({"max_image_dimension": 0}))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let stopped = registry.invoke("stop_recording", json!({})).await;
    assert_ne!(stopped.is_error, Some(true), "{stopped:?}");
    let evidence: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("turn-00001/evidence.json")).unwrap(),
    )
    .unwrap();
    for phase in ["before", "after"] {
        assert_ne!(
            evidence[phase]["screenshot"]["classification"], "capture_hook_unavailable",
            "{evidence}"
        );
    }
    assert!(
        !video_error.contains("no video backend registered"),
        "{video_error}"
    );
}
