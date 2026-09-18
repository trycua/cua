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

#[cfg(any(target_os = "windows", target_os = "linux"))]
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

    const FIXTURE_TITLE: &str = "Cua Visual-Only Canvas Fixture";
    const CHOOSER_TIMEOUT: Duration = Duration::from_secs(30);
    const MAX_CHOOSER_OUTPUT: u64 = 64 * 1024;

    struct Gate {
        source_sha: String,
        model: PathBuf,
        extension_home: PathBuf,
        evidence_dir: PathBuf,
        choice: ChoiceConfig,
    }

    fn required(name: &str) -> String {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is required for the artifact-gated demo"))
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

    fn load_gate() -> Gate {
        let choice = choice_config_from(
            std::env::var("CUA_JEV_LIVE").ok().as_deref(),
            std::env::var("CUA_JEV_MOCK_DEMO").ok().as_deref(),
            std::env::var("CUA_JEV_CHOOSER_PROGRAM").ok().as_deref(),
            std::env::var("CUA_JEV_CHOOSER_SCRIPT").ok().as_deref(),
        )
        .unwrap_or_else(|error| panic!("invalid chooser configuration: {error}"));
        let gate = Gate {
            source_sha: required("CUA_E2E_SOURCE_SHA"),
            model: required("CUA_PERCEPTION_MODEL").into(),
            extension_home: required("CUA_PERCEPTION_EXTENSION_HOME").into(),
            evidence_dir: required("CUA_PERCEPTION_EVIDENCE_DIR").into(),
            choice,
        };
        assert!(gate.model.is_file(), "model is not a regular file");
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
        assert_eq!(status["trust"], "publisher_verified");
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

    fn fixture_command(journal_url: &str) -> Command {
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut command = Command::new("py");
            command.arg("-3");
            command
        };
        #[cfg(target_os = "linux")]
        let mut command = Command::new("python3");
        command
            .arg(fixture_path())
            .args(["--journal-url", journal_url])
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

    fn external_choice(program: &Path, script: &Path, request: &ChoiceRequest) -> ChoiceResponse {
        let mut command = Command::new(program);
        command
            .arg(script)
            .env_clear()
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

    #[test]
    #[ignore = "requires an installed publisher-verified perception extension and desktop session"]
    fn authorized_visual_only_demo() {
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

        let journal = FixtureJournal::start();
        let fixture =
            spawn_in_job(&mut fixture_command(journal.url())).expect("start canvas fixture");
        let pid = i64::from(fixture.id());
        wait_until(
            || journal.snapshot()["ready"].as_bool() == Some(true),
            "fixture did not become ready",
        );
        let extension_home = gate.extension_home.to_string_lossy().into_owned();
        let mut driver = McpDriver::spawn_named_with_env(
            "authorized-jev-choice-demo",
            &[("CUA_DRIVER_RS_HOME", extension_home.as_str())],
        )
        .expect("start Driver with installed candidate extension");
        driver.reaper().push(fixture);
        let (window_id, _) = driver
            .find_window(pid, FIXTURE_TITLE)
            .expect("find canvas fixture");

        let started = Instant::now();
        let first = driver.call(
            "get_window_state",
            json!({"pid": pid, "window_id": window_id, "capture_mode": "ax"}),
        );
        assert!(
            !first.is_error(),
            "initial observation failed: {}",
            first.text()
        );
        for label in ["Save", "Send", "Cancel", "CHOOSE A SIGNAL"] {
            assert!(
                !first.tree_text().contains(label),
                "painted label leaked into AX: {label}"
            );
        }
        let capture_id = first.structured()["capture_id"]
            .as_str()
            .expect("capture_id")
            .to_owned();
        driver.start_behavior_recording();
        let parsed = driver.call("parse_visual_regions", json!({
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
        let choice = selected_candidate
            .action
            .clone()
            .expect("demo chooser must select an action");

        let click_args = json!({
            "pid": pid, "window_id": window_id, "x": choice.x, "y": choice.y,
            "capture_id": choice.capture_id, "delivery_mode": "background"
        });
        let click = driver.call("click", click_args.clone());
        assert!(
            !click.is_error(),
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
        assert!(
            driver.call("click", click_args).is_error(),
            "capture reuse was not refused"
        );

        let second = driver.call(
            "get_window_state",
            json!({"pid": pid, "window_id": window_id, "capture_mode": "ax"}),
        );
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
        fs::create_dir_all(&gate.evidence_dir).expect("create evidence directory");
        let published_recording = gate.evidence_dir.join("recording.mp4");
        fs::copy(&recording, &published_recording).expect("copy decoded recording evidence");
        let recording_sha256 = hash_file(&published_recording);
        let recording_size = fs::metadata(&published_recording)
            .expect("measure recording")
            .len();
        let platform = if cfg!(target_os = "windows") {
            "windows"
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
            "platform": platform,
            "capture_ids": {"acted": capture_id, "fresh": second_capture_id},
            "fixture_oracle": oracle,
            "extension_status": extension_status,
            "parser": parser,
            "chooser": {"mode": mode, "request": request, "response": choice_response},
            "resolved_action": {"candidate_id": selected_candidate.id, "x": choice.x, "y": choice.y},
            "verification": {"oracle": "passed", "stale_capture_refused": true},
            "timeline": {"duration_ms": duration_ms, "events": [
                "observed", "parsed", "chosen", "clicked", "oracle_verified", "stale_capture_refused", "reobserved"
            ]},
            "recording": {"local_path": recording.to_string_lossy(), "sha256": recording_sha256}
        });
        let raw_bytes = serde_json::to_vec_pretty(&raw).expect("serialize raw evidence");
        let raw_sha256 = hash_bytes(&raw_bytes);
        fs::write(gate.evidence_dir.join("raw-manifest.json"), &raw_bytes)
            .expect("write raw evidence");
        fs::write(
            gate.evidence_dir.join("timeline.json"),
            serde_json::to_vec_pretty(&raw["timeline"]).expect("serialize private timeline"),
        )
        .expect("write private timeline evidence");

        let manifest = json!({
            "schema": "cua-visual-perception-demo-evidence/v2",
            "source_sha": gate.source_sha,
            "platform": platform,
            "raw_evidence_sha256": raw_sha256,
            "fixture": {"id": "visual-only-canvas/v1", "oracle": {
                "selected": oracle["selected"], "action_count": oracle["action_count"]
            }},
            "runtime": {
                "perception": {
                    "extension_id": "cua-perception",
                    "extension_version": extension_status["active_version"],
                    "trust": "publisher_verified",
                    "publisher_id": extension_status["publisher_id"],
                    "publisher_key_id": extension_status["publisher_key_id"],
                    "catalog_version": extension_status["catalog_version"],
                    "signature_algorithm": "ed25519",
                    "model_id": parser["model_id"],
                    "model_sha256": hash_file(&gate.model)
                },
                "chooser": {
                    "mode": mode,
                    "provider": chooser_provider,
                    "model_id": choice_response.model
                }
            },
            "result": {"status": "passed", "selected_candidate": selected_candidate.id, "stale_capture_refused": true},
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
            gate.evidence_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .expect("write redacted manifest");
    }
}
