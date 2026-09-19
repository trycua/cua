//! Artifact-gated perception demo for a custom-painted surface.
//!
//! The provider-neutral chooser sees bounded observation regions and action
//! descriptions, never Driver tool arguments, environment variables, or
//! credentials. The host validates one selected ID and resolves it to the
//! capture-bound action.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const MAX_REGIONS: usize = 64;
const MAX_ACTION_CANDIDATES: usize = 16;
const MAX_SAFE_ID_BYTES: usize = 96;
const MAX_CHOOSER_ID_BYTES: usize = 64;
const MIN_CONFIDENCE: f64 = 0.80;

#[derive(Clone, Debug, PartialEq)]
struct ClickAction {
    capture_id: String,
    x: f64,
    y: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    id: String,
    description: String,
    action: Option<ClickAction>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct RegionBounds {
    x: u64,
    y: u64,
    width: u64,
    height: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct CompactRegion {
    id: String,
    kind: String,
    bounds: RegionBounds,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    confidence: f64,
    interactive: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateDescription {
    id: String,
    description: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct HistoryEntry {
    selected_id: String,
    outcome: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ChoiceRequest {
    schema: &'static str,
    goal: &'static str,
    capture_id: String,
    regions: Vec<CompactRegion>,
    history: Vec<HistoryEntry>,
    candidates: Vec<CandidateDescription>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ChoiceResponse {
    schema: String,
    selected_id: String,
    model: Option<String>,
    confidence: f64,
    probabilities: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, PartialEq)]
enum ChoiceConfig {
    Mock,
    Live { program: PathBuf, script: PathBuf },
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SAFE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-/".contains(&byte))
}

fn safe_region_id(value: &str) -> bool {
    safe_chooser_id(value) && value.len() <= MAX_CHOOSER_ID_BYTES - "region:".len()
}

fn safe_chooser_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CHOOSER_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && b"._:-".contains(&byte))
        })
}

fn safe_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn bounded_candidates(
    payload: &Value,
    capture_id: &str,
) -> Result<(Vec<Candidate>, Vec<CompactRegion>), String> {
    if !safe_id(capture_id)
        || payload["schema"] != "cua.visual_regions_v1"
        || payload["capture"]["capture_id"].as_str() != Some(capture_id)
    {
        return Err("visual result has invalid schema or capture provenance".into());
    }
    let screenshot = &payload["capture"]["screenshot"];
    let screen_width = screenshot["width"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or("invalid screenshot width")?;
    let screen_height = screenshot["height"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or("invalid screenshot height")?;
    let regions = payload["regions"]
        .as_array()
        .ok_or("visual result omitted regions")?;
    if regions.len() > MAX_REGIONS {
        return Err("visual result exceeded the region bound".into());
    }

    let mut region_ids = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut compact = Vec::with_capacity(regions.len());
    for region in regions {
        let id = region["id"]
            .as_str()
            .filter(|value| safe_region_id(value))
            .ok_or("region has an unsafe ID")?;
        if !region_ids.insert(id) {
            return Err("duplicate region ID".into());
        }
        let kind = region["kind"]
            .as_str()
            .filter(|kind| matches!(*kind, "text" | "icon"))
            .ok_or("invalid region kind")?;
        let confidence = region["confidence"]
            .as_f64()
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or("invalid confidence")?;
        let interactive = region["interactive"]
            .as_bool()
            .ok_or("invalid interactivity")?;
        let text = region.get("text").and_then(Value::as_str).map(str::trim);
        let label = region.get("label").and_then(Value::as_str).map(str::trim);
        if text.is_some_and(|value| !safe_text(value, 128))
            || label.is_some_and(|value| !safe_text(value, 128))
            || (kind == "text" && text.is_none())
            || (kind == "icon" && label.is_none())
        {
            return Err("region content is unsafe or missing for its kind".into());
        }

        let bounds = &region["bounds"];
        let x = bounds["x"].as_u64().ok_or("invalid x")?;
        let y = bounds["y"].as_u64().ok_or("invalid y")?;
        let width = bounds["width"]
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or("invalid width")?;
        let height = bounds["height"]
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or("invalid height")?;
        if x.checked_add(width)
            .is_none_or(|value| value > screen_width)
            || y.checked_add(height)
                .is_none_or(|value| value > screen_height)
        {
            return Err("region lies outside its source screenshot".into());
        }
        compact.push(CompactRegion {
            id: id.into(),
            kind: kind.into(),
            bounds: RegionBounds {
                x,
                y,
                width,
                height,
            },
            text: text.map(str::to_owned),
            label: label.map(str::to_owned),
            confidence,
            interactive,
        });
        let content = if kind == "text" {
            text.expect("validated text region")
        } else {
            label.expect("validated icon region")
        };
        if interactive && confidence >= MIN_CONFIDENCE && !content.is_empty() {
            if candidates.len() >= MAX_ACTION_CANDIDATES {
                return Err("visual result exceeded the action candidate bound".into());
            }
            candidates.push(Candidate {
                id: format!("region:{id}"),
                description: format!("Activate the visual region labeled {content}."),
                action: Some(ClickAction {
                    capture_id: capture_id.into(),
                    x: x as f64 + width as f64 / 2.0,
                    y: y as f64 + height as f64 / 2.0,
                }),
            });
        }
    }
    candidates.extend([
        Candidate {
            id: "reobserve".into(),
            description: "Discard this decision set and capture a fresh observation.".into(),
            action: None,
        },
        Candidate {
            id: "abstain".into(),
            description: "Stop without acting when no proposed action is safe.".into(),
            action: None,
        },
    ]);
    Ok((candidates, compact))
}

fn choice_request(
    capture_id: &str,
    regions: Vec<CompactRegion>,
    candidates: &[Candidate],
) -> ChoiceRequest {
    ChoiceRequest {
        schema: "cua.jev_choice_request_v1",
        goal: "Select the painted Send control, or reobserve/abstain if it is not unambiguous.",
        capture_id: capture_id.into(),
        regions,
        history: Vec::new(),
        candidates: candidates
            .iter()
            .map(|candidate| CandidateDescription {
                id: candidate.id.clone(),
                description: candidate.description.clone(),
            })
            .collect(),
    }
}

fn validate_choice(
    response: ChoiceResponse,
    candidates: &[Candidate],
) -> Result<(ChoiceResponse, Candidate), String> {
    if response.schema != "cua.jev_choice_v1"
        || !safe_chooser_id(&response.selected_id)
        || response
            .model
            .as_deref()
            .is_some_and(|value| !safe_id(value))
        || !response.confidence.is_finite()
        || !(0.0..=1.0).contains(&response.confidence)
    {
        return Err("chooser returned invalid metadata".into());
    }
    let ids = candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<BTreeSet<_>>();
    for (id, probability) in &response.probabilities {
        if !ids.contains(id.as_str())
            || !probability.is_finite()
            || !(0.0..=1.0).contains(probability)
        {
            return Err("chooser returned an unknown or invalid probability".into());
        }
    }
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.id == response.selected_id)
        .cloned()
        .ok_or("chooser selected an ID that was not supplied")?;
    Ok((response, candidate))
}

fn mock_choice(candidates: &[Candidate]) -> ChoiceResponse {
    let selected_id = candidates
        .iter()
        .find(|candidate| candidate.description.contains("Send"))
        .expect("fixture must expose a Send candidate")
        .id
        .clone();
    let probabilities = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.id.clone(),
                if candidate.id == selected_id {
                    1.0
                } else {
                    0.0
                },
            )
        })
        .collect();
    ChoiceResponse {
        schema: "cua.jev_choice_v1".into(),
        selected_id,
        model: Some("mock".into()),
        confidence: 1.0,
        probabilities,
    }
}

fn choice_config_from(
    live: Option<&str>,
    mock: Option<&str>,
    program: Option<&str>,
    script: Option<&str>,
) -> Result<ChoiceConfig, String> {
    if live.is_some_and(|value| value != "1") || mock.is_some_and(|value| value != "1") {
        return Err("chooser mode flags, when present, must equal 1".into());
    }
    if live == Some("1") {
        if mock.is_some() {
            return Err("CUA_JEV_LIVE forbids CUA_JEV_MOCK_DEMO".into());
        }
        let program = PathBuf::from(program.ok_or("CUA_JEV_CHOOSER_PROGRAM is required")?);
        let script = PathBuf::from(script.ok_or("CUA_JEV_CHOOSER_SCRIPT is required")?);
        if !program.is_absolute() || !script.is_absolute() {
            return Err("reviewed chooser program and script paths must be absolute".into());
        }
        if !program.is_file() || !script.is_file() {
            return Err("reviewed chooser program and script must be regular files".into());
        }
        return Ok(ChoiceConfig::Live { program, script });
    }
    if mock == Some("1") {
        if program.is_some() || script.is_some() {
            return Err("mock mode forbids external chooser paths".into());
        }
        return Ok(ChoiceConfig::Mock);
    }
    Err("set CUA_JEV_MOCK_DEMO=1 or CUA_JEV_LIVE=1 with a reviewed chooser".into())
}

#[test]
fn chooser_request_matches_fixture_contract_and_contains_no_action_arguments() {
    let mut payload = json!({
        "schema": "cua.visual_regions_v1",
        "capture": {"capture_id": "capture-1", "screenshot": {"width": 760, "height": 460}},
        "regions": [
            {"id": "save", "kind": "icon", "bounds": {"x": 72, "y": 250, "width": 204, "height": 40}, "label": "Save", "confidence": 0.99, "interactive": true},
            {"id": "send", "kind": "text", "bounds": {"x": 292, "y": 250, "width": 204, "height": 40}, "text": "Send", "confidence": 0.98, "interactive": true}
        ]
    });
    let (candidates, regions) = bounded_candidates(&payload, "capture-1").unwrap();
    let request = serde_json::to_value(choice_request("capture-1", regions, &candidates)).unwrap();
    assert_eq!(request["schema"], "cua.jev_choice_request_v1");
    assert_eq!(
        request
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "candidates",
            "capture_id",
            "goal",
            "history",
            "regions",
            "schema",
        ])
    );
    assert_eq!(request["candidates"].as_array().unwrap().len(), 4);
    assert_eq!(
        request["regions"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["bounds", "confidence", "id", "interactive", "kind", "label",])
    );
    assert_eq!(
        request["regions"][0]["bounds"],
        json!({"x": 72, "y": 250, "width": 204, "height": 40})
    );
    assert!(request["regions"][0].get("content").is_none());
    assert!(request["regions"][0].get("text").is_none());
    assert!(request["regions"][1].get("label").is_none());
    assert_eq!(
        request["candidates"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["description", "id"])
    );
    assert!(request.to_string().contains("reobserve"));
    assert!(request.to_string().contains("abstain"));
    for forbidden in ["tool", "arguments", "delivery_mode", "secret"] {
        assert!(
            !request.to_string().contains(forbidden),
            "leaked {forbidden}"
        );
    }

    let (response, selected) = validate_choice(mock_choice(&candidates), &candidates).unwrap();
    assert_eq!(response.selected_id, "region:send");
    assert_eq!(selected.action.unwrap().capture_id, "capture-1");

    let oversized_capture_id = "a".repeat(MAX_SAFE_ID_BYTES + 1);
    payload["capture"]["capture_id"] = json!(oversized_capture_id);
    assert!(bounded_candidates(&payload, &oversized_capture_id).is_err());
}

#[test]
fn chooser_response_and_live_mode_fail_closed() {
    let candidates = vec![
        Candidate {
            id: "region:send".into(),
            description: "Activate Send.".into(),
            action: Some(ClickAction {
                capture_id: "capture-1".into(),
                x: 1.0,
                y: 2.0,
            }),
        },
        Candidate {
            id: "reobserve".into(),
            description: "Reobserve.".into(),
            action: None,
        },
        Candidate {
            id: "abstain".into(),
            description: "Abstain.".into(),
            action: None,
        },
    ];
    let mut response = mock_choice(&candidates);
    response.selected_id = "invented".into();
    assert!(validate_choice(response, &candidates).is_err());
    let mut response = mock_choice(&candidates);
    response.probabilities.insert("invented".into(), 0.1);
    assert!(validate_choice(response, &candidates).is_err());
    let mut response = mock_choice(&candidates);
    response.model = None;
    response.probabilities = BTreeMap::from([("region:send".into(), 0.4)]);
    assert!(validate_choice(response, &candidates).is_ok());
    assert!(choice_config_from(Some("1"), Some("1"), None, None)
        .unwrap_err()
        .contains("forbids"));
    assert!(choice_config_from(None, None, None, None).is_err());

    let oversized_region_id = "a".repeat(MAX_CHOOSER_ID_BYTES - "region:".len() + 1);
    let payload = json!({
        "schema": "cua.visual_regions_v1",
        "capture": {"capture_id": "capture-1", "screenshot": {"width": 10, "height": 10}},
        "regions": [{
            "id": oversized_region_id,
            "kind": "text",
            "bounds": {"x": 0, "y": 0, "width": 10, "height": 10},
            "text": "Send",
            "confidence": 1.0,
            "interactive": true
        }]
    });
    assert!(bounded_candidates(&payload, "capture-1")
        .unwrap_err()
        .contains("unsafe ID"));
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod e2e {
    use super::*;
    use cua_driver_testkit::{driver_binary, spawn_in_job, Driver, FixtureJournal, McpDriver};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const FIXTURE_TITLE_PREFIX: &str = "Cua Visual-Only Canvas Fixture";
    const FIXTURE_TEST_TITLE: &str = "Cua Visual-Only Canvas Fixture [test-session:window]";
    const FIXTURE_DISCOVERY_ATTEMPTS: usize = 3;
    const CHOOSER_TIMEOUT: Duration = Duration::from_secs(30);
    const MAX_CHOOSER_OUTPUT: u64 = 64 * 1024;
    const PAINTED_LABELS: [&str; 4] = ["Save", "Send", "Cancel", "CHOOSE A SIGNAL"];

    #[derive(Clone, Copy)]
    enum DemoScope {
        Window,
        PrimaryDesktop,
    }

    impl DemoScope {
        fn slug(self) -> &'static str {
            match self {
                Self::Window => "window",
                Self::PrimaryDesktop => "primary-desktop",
            }
        }

        fn capture_scope(self) -> &'static str {
            match self {
                Self::Window => "window",
                Self::PrimaryDesktop => "desktop",
            }
        }

        fn capture_kind(self) -> &'static str {
            match self {
                Self::Window => "get_window_state",
                Self::PrimaryDesktop => "get_desktop_state",
            }
        }

        fn delivery_mode(self) -> &'static str {
            match self {
                Self::Window => "background",
                // Primary-desktop input is screen-absolute and therefore foreground-only.
                Self::PrimaryDesktop => "foreground",
            }
        }
    }

    struct Gate {
        source_sha: String,
        jev_source_sha: String,
        session_label: String,
        os_name: String,
        os_version: String,
        os_arch: String,
        desktop_session: String,
        runner_identity_class: String,
        model: PathBuf,
        candidate_measurements: PathBuf,
        extension_home: PathBuf,
        evidence_dir: PathBuf,
        choice: ChoiceConfig,
    }

    fn required(name: &str) -> String {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is required for the artifact-gated demo"))
    }

    fn required_sha40(name: &str) -> String {
        let value = required(name);
        assert!(
            value.len() == 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "{name} must be 40 lowercase hexadecimal characters"
        );
        value
    }

    fn measured_sha256(value: &Value, field: &str) -> String {
        let value = value[field]
            .as_str()
            .unwrap_or_else(|| panic!("candidate measurements omitted {field}"));
        assert!(
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "candidate measurements contain invalid {field}"
        );
        value.to_owned()
    }

    fn hash_file(path: &Path) -> String {
        let mut file =
            fs::File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).expect("hash evidence file");
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        format!("{:x}", hasher.finalize())
    }

    fn hash_bytes(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn recording_metadata(path: &Path) -> Value {
        let output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,avg_frame_rate:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .expect("run ffprobe for recording evidence");
        assert!(
            output.status.success(),
            "ffprobe rejected recording evidence"
        );
        let probe: Value = serde_json::from_slice(&output.stdout).expect("parse ffprobe JSON");
        let streams = probe["streams"].as_array().expect("ffprobe video streams");
        assert_eq!(streams.len(), 1, "recording must contain one video stream");
        let width = streams[0]["width"].as_u64().expect("recording width");
        let height = streams[0]["height"].as_u64().expect("recording height");
        let rate = streams[0]["avg_frame_rate"]
            .as_str()
            .expect("recording frame rate");
        let (numerator, denominator) = rate.split_once('/').expect("rational frame rate");
        let numerator: u64 = numerator.parse().expect("frame-rate numerator");
        let denominator: u64 = denominator.parse().expect("frame-rate denominator");
        let duration_ms = (probe["format"]["duration"]
            .as_str()
            .expect("recording duration")
            .parse::<f64>()
            .expect("numeric recording duration")
            * 1000.0)
            .round() as u64;
        assert!(width > 0 && height > 0 && numerator > 0 && denominator > 0 && duration_ms > 0);
        json!({
            "width": width,
            "height": height,
            "frame_rate": {"numerator": numerator, "denominator": denominator},
            "duration_ms": duration_ms
        })
    }

    fn load_gate() -> Gate {
        let choice = choice_config_from(
            std::env::var("CUA_JEV_LIVE").ok().as_deref(),
            std::env::var("CUA_JEV_MOCK_DEMO").ok().as_deref(),
            std::env::var("CUA_JEV_CHOOSER_PROGRAM").ok().as_deref(),
            std::env::var("CUA_JEV_CHOOSER_SCRIPT").ok().as_deref(),
        )
        .unwrap_or_else(|error| panic!("invalid chooser configuration: {error}"));
        let gate = Gate {
            source_sha: required_sha40("CUA_E2E_SOURCE_SHA"),
            jev_source_sha: required_sha40("CUA_JEV_SOURCE_SHA"),
            session_label: required("CUA_SESSION_LABEL"),
            os_name: required("CUA_RUNNER_OS_NAME"),
            os_version: required("CUA_RUNNER_OS_VERSION"),
            os_arch: required("CUA_RUNNER_OS_ARCH"),
            desktop_session: required("CUA_DESKTOP_SESSION_TYPE"),
            runner_identity_class: required("CUA_RUNNER_IDENTITY_CLASS"),
            model: required("CUA_PERCEPTION_MODEL").into(),
            candidate_measurements: required("CUA_CANDIDATE_MEASUREMENTS").into(),
            extension_home: required("CUA_PERCEPTION_EXTENSION_HOME").into(),
            evidence_dir: required("CUA_PERCEPTION_EVIDENCE_DIR").into(),
            choice,
        };
        assert!(gate.model.is_file(), "model is not a regular file");
        assert!(safe_id(&gate.session_label), "session label is unsafe");
        for (label, value) in [
            ("OS name", &gate.os_name),
            ("OS version", &gate.os_version),
            ("OS architecture", &gate.os_arch),
            ("desktop session", &gate.desktop_session),
            ("runner identity class", &gate.runner_identity_class),
        ] {
            assert!(safe_id(value), "{label} is unsafe");
        }
        assert!(
            gate.candidate_measurements.is_file(),
            "candidate measurements are not a regular file"
        );
        assert!(
            gate.extension_home.is_dir(),
            "extension home is not a directory"
        );
        gate
    }

    fn installed_extension_status(gate: &Gate) -> Value {
        let output = Command::new(driver_binary())
            .args([
                "extension",
                "status",
                "cua-perception",
                "--self-test",
                "--json",
            ])
            .env("CUA_DRIVER_RS_HOME", &gate.extension_home)
            .output()
            .expect("measure installed extension state through Driver");
        assert!(
            output.status.success(),
            "Driver rejected installed extension state"
        );
        let status: Value =
            serde_json::from_slice(&output.stdout).expect("Driver extension status must be JSON");
        assert_eq!(status["id"], "cua-perception");
        assert_eq!(status["installed"], true);
        assert_eq!(status["healthy"], true);
        assert_eq!(status["trust"], "review-only-publisher-verified");
        assert!(status["active_version"].as_str().is_some_and(safe_id));
        assert!(status["publisher_id"].as_str().is_some_and(safe_id));
        assert!(status["publisher_key_id"].as_str().is_some_and(safe_id));
        assert!(status["catalog_version"]
            .as_u64()
            .is_some_and(|value| value > 0));
        status
    }

    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/apps/cross-platform/visual-only-canvas/main.py")
    }

    fn fixture_title(session_label: &str, scope: DemoScope) -> String {
        format!("{FIXTURE_TITLE_PREFIX} [{session_label}:{}]", scope.slug())
    }

    fn fixture_command(journal_url: &str, title: &str) -> Command {
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut command = Command::new("py");
            command.arg("-3");
            command
        };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let mut command = Command::new("python3");
        command
            .arg(fixture_path())
            .args(["--journal-url", journal_url, "--title", title])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        command
    }

    fn wait_until(mut predicate: impl FnMut() -> bool, message: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !predicate() {
            assert!(Instant::now() < deadline, "{message}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn fixture_window_id(window: &Value, pid: i64) -> Result<u64, String> {
        window["window_id"]
            .as_u64()
            .filter(|window_id| *window_id != 0)
            .ok_or_else(|| {
                format!("exact fixture window for pid {pid} has an invalid window_id: {window}")
            })
    }

    fn exact_fixture_window(
        windows: &Value,
        pid: i64,
        title: &str,
    ) -> Result<Option<(u64, String)>, String> {
        let windows = windows
            .as_array()
            .ok_or_else(|| "list_windows omitted its windows array".to_owned())?;
        let titled = windows
            .iter()
            .filter(|window| window["title"].as_str() == Some(title))
            .collect::<Vec<_>>();
        let owned = titled
            .iter()
            .copied()
            .filter(|window| window["pid"].as_i64() == Some(pid))
            .collect::<Vec<_>>();
        match owned.as_slice() {
            [] => match titled.as_slice() {
                [] => Ok(None),
                [window] if window["pid"].as_i64().is_some() => Err(format!(
                    "exact fixture title belongs to pid {}, not expected pid {pid}",
                    window["pid"]
                )),
                [window] if window.get("pid").is_none_or(Value::is_null) => Ok(None),
                [window] => Err(format!(
                    "exact fixture window has invalid pid metadata: {window}"
                )),
                _ => Err(format!(
                    "ambiguous fixture identity: {} exact-title windows lacked expected pid {pid}",
                    titled.len()
                )),
            },
            [window] => Ok(Some((fixture_window_id(window, pid)?, title.to_owned()))),
            _ => Err(format!(
                "ambiguous fixture identity: {} exact-title windows claimed pid {pid}",
                owned.len()
            )),
        }
    }

    fn fixture_window_diagnostic(windows: &Value, pid: i64, title: &str) -> String {
        let Some(windows) = windows.as_array() else {
            return "structured response omitted the windows array".to_owned();
        };
        let titled = windows
            .iter()
            .filter(|window| window["title"].as_str() == Some(title))
            .map(|window| {
                format!(
                    "window_id={} pid={}",
                    window["window_id"]
                        .as_u64()
                        .map_or_else(|| "invalid".to_owned(), |value| value.to_string()),
                    window["pid"]
                        .as_i64()
                        .map_or_else(|| "missing".to_owned(), |value| value.to_string())
                )
            })
            .collect::<Vec<_>>();
        if titled.is_empty() {
            format!(
                "no exact-title fixture among {} listed windows for expected pid {pid}",
                windows.len()
            )
        } else {
            format!(
                "exact-title candidates for expected pid {pid}: {}",
                titled.join(", ")
            )
        }
    }

    fn find_fixture_window(
        driver: &mut impl Driver,
        pid: i64,
        title: &str,
    ) -> Result<(u64, String), String> {
        for attempt in 1..=FIXTURE_DISCOVERY_ATTEMPTS {
            // Validate ownership locally so a backend-side PID filter cannot
            // hide whether the fixture was absent or merely lacked metadata.
            let response = driver.call("list_windows", json!({}));
            let diagnostic = if response.is_error() {
                format!("list_windows failed: {}", response.text())
            } else {
                let windows = &response.structured()["windows"];
                if let Some(window) = exact_fixture_window(windows, pid, title)? {
                    return Ok(window);
                }
                fixture_window_diagnostic(windows, pid, title)
            };
            if attempt == FIXTURE_DISCOVERY_ATTEMPTS {
                return Err(format!(
                    "fixture window discovery exhausted {attempt} attempts: {diagnostic}"
                ));
            }
            thread::sleep(Duration::from_millis(150));
        }
        unreachable!("fixture discovery attempt range is non-empty")
    }

    fn assert_painted_labels_absent_from_window_ax(observation: &cua_driver_testkit::ToolResponse) {
        for label in PAINTED_LABELS {
            assert!(
                !observation.tree_text().contains(label),
                "painted label leaked into window AX tree: {label}"
            );
        }
    }

    fn assert_painted_labels_are_visual_only(
        scope: DemoScope,
        observation: &cua_driver_testkit::ToolResponse,
    ) {
        match scope {
            DemoScope::Window => assert_painted_labels_absent_from_window_ax(observation),
            DemoScope::PrimaryDesktop => {
                assert!(
                    observation.structured().get("tree_markdown").is_none()
                        && observation.structured().get("elements").is_none(),
                    "get_desktop_state unexpectedly exposed an accessibility tree: {}",
                    observation.structured()
                );
                let response_text = observation
                    .raw
                    .pointer("/result/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                for label in PAINTED_LABELS {
                    assert!(
                        !response_text.contains(label),
                        "painted label leaked into desktop response text: {label}"
                    );
                }
            }
        }
    }

    fn external_choice(program: &Path, script: &Path, request: &ChoiceRequest) -> ChoiceResponse {
        let mut command = Command::new(program);
        command
            .arg(script)
            .env_clear()
            .env("TYPESAFE_API_KEY", required("TYPESAFE_API_KEY"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        if let Some(system_root) = std::env::var_os("SYSTEMROOT") {
            command.env("SYSTEMROOT", system_root);
        }
        let mut child = spawn_in_job(&mut command).expect("start reviewed chooser directly");
        let payload = serde_json::to_vec(request).expect("serialize chooser request");
        let mut stdin = child.stdin.take().expect("chooser stdin");
        stdin.write_all(&payload).expect("write chooser request");
        drop(stdin);
        let stdout = child.stdout.take().expect("chooser stdout");
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_CHOOSER_OUTPUT + 1)
                .read_to_end(&mut bytes)
                .expect("read chooser response");
            bytes
        });
        let deadline = Instant::now() + CHOOSER_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll chooser") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("chooser exceeded its 30 second deadline");
            }
            thread::sleep(Duration::from_millis(25));
        };
        assert!(status.success(), "chooser exited unsuccessfully");
        let bytes = reader.join().expect("join chooser response reader");
        assert!(
            bytes.len() as u64 <= MAX_CHOOSER_OUTPUT,
            "chooser response exceeded 64 KiB"
        );
        serde_json::from_slice(&bytes).expect("chooser response must match the exact JSON contract")
    }

    fn choose(
        config: &ChoiceConfig,
        request: &ChoiceRequest,
        candidates: &[Candidate],
    ) -> (ChoiceResponse, Candidate) {
        let response = match config {
            ChoiceConfig::Mock => mock_choice(candidates),
            ChoiceConfig::Live { program, script } => external_choice(program, script, request),
        };
        validate_choice(response, candidates)
            .unwrap_or_else(|error| panic!("chooser response refused: {error}"))
    }

    fn run_authorized_visual_only_demo(scope: DemoScope) {
        let gate = load_gate();
        let measured = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.."))
            .output()
            .expect("measure checkout SHA");
        assert!(measured.status.success());
        assert_eq!(
            String::from_utf8(measured.stdout).unwrap().trim(),
            gate.source_sha
        );
        let extension_status = installed_extension_status(&gate);
        let candidate_measurements: Value = serde_json::from_slice(
            &fs::read(&gate.candidate_measurements).expect("read candidate measurements"),
        )
        .expect("candidate measurements must be JSON");
        assert_eq!(candidate_measurements["review_only"], true);
        assert_eq!(candidate_measurements["source_sha"], gate.source_sha);
        assert_eq!(
            candidate_measurements["review_driver_build_profile"],
            "debug-review-trust-root"
        );
        assert_eq!(candidate_measurements["signature_algorithm"], "ed25519");
        let signed_extension_archive_sha256 =
            measured_sha256(&candidate_measurements, "archive_sha256");
        let signing_key_sha256 = measured_sha256(&candidate_measurements, "public_key_sha256");
        let signed_catalog_sha256 = measured_sha256(&candidate_measurements, "catalog_sha256");
        let driver_binary_sha256 = measured_sha256(&candidate_measurements, "review_driver_sha256");
        assert_eq!(driver_binary_sha256, hash_file(&driver_binary()));
        assert_eq!(
            measured_sha256(&candidate_measurements, "supplied_model_sha256"),
            hash_file(&gate.model)
        );

        let journal = FixtureJournal::start();
        let fixture_title = fixture_title(&gate.session_label, scope);
        let fixture = spawn_in_job(&mut fixture_command(journal.url(), &fixture_title))
            .expect("start canvas fixture");
        let pid = i64::from(fixture.id());
        wait_until(
            || journal.snapshot()["ready"].as_bool() == Some(true),
            "fixture did not become ready",
        );
        #[cfg(target_os = "macos")]
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(&gate.session_label)
            .expect("connect to the exact TCC-authorized review Driver daemon");
        #[cfg(not(target_os = "macos"))]
        let mut driver = {
            let extension_home = gate.extension_home.to_string_lossy().into_owned();
            McpDriver::spawn_named_with_env(
                &gate.session_label,
                &[("CUA_DRIVER_RS_HOME", extension_home.as_str())],
            )
            .expect("start Driver with installed candidate extension")
        };
        driver.reaper().push(fixture);
        let (window_id, _) = find_fixture_window(&mut driver, pid, &fixture_title)
            .unwrap_or_else(|error| panic!("find canvas fixture: {error}"));

        let window_session = format!("{}-window-target", gate.session_label);
        let action_session = format!("{}-{}", gate.session_label, scope.slug());
        for (session, capture_scope) in [
            (window_session.as_str(), "window"),
            (action_session.as_str(), scope.capture_scope()),
        ] {
            let started = driver.call(
                "start_session",
                json!({"session": session, "capture_scope": capture_scope}),
            );
            assert!(
                !started.is_error(),
                "start_session capture_scope={capture_scope} failed: {}",
                started.text()
            );
        }
        let framed = driver.call(
            "set_window_frame",
            json!({
                "session": window_session, "pid": pid, "window_id": window_id,
                "x": 96, "y": 96, "width": 780, "height": 500
            }),
        );
        assert!(
            !framed.is_error(),
            "could not set deterministic fixture frame: {}",
            framed.text()
        );
        let foregrounded = driver.call(
            "bring_to_front",
            json!({"session": window_session, "pid": pid, "window_id": window_id}),
        );
        assert!(
            !foregrounded.is_error(),
            "could not foreground canvas fixture: {}",
            foregrounded.text()
        );
        thread::sleep(Duration::from_millis(300));

        let started = Instant::now();
        if matches!(scope, DemoScope::PrimaryDesktop) {
            let window_ax = driver.call(
                "get_window_state",
                json!({
                    "session": window_session, "pid": pid, "window_id": window_id,
                    "capture_mode": "ax"
                }),
            );
            assert!(
                !window_ax.is_error(),
                "fixture AX proof failed before desktop capture: {}",
                window_ax.text()
            );
            assert_painted_labels_absent_from_window_ax(&window_ax);
        }
        let observation_args = match scope {
            DemoScope::Window => {
                json!({"session": action_session, "pid": pid, "window_id": window_id, "capture_mode": "ax"})
            }
            DemoScope::PrimaryDesktop => json!({"session": action_session}),
        };
        let first = driver.call(scope.capture_kind(), observation_args.clone());
        assert!(
            !first.is_error(),
            "initial observation failed: {}",
            first.text()
        );
        assert_painted_labels_are_visual_only(scope, &first);
        let capture_id = first.structured()["capture_id"]
            .as_str()
            .expect("capture_id")
            .to_owned();
        driver.start_behavior_recording();
        let parsed = driver.call("parse_visual_regions", json!({
            "session": action_session,
            "capture_id": capture_id,
            "options": {"kinds": ["text", "icon"], "min_confidence": MIN_CONFIDENCE, "max_regions": MAX_REGIONS}
        }));
        assert!(
            !parsed.is_error(),
            "perception worker failed: {}",
            parsed.text()
        );
        let (candidates, regions) =
            bounded_candidates(parsed.structured(), &capture_id).expect("bounded candidates");
        let request = choice_request(&capture_id, regions, &candidates);
        let (choice_response, selected_candidate) = choose(&gate.choice, &request, &candidates);
        if matches!(&gate.choice, ChoiceConfig::Live { .. }) {
            assert!(
                choice_response.model.as_deref().is_some_and(safe_id),
                "live chooser must return its bounded provider model identity"
            );
        }
        let choice = selected_candidate
            .action
            .clone()
            .expect("demo chooser must select an action");

        let click_args = match scope {
            DemoScope::Window => json!({
                "session": action_session, "pid": pid, "window_id": window_id,
                "x": choice.x, "y": choice.y, "capture_id": choice.capture_id,
                "delivery_mode": scope.delivery_mode()
            }),
            DemoScope::PrimaryDesktop => json!({
                "session": action_session, "scope": "desktop", "x": choice.x, "y": choice.y,
                "capture_id": choice.capture_id, "delivery_mode": scope.delivery_mode()
            }),
        };
        let (background_desktop_refused, refused_capture_id) =
            if matches!(scope, DemoScope::PrimaryDesktop) {
                let mut refused_args = click_args.clone();
                refused_args["delivery_mode"] = Value::String("background".to_owned());
                let refused_capture_id = refused_args["capture_id"]
                    .as_str()
                    .expect("background refusal capture_id")
                    .to_owned();
                let refused = driver.call("click", refused_args);
                assert!(
                    refused.is_error(),
                    "desktop/background click was not refused: {}",
                    refused.text()
                );
                assert_eq!(
                    refused.structured()["code"],
                    "background_unavailable",
                    "desktop/background refusal used an unexpected code"
                );
                assert_eq!(refused.structured()["effect"], "refused");
                assert_eq!(
                    refused.structured()["escalation"]["recommended"],
                    "foreground"
                );
                assert_eq!(
                    journal.snapshot()["action_count"],
                    0,
                    "refused desktop/background click reached the fixture"
                );
                (Some(true), Some(refused_capture_id))
            } else {
                (None, None)
            };
        let click = driver.call("click", click_args.clone());
        let click_succeeded = !click.is_error();
        assert!(
            click_succeeded,
            "capture-bound click failed: {}",
            click.text()
        );
        wait_until(
            || {
                let state = journal.snapshot();
                state["selected"] == "send" && state["action_count"] == 1
            },
            "journal did not record exactly one Send transition",
        );
        let capture_preserved_after_refusal = refused_capture_id
            .map(|refused_capture_id| click_succeeded && refused_capture_id == choice.capture_id);
        assert_eq!(
            capture_preserved_after_refusal, background_desktop_refused,
            "foreground retry did not successfully reuse the refused desktop capture"
        );
        assert!(
            driver.call("click", click_args).is_error(),
            "capture reuse was not refused"
        );

        let second = driver.call(scope.capture_kind(), observation_args);
        assert!(
            !second.is_error(),
            "fresh observation failed: {}",
            second.text()
        );
        let second_capture_id = second.structured()["capture_id"]
            .as_str()
            .expect("fresh capture_id")
            .to_owned();
        assert_ne!(capture_id, second_capture_id);
        let reparsed = driver.call("parse_visual_regions", json!({
            "session": action_session,
            "capture_id": second_capture_id,
            "options": {"kinds": ["text", "icon"], "min_confidence": MIN_CONFIDENCE, "max_regions": MAX_REGIONS}
        }));
        assert!(
            !reparsed.is_error(),
            "fresh perception pass failed: {}",
            reparsed.text()
        );
        let (fresh_candidates, _) = bounded_candidates(reparsed.structured(), &second_capture_id)
            .expect("fresh bounded candidates");
        assert!(fresh_candidates
            .iter()
            .any(|candidate| candidate.id == "reobserve"));
        assert!(fresh_candidates
            .iter()
            .any(|candidate| candidate.id == "abstain"));
        assert!(
            fresh_candidates
                .iter()
                .filter_map(|candidate| candidate.action.as_ref())
                .all(|action| action.capture_id == second_capture_id
                    && action.capture_id != capture_id),
            "reobserve reused an action from the consumed capture"
        );

        let oracle = journal.snapshot();
        let recording_dir = driver
            .recording_dir()
            .expect("recording directory")
            .to_path_buf();
        let parser = parsed.structured()["parser"].clone();
        let duration_ms = started.elapsed().as_millis() as u64;
        drop(driver);
        let recording = recording_dir.join("recording.mp4");
        assert!(recording.is_file(), "testkit recording did not finalize");
        let evidence_dir = match scope {
            DemoScope::Window => gate.evidence_dir.clone(),
            DemoScope::PrimaryDesktop => gate.evidence_dir.join("primary-desktop"),
        };
        fs::create_dir_all(&evidence_dir).expect("create evidence directory");
        let published_recording = evidence_dir.join("recording.mp4");
        fs::copy(&recording, &published_recording).expect("copy decoded recording evidence");
        let recording_sha256 = hash_file(&published_recording);
        let recording_size = fs::metadata(&published_recording)
            .expect("measure recording")
            .len();
        let recording_metadata = recording_metadata(&published_recording);
        let capture_width = parsed.structured()["capture"]["screenshot"]["width"]
            .as_u64()
            .expect("parsed capture width");
        let capture_height = parsed.structured()["capture"]["screenshot"]["height"]
            .as_u64()
            .expect("parsed capture height");
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux-x11"
        };
        let (mode, chooser_provider) = match &gate.choice {
            ChoiceConfig::Mock => ("mock", "fixture"),
            ChoiceConfig::Live { .. } => ("live", "typesafe"),
        };
        let raw = json!({
            "schema": "cua-visual-perception-demo-raw/v2",
            "source_sha": gate.source_sha,
            "jev_source_sha": gate.jev_source_sha,
            "platform": platform,
            "capture_ids": {"acted": capture_id, "fresh": second_capture_id},
            "observation": {
                "input_scope": scope.capture_scope(), "capture_kind": scope.capture_kind(),
                "capture_source": "driver-screenshot", "width": capture_width,
                "height": capture_height, "desktop_session": gate.desktop_session,
                "runner_identity_class": gate.runner_identity_class,
                "delivery_mode": scope.delivery_mode()
            },
            "fixture_oracle": oracle,
            "extension_status": extension_status,
            "parser": parser,
            "chooser": {"mode": mode, "request": request, "response": choice_response},
            "resolved_action": {"candidate_id": selected_candidate.id, "x": choice.x, "y": choice.y},
            "verification": {
                "oracle": "passed",
                "background_desktop_refused": background_desktop_refused,
                "capture_preserved_after_refusal": capture_preserved_after_refusal,
                "stale_capture_refused": true
            },
            "timeline": {"duration_ms": duration_ms, "events": [
                "observed", "parsed", "chosen",
                if background_desktop_refused == Some(true) { "background_desktop_refused" } else { "background_refusal_not_applicable" },
                "clicked", "oracle_verified", "stale_capture_refused", "reobserved"
            ]},
            "recording": {"local_path": recording.to_string_lossy(), "sha256": recording_sha256,
                "metadata": recording_metadata}
        });
        let raw_bytes = serde_json::to_vec_pretty(&raw).expect("serialize raw evidence");
        let raw_sha256 = hash_bytes(&raw_bytes);
        fs::write(evidence_dir.join("raw-manifest.json"), &raw_bytes).expect("write raw evidence");
        fs::write(
            evidence_dir.join("timeline.json"),
            serde_json::to_vec_pretty(&raw["timeline"]).expect("serialize private timeline"),
        )
        .expect("write private timeline evidence");

        let manifest = json!({
            "schema": "cua-visual-perception-demo-evidence/v3",
            "platform": platform,
            "raw_evidence_sha256": raw_sha256,
            "fixture": {"id": "visual-only-canvas/v1", "oracle": {
                "source": "fixture-journal", "initial_ready": true,
                "selected": oracle["selected"], "action_count": oracle["action_count"],
                "result": "passed"
            }},
            "runtime": {
                "driver": {
                    "source_sha": gate.source_sha,
                    "version": candidate_measurements["review_driver_version"],
                    "binary_sha256": driver_binary_sha256,
                    "build": {
                        "profile": candidate_measurements["review_driver_build_profile"],
                        "target": candidate_measurements["target"],
                        "sealed_artifact_manifest_sha256": candidate_measurements["sealed_artifact_manifest_sha256"],
                        "sealed_extension_manifest_sha256": candidate_measurements["sealed_extension_manifest_sha256"]
                    }
                },
                "perception": {
                    "extension_id": "cua-perception",
                    "extension_version": extension_status["active_version"],
                    "trust": "review-only-publisher-verified",
                    "publisher_id": extension_status["publisher_id"],
                    "publisher_key_id": extension_status["publisher_key_id"],
                    "catalog_version": extension_status["catalog_version"],
                    "signature_algorithm": "ed25519",
                    "signed_extension_archive_sha256": signed_extension_archive_sha256,
                    "signing_key_sha256": signing_key_sha256,
                    "signed_catalog_sha256": signed_catalog_sha256,
                    "protocol_version": candidate_measurements["protocol_version"],
                    "worker_sha256": candidate_measurements["worker_sha256"],
                    "self_test": candidate_measurements["self_test"],
                    "models": candidate_measurements["models"],
                    "onnx_runtime": candidate_measurements["onnx_runtime"],
                    "parser_model_id": parser["model_id"]
                },
                "chooser": {
                    "mode": mode,
                    "provider": chooser_provider,
                    "model_id": choice_response.model,
                    "adapter_source_sha": gate.source_sha,
                    "source_sha": gate.jev_source_sha
                }
            },
            "observation": {
                "session_label": action_session,
                "input_scope": scope.capture_scope(), "capture_kind": scope.capture_kind(),
                "capture_source": "driver-screenshot",
                "dimensions": {"width": capture_width, "height": capture_height},
                "acted_capture_id_sha256": hash_bytes(capture_id.as_bytes()),
                "fresh_capture_id_sha256": hash_bytes(second_capture_id.as_bytes()),
                "capture_trace_sha256": hash_bytes(format!("{capture_id}\0{second_capture_id}").as_bytes()),
                "candidates": raw["chooser"]["request"]["candidates"]
            },
            "environment": {
                "os": {"name": gate.os_name, "version": gate.os_version, "arch": gate.os_arch},
                "desktop_session": gate.desktop_session,
                "runner_identity_class": gate.runner_identity_class,
                "delivery_mode": scope.delivery_mode()
            },
            "result": {
                "status": "passed",
                "selected_candidate": selected_candidate.id,
                "background_desktop_refused": background_desktop_refused,
                "capture_preserved_after_refusal": capture_preserved_after_refusal,
                "stale_capture_refused": true
            },
            "recording": {
                "original_dimensions": {"width": recording_metadata["width"], "height": recording_metadata["height"]},
                "delivered_dimensions": {"width": recording_metadata["width"], "height": recording_metadata["height"]},
                "frame_rate": recording_metadata["frame_rate"],
                "cursor": {"agent_overlay": false, "system_cursor": "recorder-default"},
                "edit_operations": [{"operation": "none", "speed": "1x"}],
                "shots": [{"source_sha256": recording_sha256, "start_ms": 0,
                    "end_ms": recording_metadata["duration_ms"]}],
                "final_sha256": recording_sha256, "size_bytes": recording_size
            },
            "artifacts": [{"kind": "video", "path": "recording.mp4", "sha256": recording_sha256, "size_bytes": recording_size}]
        });
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../tests/perception-demo/evidence-manifest.schema.json"
        ))
        .expect("parse evidence schema");
        assert!(
            jsonschema::is_valid(&schema, &manifest),
            "redacted manifest must satisfy schema"
        );
        fs::write(
            evidence_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .expect("write redacted manifest");
    }

    #[test]
    fn fixture_discovery_requires_exact_title_and_owner() {
        let windows = json!([
            {"window_id": 10, "pid": 42, "title": "Cua Visual-Only Canvas Fixture - stale"},
            {"window_id": 11, "pid": 41, "title": FIXTURE_TEST_TITLE},
            {"window_id": 12, "pid": 42, "title": FIXTURE_TEST_TITLE}
        ]);
        assert_eq!(
            exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE).unwrap(),
            Some((12, FIXTURE_TEST_TITLE.to_owned()))
        );
    }

    #[test]
    fn fixture_discovery_refuses_ambiguous_owner_matches() {
        let windows = json!([
            {"window_id": 10, "pid": null, "title": FIXTURE_TEST_TITLE},
            {"window_id": 11, "pid": null, "title": FIXTURE_TEST_TITLE}
        ]);
        assert!(exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE)
            .unwrap_err()
            .contains("ambiguous fixture identity"));
    }

    #[test]
    fn fixture_discovery_retries_one_unique_title_with_missing_pid() {
        let windows = json!([
            {"window_id": 10, "pid": null, "title": FIXTURE_TEST_TITLE}
        ]);
        assert_eq!(
            exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE).unwrap(),
            None
        );
        let diagnostic = fixture_window_diagnostic(&windows, 42, FIXTURE_TEST_TITLE);
        assert!(diagnostic.contains("expected pid 42"));
        assert!(diagnostic.contains("pid=missing"));
    }

    #[test]
    fn fixture_discovery_rejects_explicit_mismatched_pid() {
        let windows = json!([
            {"window_id": 10, "pid": 41, "title": FIXTURE_TEST_TITLE}
        ]);
        assert!(exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE)
            .unwrap_err()
            .contains("not expected pid 42"));
    }

    #[test]
    fn fixture_discovery_refuses_zero_window_id() {
        let windows = json!([
            {"window_id": 0, "pid": 42, "title": FIXTURE_TEST_TITLE}
        ]);
        assert!(exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE)
            .unwrap_err()
            .contains("invalid window_id"));
    }

    #[test]
    fn fixture_discovery_refuses_missing_or_non_array_windows() {
        for windows in [Value::Null, json!({"window_id": 10})] {
            assert!(exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE)
                .unwrap_err()
                .contains("omitted its windows array"));
        }
    }

    #[test]
    fn fixture_discovery_accepts_maximum_window_id() {
        let windows = json!([
            {"window_id": u64::MAX, "pid": 42, "title": FIXTURE_TEST_TITLE}
        ]);
        assert_eq!(
            exact_fixture_window(&windows, 42, FIXTURE_TEST_TITLE).unwrap(),
            Some((u64::MAX, FIXTURE_TEST_TITLE.to_owned()))
        );
    }

    #[test]
    #[ignore = "requires an installed review-only publisher-verified perception extension and desktop session"]
    fn authorized_visual_only_window_demo() {
        run_authorized_visual_only_demo(DemoScope::Window);
    }

    #[test]
    #[ignore = "requires an installed review-only publisher-verified perception extension and desktop session"]
    fn authorized_visual_only_primary_desktop_demo() {
        run_authorized_visual_only_demo(DemoScope::PrimaryDesktop);
    }
}
