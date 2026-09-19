//! One strict GUI/recording preflight before a canonical E2E lane runs.

#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use std::any::Any;
use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use cua_driver_testkit::e2e::DisplayServer;
use cua_driver_testkit::e2e::{write_environment_from_env, EnvironmentRecord};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{driver_binary, harness_app, spawn_in_job, Driver, McpDriver};

struct PreflightFixture {
    path: std::path::PathBuf,
    args: Vec<&'static str>,
    title: &'static str,
    ax_marker: &'static str,
}

struct FixtureChildGuard {
    child: Option<Child>,
}

impl FixtureChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("fixture child guard is armed")
    }

    fn into_child(mut self) -> Child {
        self.child.take().expect("fixture child guard is armed")
    }
}

impl Drop for FixtureChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn preflight_fixture() -> PreflightFixture {
    #[cfg(target_os = "windows")]
    {
        PreflightFixture {
            path: harness_app("harness-electron", "CuaTestHarness.Electron.exe"),
            args: vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
            title: "CuaTestHarness Electron",
            ax_marker: "WEB_HARNESS_MARKER_v1",
        }
    }
    #[cfg(target_os = "macos")]
    {
        PreflightFixture {
            path: harness_app(
                "harness-electron",
                "CuaTestHarness.Electron.app/Contents/MacOS/Electron",
            ),
            args: vec!["--force-renderer-accessibility"],
            title: "CuaTestHarness Electron",
            ax_marker: "WEB_HARNESS_MARKER_v1",
        }
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var("CUA_E2E_INTERNAL_LANE").as_deref() == Ok("native") {
            PreflightFixture {
                path: harness_app("harness-gtk3", "CuaTestHarness.Gtk3"),
                args: Vec::new(),
                title: "CuaTestHarness GTK3",
                ax_marker: "HARNESS_TEXT_MARKER_v1",
            }
        } else {
            PreflightFixture {
                path: harness_app("harness-electron", "CuaTestHarness.Electron"),
                args: vec![
                    "--no-sandbox",
                    "--disable-gpu",
                    "--force-renderer-accessibility",
                ],
                title: "CuaTestHarness Electron",
                ax_marker: "WEB_HARNESS_MARKER_v1",
            }
        }
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
    image_present(&response.raw, response.structured())
}

fn image_present(raw: &serde_json::Value, structured: &serde_json::Value) -> bool {
    raw["result"]["content"]
        .as_array()
        .map(|content| {
            content
                .iter()
                .any(|item| item["type"].as_str() == Some("image"))
        })
        .unwrap_or(false)
        || structured["screenshot_png_base64"]
            .as_str()
            .map(|png| !png.is_empty())
            .unwrap_or(false)
}

fn readiness_contract(
    is_error: bool,
    element_count: usize,
    marker_present: bool,
    image_present: bool,
) -> bool {
    !is_error && element_count > 0 && marker_present && image_present
}

fn ax_count(structured: &serde_json::Value) -> usize {
    structured["element_count"]
        .as_u64()
        .map(|count| count as usize)
        .or_else(|| structured["elements"].as_array().map(Vec::len))
        .unwrap_or_default()
}

fn preflight_state_ready(response: &cua_driver_testkit::ToolResponse, ax_marker: &str) -> bool {
    let element_count = ax_count(response.structured());
    readiness_contract(
        response.is_error(),
        element_count,
        response.tree_text().contains(ax_marker),
        has_image(response),
    )
}

// Keep the timeout record safe for both terminal logs and environment.jsonl.
// No raw response, free-form MCP text, AX payload, image, or unknown field is copied.
const READINESS_DIAGNOSTIC_BYTES: usize = 8 * 1024;
const DIAGNOSTIC_STRING_BYTES: usize = 512;

fn diagnostic_string(value: &str) -> serde_json::Value {
    let mut text = String::new();
    let mut encoded_bytes = 2; // JSON quotes; account for escaping before appending.
    let mut truncated = false;
    let mut redacted = false;
    for ch in value.chars() {
        // ASCII-only diagnostics deliberately replace Unicode as well as C0/C1,
        // escape, bidi, and other invisible terminal-formatting characters.
        let safe = if ch.is_ascii() && !ch.is_ascii_control() {
            ch
        } else {
            '\u{fffd}'
        };
        let cost = if matches!(safe, '"' | '\\') {
            2
        } else {
            safe.len_utf8()
        };
        if encoded_bytes + cost > DIAGNOSTIC_STRING_BYTES {
            truncated = true;
            break;
        }
        redacted |= safe != ch;
        text.push(safe);
        encoded_bytes += cost;
    }
    // These fields describe a canonical fixture, not arbitrary user content.
    // Conservatively suppress whole strings with URL/assignment syntax or
    // credential hints, rather than retaining URL userinfo, queries, or fragments.
    let lower = text.to_ascii_lowercase();
    if text.contains([':', '@', '?', '#', '=', '%'])
        || lower.split_whitespace().any(|word| word.len() > 64)
        || [
            "password",
            "secret",
            "token",
            "credential",
            "authorization",
            "bearer",
            "api_key",
            "apikey",
            "private key",
            "cookie",
        ]
        .iter()
        .any(|hint| lower.contains(hint))
    {
        text = "[redacted]".to_owned();
        redacted = true;
    }
    serde_json::json!({
        "state": "string", "value": text, "truncated": truncated, "redacted": redacted
    })
}

fn diagnostic_scalar(value: Option<&serde_json::Value>, expected: &str) -> serde_json::Value {
    use serde_json::{json, Value};
    match value {
        None => json!({"state": "absent", "value": null}),
        Some(Value::Null) => json!({"state": "null", "value": null}),
        Some(Value::String(text)) if expected == "string" => diagnostic_string(text),
        Some(Value::Bool(value)) if expected == "boolean" => {
            json!({"state": "boolean", "value": value})
        }
        Some(value) if expected == "u64" && value.as_u64().is_some() => {
            json!({"state": "number", "value": value.as_u64()})
        }
        Some(value) => {
            let kind = match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            json!({"state": "unexpected_type", "type": kind, "value": null})
        }
    }
}

fn readiness_diagnostic(
    structured: &serde_json::Value,
    is_error: bool,
    marker_present: bool,
    image_present: bool,
    pid: i64,
    window_id: u64,
) -> String {
    use serde_json::json;
    let screenshot_error = match structured.get("screenshot_error") {
        Some(serde_json::Value::Object(error)) => {
            let fields = ["code", "window_id", "reason", "message", "suggestion"];
            let mut projected = serde_json::Map::new();
            for field in fields {
                let expected = if field == "window_id" {
                    "u64"
                } else {
                    "string"
                };
                projected.insert(
                    field.to_owned(),
                    diagnostic_scalar(error.get(field), expected),
                );
            }
            json!({
                "state": "object", "value": projected,
                "other_fields_omitted": error.keys().any(|key| !fields.contains(&key.as_str()))
            })
        }
        value => diagnostic_scalar(value, "string"),
    };
    let encoded = json!({
        "sample": "latest_sampled_response",
        "stage": "get_window_state",
        "projection": "allowlisted_metadata_only",
        "target_pid": pid,
        "target_window_id": window_id,
        "is_error": is_error,
        "ax_count": ax_count(structured),
        "marker_present": marker_present,
        "image_present": image_present,
        "screenshot_frame_valid": diagnostic_scalar(structured.get("screenshot_frame_valid"), "boolean"),
        "degraded": diagnostic_scalar(structured.get("degraded"), "boolean"),
        "degraded_reason": diagnostic_scalar(structured.get("degraded_reason"), "string"),
        "screenshot_error": screenshot_error,
    }).to_string();
    // The closed projection has at most five strings, each <=512 encoded bytes.
    // Keep a fail-closed bound even if a future edit expands that projection.
    if encoded.len() > READINESS_DIAGNOSTIC_BYTES {
        json!({
            "sample": "latest_sampled_response", "stage": "get_window_state",
            "target_pid": pid, "target_window_id": window_id,
            "truncated": true, "diagnostic_omitted": "encoded_size_limit"
        })
        .to_string()
    } else {
        encoded
    }
}

fn extract_last_video_frame(
    video: &std::path::Path,
    frame: &std::path::Path,
) -> Result<(), String> {
    // Preserve evidence and prevent stale output from hiding a frameless decode.
    if frame
        .try_exists()
        .map_err(|error| format!("could not check preflight frame: {error}"))?
    {
        return Err("preflight frame already exists".to_owned());
    }
    // A VFR recording's final frame can precede its duration by more than a seek
    // offset. Decode in order and overwrite one image to retain the last frame.
    let extracted = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-xerror",
            "-y",
            "-i",
        ])
        .arg(video)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "demux",
            "-f",
            "image2",
            "-update",
            "1",
        ])
        .arg(frame)
        .stdout(Stdio::null())
        .output()
        .map_err(|error| format!("ffmpeg is required for canonical E2E: {error}"))?;
    let has_output = std::fs::metadata(frame)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false);
    if !extracted.status.success() || !has_output {
        let stderr = String::from_utf8_lossy(&extracted.stderr);
        let diagnostic: String = stderr.chars().take(4096).collect();
        return Err(format!(
            "could not extract preflight video frame (status {}; output missing/empty: {}): {diagnostic}",
            extracted.status,
            !has_output,
        ));
    }
    Ok(())
}

fn run_preflight() {
    let expected_sha = std::env::var("CUA_E2E_SOURCE_SHA").ok();
    if let Some(expected_sha) = expected_sha.as_deref() {
        assert!(
            expected_sha.len() == 40 && expected_sha.chars().all(|ch| ch.is_ascii_hexdigit()),
            "CUA_E2E_SOURCE_SHA must be a full commit SHA"
        );
        let source = Command::new("git").args(["rev-parse", "HEAD"]).output();
        let actual_sha = source
            .ok()
            .filter(|source| source.status.success())
            .map(|source| String::from_utf8_lossy(&source.stdout).trim().to_owned())
            .or_else(|| {
                let marker = std::env::var_os("CUA_E2E_SOURCE_MARKER")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from(".cua-e2e-source-sha"));
                std::fs::read_to_string(marker)
                    .ok()
                    .map(|source| source.trim().to_owned())
            })
            .expect("neither git HEAD nor .cua-e2e-source-sha identifies the synced source");
        assert_eq!(
            actual_sha.to_ascii_lowercase(),
            expected_sha.to_ascii_lowercase(),
            "checked-out source does not match the workflow's resolved SHA"
        );
    }

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

    let fixture = preflight_fixture();
    assert!(
        fixture.path.exists(),
        "required preflight fixture is missing at {}",
        fixture.path.display()
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
    if let Some(expected_sha) = expected_sha.as_deref() {
        assert_eq!(
            config.structured()["source_sha"].as_str(),
            Some(expected_sha),
            "connected driver was not built from the requested source SHA"
        );
    }
    let recording_dir = driver
        .recording_dir()
        .expect("preflight evidence directory was not prepared")
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

    let mut command = Command::new(&fixture.path);
    command
        .args(&fixture.args)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = spawn_in_job(&mut command).expect("preflight fixture failed to launch");
    let launched_pid = child.id() as i64;
    let mut child = FixtureChildGuard::new(child);

    let deadline = Instant::now() + Duration::from_secs(35);
    let mut last_readiness = r#"{"sample":"none","stage":"get_window_state"}"#.to_owned();
    #[cfg(target_os = "linux")]
    let mut activated_window_id = None;
    let (pid, window_id) = loop {
        if let Some(status) = child
            .child_mut()
            .try_wait()
            .expect("could not inspect preflight fixture process")
        {
            panic!("preflight fixture exited before mapping a window: {status}");
        }
        let windows = driver.call("list_windows", serde_json::json!({}));
        let candidate = windows.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find_map(|window| {
                    let window_id = window["window_id"].as_u64()?;
                    let is_new = !before_ids.contains(&window_id);
                    let title_matches = window["title"]
                        .as_str()
                        .unwrap_or("")
                        .contains(fixture.title);
                    (is_new && title_matches)
                        .then(|| (window["pid"].as_i64().unwrap_or(launched_pid), window_id))
                })
            });

        if let Some((pid, window_id)) = candidate {
            #[cfg(target_os = "linux")]
            if DisplayServer::current() == DisplayServer::X11
                && activated_window_id != Some(window_id)
            {
                let activated = driver.call(
                    "bring_to_front",
                    serde_json::json!({ "pid": pid, "window_id": window_id }),
                );
                assert!(
                    !activated.is_error(),
                    "preflight fixture could not be placed on the Linux desktop: {}",
                    activated.text()
                );
                activated_window_id = Some(window_id);
                std::thread::sleep(Duration::from_millis(300));
            }

            let state = driver.call(
                "get_window_state",
                serde_json::json!({
                    "pid": pid,
                    "window_id": window_id,
                    "capture_mode": "ax"
                }),
            );
            if preflight_state_ready(&state, fixture.ax_marker) {
                break (pid, window_id);
            }
            last_readiness = readiness_diagnostic(
                state.structured(),
                state.is_error(),
                state.tree_text().contains(fixture.ax_marker),
                has_image(&state),
                pid,
                window_id,
            );
        }

        if Instant::now() >= deadline {
            panic!(
                "preflight could not map a ready fixture window with AX state and screenshot; latest sampled get_window_state (not necessarily current): {last_readiness}; latest list_windows is_error: {}",
                windows.is_error()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    driver.reaper().push(child.into_child());

    let target = TargetWindow {
        pid: pid as u32,
        native_id: window_id,
    };
    let sentinel = ForegroundSentinel::launch(&mut driver);
    // Sentinel activation is preflight setup, not behavior under test. Starting
    // the recorder before launch turns its bring_to_front call into a captured
    // action; nested Wayland then blocks on a setup-only full-display preimage.
    // Keep the deliberate guard canaries in the recording while excluding the
    // activation that establishes their baseline.
    driver.start_behavior_recording();
    sentinel
        .assert_guard_canaries(&mut driver, target)
        .expect("foreground sentinel guard canaries failed");
    drop(sentinel);

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
    extract_last_video_frame(&video, &frame).unwrap_or_else(|error| panic!("{error}"));
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
fn readiness_requires_nonempty_ax_marker_and_screenshot_together() {
    assert!(readiness_contract(false, 1, true, true));
    assert!(!readiness_contract(false, 0, true, true));
    assert!(!readiness_contract(false, 1, false, true));
    assert!(!readiness_contract(false, 1, true, false));
    assert!(!readiness_contract(true, 1, true, true));
}

#[cfg(test)]
mod preflight_diagnostics {
    use super::*;
    use serde_json::{json, Value};

    fn sample(structured: &Value, is_error: bool, marker: bool, image: bool) -> Value {
        let encoded = readiness_diagnostic(structured, is_error, marker, image, 75315, 4935843840);
        assert!(encoded.len() <= READINESS_DIAGNOSTIC_BYTES);
        serde_json::from_str(&encoded).expect("bounded diagnostic must remain valid JSON")
    }

    #[test]
    fn screenshot_error_shapes_are_distinct() {
        let absent = sample(&json!({}), false, false, false);
        assert_eq!(
            absent["screenshot_error"],
            json!({"state": "absent", "value": null})
        );
        for (error, expected_state, expected_type) in [
            (Value::Null, "null", None),
            (json!("capture refused"), "string", None),
            (json!({}), "object", None),
            (json!(false), "unexpected_type", Some("boolean")),
            (json!(17), "unexpected_type", Some("number")),
            (
                json!(["private array content"]),
                "unexpected_type",
                Some("array"),
            ),
        ] {
            let summary = sample(&json!({"screenshot_error": error}), false, false, false);
            assert_eq!(summary["screenshot_error"]["state"], expected_state);
            assert_eq!(summary["screenshot_error"]["type"].as_str(), expected_type);
            assert!(!summary.to_string().contains("private array content"));
        }
        let string = sample(
            &json!({"screenshot_error": "capture refused"}),
            false,
            false,
            false,
        );
        assert_eq!(string["screenshot_error"]["value"], "capture refused");
        assert_eq!(string["screenshot_error"]["redacted"], false);
        assert_eq!(string["screenshot_error"]["truncated"], false);
    }

    #[test]
    fn structured_refusal_survives_without_payloads() {
        // Synthetic production-shaped refusal, NOT the lost response from CI.
        let structured = json!({
            "element_count": 23,
            "tree_markdown": "HARNESS_TEXT_MARKER_v1 private AX tree",
            "elements": [{"label": "private AX element"}],
            "screenshot_png_base64": "private image payload",
            "environment": {"TOKEN": "private environment"},
            "unknown_secret": "private unknown field",
            "screenshot_frame_valid": false,
            "degraded": true,
            "degraded_reason": "capture unavailable",
            "screenshot_error": {
                "code": "surface_identity_unproven",
                "window_id": 4935843840_u64,
                "reason": "no compositor-attested window geometry is available",
                "message": "capture refused",
                "suggestion": "use a compositor-attested capture route",
                "unknown_secret": "private nested field",
                "image": "private nested image"
            }
        });
        let summary = sample(&structured, false, true, true);
        assert_eq!(
            summary["screenshot_error"]["value"]["code"]["value"],
            "surface_identity_unproven"
        );
        assert_eq!(summary["sample"], "latest_sampled_response");
        assert_eq!(summary["stage"], "get_window_state");
        assert_eq!(summary["target_pid"], 75315);
        assert_eq!(summary["target_window_id"], 4935843840_u64);
        assert_eq!(summary["ax_count"], 23);
        assert_eq!(summary["marker_present"], true);
        assert_eq!(summary["image_present"], true);
        assert_eq!(summary["is_error"], false);
        assert_eq!(summary["screenshot_frame_valid"]["value"], false);
        assert_eq!(summary["degraded"]["value"], true);
        assert_eq!(summary["degraded_reason"]["value"], "capture unavailable");
        let error = &summary["screenshot_error"];
        assert_eq!(error["value"]["code"]["value"], "surface_identity_unproven");
        assert_eq!(error["value"]["window_id"]["value"], 4935843840_u64);
        assert_eq!(
            error["value"]["reason"]["value"],
            "no compositor-attested window geometry is available"
        );
        assert_eq!(error["value"]["message"]["value"], "capture refused");
        assert_eq!(
            error["value"]["suggestion"]["value"],
            "use a compositor-attested capture route"
        );
        assert_eq!(error["other_fields_omitted"], true);
        for excluded in [
            "private",
            "tree_markdown",
            "elements",
            "base64",
            "environment",
            "unknown_secret",
        ] {
            assert!(!summary.to_string().contains(excluded), "leaked {excluded}");
        }
    }

    #[test]
    fn nullable_and_malformed_metadata_is_not_coerced() {
        let absent = sample(&Value::Null, true, false, false);
        assert_eq!(absent["is_error"], true);
        for key in ["screenshot_frame_valid", "degraded", "degraded_reason"] {
            assert_eq!(absent[key]["state"], "absent");
            assert!(absent[key]["value"].is_null());
            let null = sample(&json!({key: null}), false, false, false);
            assert_eq!(null[key]["state"], "null");
        }
        let malformed = sample(
            &json!({
                "screenshot_frame_valid": "private flag",
                "degraded": {"secret": "private object"},
                "degraded_reason": ["private array"],
                "screenshot_error": {"code": ["private code"], "window_id": "private ID", "reason": null}
            }),
            false,
            false,
            false,
        );
        for key in ["screenshot_frame_valid", "degraded", "degraded_reason"] {
            assert_eq!(malformed[key]["state"], "unexpected_type");
            assert!(malformed[key]["value"].is_null());
        }
        let error = &malformed["screenshot_error"]["value"];
        assert_eq!(error["code"]["state"], "unexpected_type");
        assert_eq!(error["window_id"]["state"], "unexpected_type");
        assert_eq!(error["reason"]["state"], "null");
        assert_eq!(error["message"]["state"], "absent");
        assert!(!malformed.to_string().contains("private"));
    }

    #[test]
    fn controls_unicode_and_sensitive_strings_are_redacted() {
        let unsafe_text = "refusal\u{1b}[2J\r\n\t\0\u{7f}\u{85}\u{202e}\u{2066}\u{200b}\u{feff}雪";
        let field = diagnostic_string(unsafe_text);
        let safe = field["value"].as_str().unwrap();
        assert!(safe
            .chars()
            .all(|ch| (ch.is_ascii() && !ch.is_ascii_control()) || ch == '\u{fffd}'));
        assert!(safe.contains('\u{fffd}'));
        assert_eq!(field["redacted"], true);
        assert_eq!(field["truncated"], false);
        for text in [
            "https://alice:private-pass@example.test/path?key=private-query#private-fragment",
            "//alice@example.test/path",
            "example.test/path?private-query",
            "Bearer private-credential",
            "PASSWORD private-credential",
            "API_KEY private-credential",
            "data:image/png;base64,private-image",
            "TOKEN=private-env",
            "secret private-value",
        ] {
            let field = diagnostic_string(text);
            assert_eq!(field["value"], "[redacted]", "{text}");
            assert_eq!(field["redacted"], true);
            assert_eq!(field["truncated"], false);
        }
    }

    #[test]
    fn encoded_escaping_and_long_fields_respect_bounds() {
        // Quotes/backslashes expand on JSON encoding; spaces avoid a long opaque token.
        for unit in ["\" \\ ", "雪 ", "\u{1b} ", "readable words "] {
            let long = unit.repeat(10_000);
            let summary = sample(
                &json!({
                    "degraded_reason": long,
                    "screenshot_error": {"code": long, "reason": long, "message": long, "suggestion": long}
                }),
                false,
                true,
                false,
            );
            assert!(summary.get("diagnostic_omitted").is_none());
            for field in [
                &summary["degraded_reason"],
                &summary["screenshot_error"]["value"]["code"],
                &summary["screenshot_error"]["value"]["reason"],
                &summary["screenshot_error"]["value"]["message"],
                &summary["screenshot_error"]["value"]["suggestion"],
            ] {
                assert!(field["value"].to_string().len() <= DIAGNOSTIC_STRING_BYTES);
                assert_eq!(field["truncated"], true);
                assert_eq!(
                    field["redacted"],
                    !unit.is_ascii() || unit.contains('\u{1b}')
                );
            }
            // environment.jsonl embeds the already-encoded summary in a JSON string.
            assert!(
                json!({"error": summary.to_string()}).to_string().len()
                    <= READINESS_DIAGNOSTIC_BYTES
            );
        }
        let boundary = "a ".repeat(255); // 510 payload bytes plus two JSON quotes.
        let exact = diagnostic_string(&boundary);
        assert_eq!(exact["value"].to_string().len(), DIAGNOSTIC_STRING_BYTES);
        assert_eq!(exact["truncated"], false);
        let discarded_control = diagnostic_string(&(boundary.clone() + "\u{1b}"));
        assert_eq!(discarded_control["truncated"], true);
        assert_eq!(discarded_control["redacted"], false);
        let clipped = diagnostic_string(&(boundary + "x"));
        assert_eq!(clipped["truncated"], true);
        assert_eq!(clipped["redacted"], false);
        let opaque = diagnostic_string(&"a".repeat(10_000));
        assert_eq!(opaque["truncated"], true);
        assert_eq!(opaque["redacted"], true);
        assert_eq!(opaque["value"], "[redacted]");
    }

    #[test]
    fn ax_fallback_and_strict_readiness_are_unchanged() {
        for (structured, count) in [
            (json!({"element_count": 23, "elements": [{}]}), 23),
            (json!({"element_count": 0, "elements": [{}]}), 0),
            (json!({"elements": [{}, {}]}), 2),
            (json!({"element_count": "invalid", "elements": [{}]}), 1),
            (json!({"element_count": -1, "elements": [{}]}), 1),
            (Value::Null, 0),
        ] {
            assert_eq!(ax_count(&structured), count);
            for is_error in [false, true] {
                for marker in [false, true] {
                    for image in [false, true] {
                        let summary = sample(&structured, is_error, marker, image);
                        assert_eq!(summary["ax_count"], count);
                        assert_eq!(summary["is_error"], is_error);
                        assert_eq!(summary["marker_present"], marker);
                        assert_eq!(summary["image_present"], image);
                        assert_eq!(
                            readiness_contract(is_error, count, marker, image),
                            !is_error && count > 0 && marker && image
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn image_predicate_keeps_both_existing_routes() {
        let no_image = json!({"result": {"content": [{"type": "text", "text": "marker"}]}});
        assert!(!image_present(
            &no_image,
            &json!({"screenshot_png_base64": ""})
        ));
        assert!(!image_present(
            &no_image,
            &json!({"screenshot_png_base64": false})
        ));
        assert!(image_present(
            &no_image,
            &json!({"screenshot_png_base64": "image"})
        ));
        // The existing predicate checks the content type, not decoded image validity.
        let image = json!({"result": {"content": [{"type": "image"}]}});
        assert!(image_present(&image, &Value::Null));
        assert!(!image_present(&Value::Null, &Value::Null));
    }
}

#[test]
fn last_frame_extraction_rejects_existing_output() {
    let directory = tempfile::tempdir().unwrap();
    let frame = directory.path().join("frame.png");
    std::fs::write(&frame, b"existing evidence").unwrap();
    let error =
        extract_last_video_frame(&directory.path().join("missing.mp4"), &frame).unwrap_err();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(std::fs::read(frame).unwrap(), b"existing evidence");
}

#[test]
#[ignore = "requires ffmpeg with lavfi and the mpeg4 encoder; no GUI required"]
fn last_frame_extraction_handles_long_final_duration_and_invalid_input() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("two-colors.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=32x32:r=2:d=1.5",
            "-vf",
            "drawbox=color=blue:t=fill:enable='gte(t,1)',setpts='if(eq(N,2),4,N)'",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "mpeg4",
        ])
        .arg(&video)
        .output()
        .expect("ffmpeg is required for this regression test");
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    // VFR timestamps are 0s, 0.5s, and 2s; the last (blue) frame lasts to 2.5s.
    // Seeking to 2.3s reproduces a successful exit without emitting a frame.
    let old_frame = directory.path().join("old-frame.png");
    let old = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-sseof",
            "-0.2",
            "-i",
        ])
        .arg(&video)
        .args(["-frames:v", "1"])
        .arg(&old_frame)
        .output()
        .unwrap();
    assert!(
        old.status.success(),
        "{}",
        String::from_utf8_lossy(&old.stderr)
    );
    assert!(
        !old_frame.exists(),
        "fixture must reproduce the frameless seek"
    );

    let frame = directory.path().join("last-frame.png");
    extract_last_video_frame(&video, &frame).unwrap();
    let decoded = image::open(&frame).unwrap().to_rgb8();
    assert_eq!(decoded.dimensions(), (32, 32));
    assert!(
        decoded
            .pixels()
            .all(|pixel| pixel[2] > 200 && pixel[0] < 30 && pixel[1] < 30),
        "must retain the last blue frame, not the first red frame"
    );

    let invalid = directory.path().join("invalid.mp4");
    std::fs::write(&invalid, b"not an MP4").unwrap();
    for input in [invalid, directory.path().join("missing.mp4")] {
        let output = directory.path().join("failed-frame.png");
        let error = extract_last_video_frame(&input, &output).unwrap_err();
        assert!(
            error.contains("could not extract preflight video frame"),
            "{error}"
        );
        assert!(!output.exists());
    }
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
