use std::{
    io::{Cursor, Write},
    process::{Command, Output, Stdio},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cua_perception::{
    handle_payload, read_frame, write_frame, FrameError, Response, MAX_FRAME_BYTES,
    MAX_IMAGE_DIMENSION,
};
use serde_json::{json, Value};

const FIXTURE_REQUEST: &str = include_str!("fixtures/parse-request.json");
const FIXTURE_RESPONSE: &str = include_str!("fixtures/parse-response.json");

fn response(payload: impl AsRef<[u8]>) -> Value {
    serde_json::to_value(handle_payload(payload.as_ref())).expect("serialize response")
}

#[test]
fn health_request_reports_fixture_only_capabilities() {
    let actual = response(
        br#"{"protocol":"cua-perception/1","request_id":"health-1","method":"health","params":{}}"#,
    );
    assert_eq!(actual["status"], "ok");
    assert_eq!(actual["result"]["ready"], true);
    assert_eq!(actual["result"]["runtime"], "fixture_only");
}

#[test]
fn protocol_mismatch_is_structured() {
    let actual = response(
        br#"{"protocol":"cua-perception/2","request_id":"version-1","method":"health","params":{}}"#,
    );
    assert_eq!(actual["status"], "error");
    assert_eq!(actual["error"]["code"], "incompatible_protocol");
    assert_eq!(actual["request_id"], "version-1");
}

#[test]
fn malformed_json_is_structured() {
    let actual = response(br#"{"protocol": broken"#);
    assert_eq!(actual["status"], "error");
    assert_eq!(actual["error"]["code"], "invalid_json");
    assert!(actual.get("request_id").is_none());
}

#[test]
fn framing_round_trips_and_rejects_malformed_lengths() {
    let payload = br#"{"fixture":true}"#;
    let mut framed = Vec::new();
    write_frame(&mut framed, payload).expect("write frame");
    assert_eq!(
        read_frame(&mut Cursor::new(framed)),
        Ok(Some(payload.to_vec()))
    );

    let mut oversized = Cursor::new(((MAX_FRAME_BYTES as u32) + 1).to_be_bytes());
    assert_eq!(
        read_frame(&mut oversized),
        Err(FrameError::Oversized {
            declared: MAX_FRAME_BYTES + 1,
            maximum: MAX_FRAME_BYTES
        })
    );

    let truncated = [0, 0, 0, 4, b'{', b'}'];
    let error = read_frame(&mut Cursor::new(truncated)).expect_err("reject truncated frame");
    assert_eq!(
        error,
        FrameError::TruncatedPayload {
            declared: 4,
            received: 2
        }
    );
    let structured =
        serde_json::to_value(Response::from_frame_error(&error)).expect("serialize framing error");
    assert_eq!(structured["status"], "error");
    assert_eq!(structured["error"]["code"], "invalid_frame");

    let prefix_error =
        read_frame(&mut Cursor::new([0_u8, 0_u8])).expect_err("reject partial prefix");
    assert_eq!(prefix_error, FrameError::TruncatedPrefix { received: 2 });
    let prefix_response = serde_json::to_value(Response::from_frame_error(&prefix_error))
        .expect("serialize prefix error");
    assert_eq!(
        prefix_response["error"]["message"],
        "frame length prefix is incomplete: received 2 of 4 bytes"
    );

    let error = write_frame(&mut Vec::new(), &vec![0; MAX_FRAME_BYTES + 1])
        .expect_err("reject oversized outbound frame");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn oversized_dimensions_are_rejected_before_image_decode() {
    let request = json!({
        "protocol": "cua-perception/1",
        "request_id": "large-1",
        "method": "parse",
        "params": {
            "capture_id": "large",
            "image": {
                "media_type": "image/png",
                "width": 16385,
                "height": 1,
                "byte_length": 1,
                "data_base64": "AA=="
            }
        }
    });
    let actual = response(serde_json::to_vec(&request).expect("serialize request"));
    assert_eq!(actual["status"], "error");
    assert_eq!(actual["error"]["code"], "invalid_image");
    assert_eq!(actual["error"]["details"]["width"], 16385);
}

#[test]
fn image_byte_length_is_a_bounded_u64_on_the_wire() {
    let request = json!({
        "protocol": "cua-perception/1",
        "request_id": "large-byte-length",
        "method": "parse",
        "params": {
            "capture_id": "large-byte-length",
            "image": {
                "media_type": "image/png",
                "width": 1,
                "height": 1,
                "byte_length": 4_294_967_296_u64,
                "data_base64": ""
            }
        }
    });
    let actual = response(serde_json::to_vec(&request).expect("serialize request"));
    assert_eq!(actual["status"], "error");
    assert_eq!(actual["error"]["code"], "invalid_image");
    assert_eq!(actual["error"]["details"]["byte_length"], 4_294_967_296_u64);
}

#[test]
fn oversized_png_header_is_rejected_by_decoder_limits() {
    let actual = response_for_png_header(MAX_IMAGE_DIMENSION + 1, 1);
    assert_eq!(actual["status"], "error");
    assert_eq!(actual["error"]["code"], "invalid_image");
    assert_eq!(
        actual["error"]["message"],
        "decoded PNG exceeds image resource limits"
    );
}

#[test]
fn oversized_png_pixel_count_is_rejected_before_full_decode() {
    let actual = response_for_png_header(8193, 8193);
    assert_eq!(actual["status"], "error");
    assert_eq!(actual["error"]["code"], "invalid_image");
    assert_eq!(
        actual["error"]["message"],
        "decoded PNG dimensions exceed image resource limits"
    );
}

#[test]
fn fixture_parse_matches_the_checked_in_golden() {
    let actual = response(FIXTURE_REQUEST);
    let expected: Value = serde_json::from_str(FIXTURE_RESPONSE).expect("parse response golden");
    assert_eq!(actual, expected);
    assert_eq!(response(FIXTURE_REQUEST), expected);
}

#[test]
fn self_test_exercises_the_embedded_fixture() {
    let actual = response(
        br#"{"protocol":"cua-perception/1","request_id":"self-test-1","method":"self_test","params":{}}"#,
    );
    assert_eq!(actual["status"], "ok");
    assert_eq!(actual["result"]["passed"], true);
    assert_eq!(
        actual["result"]["checks"],
        json!(["fixture_decode", "image_validation", "known_answer_fixture"])
    );
}

#[test]
fn standalone_binary_serves_multiple_framed_requests_over_stdio() {
    let first_request =
        br#"{"protocol":"cua-perception/1","request_id":"binary-1","method":"health","params":{}}"#;
    let second_request =
        br#"{"protocol":"cua-perception/1","request_id":"binary-2","method":"self_test","params":{}}"#;
    let mut framed = Vec::new();
    write_frame(&mut framed, first_request).expect("frame first request");
    write_frame(&mut framed, second_request).expect("frame second request");

    let output = run_worker(&framed);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let mut stdout = Cursor::new(output.stdout);
    let first_payload = read_frame(&mut stdout)
        .expect("read response frame")
        .expect("response payload");
    let first: Value = serde_json::from_slice(&first_payload).expect("parse first response JSON");
    assert_eq!(first["request_id"], "binary-1");
    assert_eq!(first["status"], "ok");
    assert_eq!(first["result"]["ready"], true);

    let second_payload = read_frame(&mut stdout)
        .expect("read second response frame")
        .expect("second response payload");
    let second: Value =
        serde_json::from_slice(&second_payload).expect("parse second response JSON");
    assert_eq!(second["request_id"], "binary-2");
    assert_eq!(second["status"], "ok");
    assert_eq!(second["result"]["passed"], true);
    assert_eq!(read_frame(&mut stdout), Ok(None));
}

#[test]
fn standalone_binary_recovers_after_malformed_json_frame() {
    let mut framed = Vec::new();
    write_frame(&mut framed, br#"{"protocol":broken"#).expect("frame malformed JSON");
    write_frame(
        &mut framed,
        br#"{"protocol":"cua-perception/1","request_id":"after-error","method":"health","params":{}}"#,
    )
    .expect("frame valid request");

    let output = run_worker(&framed);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let mut stdout = Cursor::new(output.stdout);
    let malformed = read_json_frame(&mut stdout);
    assert_eq!(malformed["status"], "error");
    assert_eq!(malformed["error"]["code"], "invalid_json");
    let valid = read_json_frame(&mut stdout);
    assert_eq!(valid["request_id"], "after-error");
    assert_eq!(valid["status"], "ok");
    assert_eq!(read_frame(&mut stdout), Ok(None));
}

#[test]
fn standalone_binary_reports_oversized_frame_and_terminates() {
    let mut input = ((MAX_FRAME_BYTES as u32) + 1).to_be_bytes().to_vec();
    write_frame(
        &mut input,
        br#"{"protocol":"cua-perception/1","request_id":"must-not-run","method":"health","params":{}}"#,
    )
    .expect("append valid frame");

    let output = run_worker(&input);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let mut stdout = Cursor::new(output.stdout);
    let error = read_json_frame(&mut stdout);
    assert_eq!(error["status"], "error");
    assert_eq!(error["error"]["code"], "invalid_frame");
    assert!(error.get("request_id").is_none());
    assert_eq!(read_frame(&mut stdout), Ok(None));
}

#[test]
fn standalone_binary_requires_an_explicit_backend() {
    let output = Command::new(env!("CARGO_BIN_EXE_cua-perception"))
        .output()
        .expect("run worker without backend arguments");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("usage: cua-perception --fixture | --manifest <manifest.json>"));
}

fn png_crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn response_for_png_header(width: u32, height: u32) -> Value {
    let request: Value = serde_json::from_str(FIXTURE_REQUEST).expect("parse fixture request");
    let encoded = request["params"]["image"]["data_base64"]
        .as_str()
        .expect("fixture base64");
    let mut png = BASE64.decode(encoded).expect("decode fixture PNG");
    png[16..20].copy_from_slice(&width.to_be_bytes());
    png[20..24].copy_from_slice(&height.to_be_bytes());
    let ihdr_crc = png_crc32(&png[12..29]);
    png[29..33].copy_from_slice(&ihdr_crc.to_be_bytes());

    let request = json!({
        "protocol": "cua-perception/1",
        "request_id": "oversized-ihdr-1",
        "method": "parse",
        "params": {
            "capture_id": "oversized-ihdr",
            "image": {
                "media_type": "image/png",
                "width": 1,
                "height": 1,
                "byte_length": png.len(),
                "data_base64": BASE64.encode(png)
            }
        }
    });
    response(serde_json::to_vec(&request).expect("serialize request"))
}

fn run_worker(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cua-perception"))
        .arg("--fixture")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn worker");
    child
        .stdin
        .take()
        .expect("worker stdin")
        .write_all(input)
        .expect("write worker input");
    child.wait_with_output().expect("wait for worker")
}

fn read_json_frame(reader: &mut Cursor<Vec<u8>>) -> Value {
    let payload = read_frame(reader)
        .expect("read response frame")
        .expect("response payload");
    serde_json::from_slice(&payload).expect("parse response JSON")
}
