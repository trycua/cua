//! Artifact-gated, offline perception demo for a custom-painted surface.

use serde_json::{json, Value};

const MAX_REGIONS: usize = 64;
const MIN_CONFIDENCE: f64 = 0.80;

#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    id: String,
    label: String,
    capture_id: String,
    x: f64,
    y: f64,
    confidence: f64,
}

fn bounded_candidates(payload: &Value, capture_id: &str) -> Result<Vec<Candidate>, String> {
    if payload["schema"] != "cua.visual_regions_v1"
        || payload["capture"]["capture_id"].as_str() != Some(capture_id)
    {
        return Err("visual result has invalid schema or capture provenance".into());
    }
    let screenshot = &payload["capture"]["screenshot"];
    let screen_width = screenshot["width"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("invalid screenshot width")?;
    let screen_height = screenshot["height"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("invalid screenshot height")?;
    let regions = payload["regions"]
        .as_array()
        .ok_or("visual result omitted regions")?;
    if regions.len() > MAX_REGIONS {
        return Err("visual result exceeded the candidate bound".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut candidates = Vec::new();
    for region in regions {
        let id = region["id"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .ok_or("region has no ID")?;
        if !ids.insert(id) {
            return Err("duplicate region ID".into());
        }
        let confidence = region["confidence"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .ok_or("invalid confidence")?;
        let bounds = &region["bounds"];
        let x = bounds["x"].as_u64().ok_or("invalid x")?;
        let y = bounds["y"].as_u64().ok_or("invalid y")?;
        let width = bounds["width"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or("invalid width")?;
        let height = bounds["height"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or("invalid height")?;
        if x.checked_add(width).is_none_or(|v| v > screen_width)
            || y.checked_add(height).is_none_or(|v| v > screen_height)
        {
            return Err("region lies outside its source screenshot".into());
        }
        let label = region
            .get("text")
            .or_else(|| region.get("label"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if confidence >= MIN_CONFIDENCE && label.eq_ignore_ascii_case("send") {
            candidates.push(Candidate {
                id: id.into(),
                label: "Send".into(),
                capture_id: capture_id.into(),
                x: x as f64 + width as f64 / 2.0,
                y: y as f64 + height as f64 / 2.0,
                confidence,
            });
        }
    }
    Ok(candidates)
}

fn choose_send(candidates: &[Candidate]) -> Result<&Candidate, String> {
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate.label == "Send");
    let selected = matches.next().ok_or("no bounded Send candidate")?;
    if matches.next().is_some() {
        return Err("Send candidate is ambiguous".into());
    }
    Ok(selected)
}

#[test]
fn mock_choice_is_bounded_capture_bound_and_deterministic() {
    let payload = json!({
        "schema": "cua.visual_regions_v1",
        "capture": {"capture_id": "capture-1", "screenshot": {"width": 760, "height": 460}},
        "regions": [
            {"id": "save", "bounds": {"x": 72, "y": 250, "width": 204, "height": 40}, "text": "Save", "confidence": 0.99},
            {"id": "send", "bounds": {"x": 292, "y": 250, "width": 204, "height": 40}, "text": "Send", "confidence": 0.98}
        ]
    });
    let candidates = bounded_candidates(&payload, "capture-1").unwrap();
    let choice = choose_send(&candidates).unwrap();
    assert_eq!(choice.capture_id, "capture-1");
    assert_eq!((choice.x, choice.y), (394.0, 270.0));
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod e2e {
    use super::{bounded_candidates, choose_send, MAX_REGIONS, MIN_CONFIDENCE};
    use cua_driver_testkit::{spawn_in_job, Driver, FixtureJournal, McpDriver};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const FIXTURE_TITLE: &str = "Cua Visual-Only Canvas Fixture";

    struct Gate {
        source_sha: String,
        extension: PathBuf,
        signature: PathBuf,
        public_key: PathBuf,
        public_key_sha256: String,
        model: PathBuf,
        extension_home: PathBuf,
        evidence_dir: PathBuf,
    }

    fn required(name: &str) -> String {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is required for the artifact-gated demo"))
    }

    fn hash_file(path: &Path) -> String {
        let mut file =
            fs::File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).expect("hash candidate artifact");
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        format!("{:x}", hasher.finalize())
    }

    fn load_gate() -> Gate {
        assert_eq!(required("CUA_JEV_MOCK_DEMO"), "1");
        let gate = Gate {
            source_sha: required("CUA_E2E_SOURCE_SHA"),
            extension: required("CUA_PERCEPTION_EXTENSION_ARCHIVE").into(),
            signature: required("CUA_PERCEPTION_EXTENSION_SIGNATURE").into(),
            public_key: required("CUA_PERCEPTION_TRUSTED_PUBLIC_KEY").into(),
            public_key_sha256: required("CUA_PERCEPTION_TRUSTED_PUBLIC_KEY_SHA256"),
            model: required("CUA_PERCEPTION_MODEL").into(),
            extension_home: required("CUA_PERCEPTION_EXTENSION_HOME").into(),
            evidence_dir: required("CUA_PERCEPTION_EVIDENCE_DIR").into(),
        };
        for path in [
            &gate.extension,
            &gate.signature,
            &gate.public_key,
            &gate.model,
        ] {
            assert!(
                path.is_file(),
                "candidate artifact is not a regular file: {}",
                path.display()
            );
        }
        assert_eq!(
            hash_file(&gate.public_key),
            gate.public_key_sha256,
            "unapproved signing key"
        );
        let status = Command::new("openssl")
            .args(["dgst", "-sha256", "-verify"])
            .arg(&gate.public_key)
            .arg("-signature")
            .arg(&gate.signature)
            .arg(&gate.extension)
            .status()
            .expect("run signature verification");
        assert!(status.success(), "candidate extension signature is invalid");
        gate
    }

    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/apps/cross-platform/visual-only-canvas/main.py")
    }

    fn fixture_command(journal_url: &str) -> Command {
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut c = Command::new("py");
            c.arg("-3");
            c
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
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    #[ignore = "requires an installed signed candidate perception extension and desktop session"]
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
            "authorized-jev-mock-choice-demo",
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
            json!({
                "pid": pid, "window_id": window_id, "capture_mode": "ax"
            }),
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
            .expect("observation must publish capture_id")
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
        let choice = choose_send(
            &bounded_candidates(parsed.structured(), &capture_id)
                .expect("build bounded candidates"),
        )
        .expect("choose Send")
        .clone();

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
            json!({
                "pid": pid, "window_id": window_id, "capture_mode": "ax"
            }),
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
        let elapsed_ms = started.elapsed().as_millis() as u64;
        drop(driver);
        let recording = recording_dir.join("recording.mp4");
        assert!(recording.is_file(), "testkit recording did not finalize");
        fs::create_dir_all(&gate.evidence_dir).expect("create evidence directory");
        let published_recording = gate.evidence_dir.join("recording.mp4");
        fs::copy(&recording, &published_recording).expect("copy decoded recording evidence");
        let recording_size = fs::metadata(&published_recording)
            .expect("measure recording evidence")
            .len();
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux-x11"
        };
        let manifest = json!({
            "schema": "cua-visual-perception-demo-raw/v1", "source_sha": gate.source_sha,
            "platform": platform, "capture_ids": {"acted": capture_id, "fresh": second_capture_id},
            "fixture_oracle": oracle, "parser": parser,
            "candidate": {"id": choice.id, "label": choice.label, "x": choice.x, "y": choice.y, "confidence": choice.confidence},
            "artifacts": {
                "extension_sha256": hash_file(&gate.extension), "model_sha256": hash_file(&gate.model),
                "recording": recording.to_string_lossy(), "recording_sha256": hash_file(&recording)
            }
        });
        let timeline = json!({
            "schema": "cua-visual-perception-demo-timeline/v1", "duration_ms": elapsed_ms,
            "events": ["observed", "parsed", "selected_send", "clicked", "journal_verified", "stale_capture_refused", "reobserved"]
        });
        let redacted_manifest = json!({
            "schema": "cua-visual-perception-demo-evidence/v1",
            "source_sha": gate.source_sha,
            "platform": platform,
            "fixture": {"id": "visual-only-canvas/v1", "oracle": {
                "selected": oracle["selected"], "action_count": oracle["action_count"]
            }},
            "runtime": {
                "adapter": "jev-use",
                "model_id": parser["model_id"].as_str().expect("parser model_id"),
                "model_sha256": hash_file(&gate.model),
                "signed_extension_sha256": hash_file(&gate.extension),
                "signing_key_sha256": gate.public_key_sha256,
                "signature_algorithm": "rsa-sha256"
            },
            "result": {"status": "passed"},
            "artifacts": [{
                "kind": "video", "path": "recording.mp4",
                "sha256": hash_file(&published_recording), "size_bytes": recording_size
            }]
        });
        let evidence_schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/perception-demo/evidence-manifest.schema.json"
        ))
        .expect("parse evidence schema");
        assert!(
            jsonschema::is_valid(&evidence_schema, &redacted_manifest),
            "redacted manifest must satisfy the public evidence schema"
        );
        fs::write(
            gate.evidence_dir.join("raw-manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .expect("write raw manifest");
        fs::write(
            gate.evidence_dir.join("timeline.json"),
            serde_json::to_vec_pretty(&timeline).unwrap(),
        )
        .expect("write timeline");
        fs::write(
            gate.evidence_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&redacted_manifest).unwrap(),
        )
        .expect("write redacted manifest");
    }
}
