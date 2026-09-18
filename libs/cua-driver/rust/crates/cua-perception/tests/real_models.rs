use std::{env, fs, path::PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cua_perception::Worker;
use image::ImageReader;
use serde_json::{json, Value};

#[test]
#[ignore = "requires separately obtained, hash-pinned OmniParser/PP-OCR/ORT artifacts"]
fn pinned_real_models_match_the_synthetic_ui_known_answer() {
    let manifest = required_path("CUA_PERCEPTION_REAL_MANIFEST");
    let runtime = required_path("CUA_PERCEPTION_ORT_LIBRARY");
    let image_path = required_path("CUA_PERCEPTION_KNOWN_ANSWER_IMAGE");
    let worker = Worker::from_manifest(&manifest, &runtime).expect("load pinned real models");
    let bytes = fs::read(&image_path).expect("read known-answer image");
    let dimensions = ImageReader::open(&image_path)
        .expect("open known-answer image")
        .into_dimensions()
        .expect("read known-answer dimensions");
    let request = json!({
        "protocol": "cua-perception/1",
        "request_id": "real-known-answer",
        "method": "parse",
        "params": {
            "capture_id": "synthetic-ui",
            "image": {
                "media_type": "image/png",
                "width": dimensions.0,
                "height": dimensions.1,
                "byte_length": bytes.len(),
                "data_base64": BASE64.encode(bytes)
            }
        }
    });
    let response = serde_json::to_value(
        worker.handle_payload(&serde_json::to_vec(&request).expect("serialize request")),
    )
    .expect("serialize response");
    assert_eq!(response["status"], "ok", "{response:#}");
    assert_eq!(response["result"]["runtime"], "onnx_runtime_cpu");
    let regions = response["result"]["regions"]
        .as_array()
        .expect("regions array");
    assert_eq!(regions.len(), 8, "{regions:#?}");
    let texts = regions
        .iter()
        .filter_map(|region| region.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(texts.contains(&"Visual-only action fixture"));
    assert!(texts.contains(&"Archive"));
    assert!(texts.contains(&"Send"));
    assert!(texts.contains(&"Status: draft ready"));
    assert_eq!(
        regions
            .iter()
            .filter(|region| region["kind"] == "icon")
            .count(),
        3
    );
    assert!(regions
        .iter()
        .filter(|region| region["kind"] == "icon")
        .all(|region| region
            .get("label")
            .and_then(Value::as_str)
            .is_some_and(|label| !label.trim().is_empty())));
}

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must point to the pinned local test artifact"))
}
