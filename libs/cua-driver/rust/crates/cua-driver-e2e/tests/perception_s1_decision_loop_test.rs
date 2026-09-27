//! Canonical desktop row for the model-backed perception decision loop.
//!
//! The row drives the complete loop that `jev-use` documents for a local
//! decision model, with real models at every step:
//!
//! 1. Cua Driver captures the Electron web harness window and retains the
//!    capture (`get_window_state` -> `capture_id`).
//! 2. The published, publisher-verified `cua-perception` extension, installed
//!    from its signed release catalog, parses that exact capture with
//!    OmniParser (`parse_visual_regions`).
//! 3. Cua-S1-4B, loaded once from the pinned public weights and kept resident
//!    (`libs/cua-s1/ci/warm_chooser.py`), chooses one closed candidate built
//!    from the parsed text regions.
//! 4. Driver clicks the chosen region's center once with the same
//!    `capture_id`, inside the 60-second capture lifetime.
//! 5. The fixture's loopback journal, which the Driver never reads, proves the
//!    effect (`counter=1`).
//! 6. Stale captures are refused without effect: the consumed capture returns
//!    `capture_not_found`, and an unused capture older than the lifetime
//!    returns `capture_expired`. The counter stays at 1.
//!
//! The row needs about 10 GB of free memory for the model, so it runs only in
//! the weights-backed Linux X11 lane (`.github/workflows/ci-cua-s1-weights.yml`
//! through `scripts/ci/linux/run-rust-e2e.sh` with the `s1-perception` lane).
//! That lane supplies:
//!
//! - `CUA_E2E_PERCEPTION_CATALOG`: the release catalog JSON, with its archive
//!   next to it;
//! - `CUA_E2E_PERCEPTION_VERSION`: the extension version the catalog names;
//! - `CUA_E2E_S1_PYTHON`: a Python with the `cua-s1[four-b]` environment;
//! - `S1_BASE_MODEL_PATH`, `S1_ADAPTER_PATH`, `S1_DEVICE`, `S1_DTYPE`, and
//!   optionally `S1_MODALITY`, exactly as the jev-use chooser reads them.
//!
//! ```text
//! cargo test -p cua-driver-e2e --test perception_s1_decision_loop_test -- \
//!   --ignored --nocapture --test-threads=1
//! ```

#![cfg(target_os = "linux")]

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, shared_web_route, CaseSpec, Delivery, DisplayServer,
    Evidence, Observation, OracleKind, Platform, Scope, Targeting,
};
use cua_driver_testkit::{
    driver_binary, harness_app, spawn_in_job, Driver, FixtureJournal, McpDriver, ToolResponse,
};
use serde_json::{json, Value};

const FIXTURE_TITLE: &str = "CuaTestHarness Electron";
const GOAL: &str = "Increment the counter once.";
/// Driver's capture lifetime (`CaptureRegistryConfig::default().ttl`).
const CAPTURE_TTL: Duration = Duration::from_secs(60);
/// S1 scores one letter per option (at most 26 with reobserve/abstain). Each
/// offered region lengthens the single prefill pass. On a 4-vCPU hosted runner
/// with AVX512-BF16, 16 regions took 20 to 38 s per decision while the row
/// recorded video, so 12 keep parse + decision + click well inside the
/// capture lifetime.
const MAX_REGION_CANDIDATES: usize = 12;
const S1_READY_TIMEOUT: Duration = Duration::from_secs(900);
const S1_DECISION_TIMEOUT: Duration = Duration::from_secs(120);

fn required_env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} is required by the S1 perception row"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("repository root above crates/cua-driver-e2e")
        .to_path_buf()
}

fn evidence_dir() -> PathBuf {
    let root = std::env::var_os("CUA_E2E_RESULTS_FILE")
        .map(PathBuf::from)
        .and_then(|results| results.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let directory = root.join("s1-perception");
    fs::create_dir_all(&directory).expect("create S1 perception evidence directory");
    directory
}

fn write_evidence(name: &str, value: &Value) {
    let path = evidence_dir().join(name);
    fs::write(&path, serde_json::to_vec_pretty(value).unwrap())
        .unwrap_or_else(|error| panic!("write {path:?}: {error}"));
}

// ---------------------------------------------------------------- S1

/// One resident Cua-S1-4B chooser speaking JSON lines over stdio.
struct WarmChooser {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    ready: Value,
}

impl WarmChooser {
    fn start() -> Self {
        let python = required_env("CUA_E2E_S1_PYTHON");
        for name in ["S1_BASE_MODEL_PATH", "S1_ADAPTER_PATH"] {
            required_env(name);
        }
        let script = repo_root().join("libs/cua-s1/ci/warm_chooser.py");
        let mut child = Command::new(python)
            .arg(&script)
            .arg("--warmup")
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|error| panic!("start {script:?}: {error}"));
        let stdin = child.stdin.take().expect("chooser stdin");
        let stdout = child.stdout.take().expect("chooser stdout");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let started = Instant::now();
        let line = lines
            .recv_timeout(S1_READY_TIMEOUT)
            .unwrap_or_else(|_| panic!("S1 chooser was not ready within {S1_READY_TIMEOUT:?}"));
        let ready: Value = serde_json::from_str(&line).expect("S1 ready line is JSON");
        assert_eq!(ready["ready"], true, "S1 chooser failed to start: {ready}");
        eprintln!(
            "[s1-perception] chooser ready in {:.1}s: {ready}",
            started.elapsed().as_secs_f64()
        );
        Self {
            child,
            stdin,
            lines,
            ready,
        }
    }

    fn decide(&mut self, request: &Value, screenshot: Option<&Path>) -> (Value, u64) {
        let message = json!({"request": request, "screenshot": screenshot});
        writeln!(self.stdin, "{message}").expect("send request to S1 chooser");
        self.stdin.flush().expect("flush S1 chooser request");
        let line = self
            .lines
            .recv_timeout(S1_DECISION_TIMEOUT)
            .unwrap_or_else(|_| panic!("S1 decision took longer than {S1_DECISION_TIMEOUT:?}"));
        let reply: Value = serde_json::from_str(&line).expect("S1 reply is JSON");
        assert!(reply.get("error").is_none(), "S1 chooser refused: {reply}");
        let latency = reply["latency_ms"].as_u64().expect("S1 latency_ms");
        (reply["decision"].clone(), latency)
    }
}

impl Drop for WarmChooser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------- extension

fn run_driver_cli(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(driver_binary())
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdin(Stdio::null())
        .output()
        .expect("run cua-driver")
}

/// Install the published extension from its signed release catalog, the
/// documented user path, and require publisher-verified trust.
fn install_published_extension(home: &Path) -> Value {
    let catalog = required_env("CUA_E2E_PERCEPTION_CATALOG");
    let version = required_env("CUA_E2E_PERCEPTION_VERSION");
    let install = run_driver_cli(
        home,
        &[
            "extension",
            "install",
            "cua-perception",
            "--catalog",
            &catalog,
        ],
    );
    assert!(
        install.status.success(),
        "published extension install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    let status = run_driver_cli(home, &["extension", "status", "cua-perception", "--json"]);
    assert!(
        status.status.success(),
        "extension status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("extension status JSON");
    assert_eq!(status["installed"], true, "extension status: {status}");
    assert_eq!(
        status["trust"], "publisher-verified",
        "extension status: {status}"
    );
    assert_eq!(
        status["active_version"], version,
        "extension status: {status}"
    );
    status
}

// ---------------------------------------------------------------- fixture

struct Fixture {
    pid: u32,
    window_id: u64,
    journal: FixtureJournal,
}

fn launch_electron(driver: &mut McpDriver) -> Fixture {
    let journal = FixtureJournal::start();
    let cdp_port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("allocate a loopback CDP port")
        .port();
    let window_ids = |driver: &mut McpDriver| -> HashSet<u64> {
        driver.call("list_windows", json!({})).structured()["windows"]
            .as_array()
            .map(|windows| {
                windows
                    .iter()
                    .filter_map(|window| window["window_id"].as_u64())
                    .collect()
            })
            .unwrap_or_default()
    };
    let before = window_ids(driver);
    let path = harness_app("harness-electron", "CuaTestHarness.Electron");
    assert!(
        path.exists(),
        "the Electron harness was not staged at {path:?}"
    );
    let mut command = Command::new(path);
    command
        .args([
            "--no-sandbox",
            "--disable-gpu",
            "--force-renderer-accessibility",
        ])
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let child = spawn_in_job(&mut command).expect("launch the Electron harness");
    driver.reaper().push(child);

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let windows = driver.call("list_windows", json!({}));
        let found = windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| {
                window["window_id"]
                    .as_u64()
                    .is_some_and(|id| !before.contains(&id))
                    && window["title"]
                        .as_str()
                        .is_some_and(|title| title.contains(FIXTURE_TITLE))
            })
            .and_then(|window| {
                Some((
                    window["pid"].as_u64()? as u32,
                    window["window_id"].as_u64()?,
                ))
            });
        if let Some((pid, window_id)) = found {
            driver.reaper().track_pid(pid);
            while Instant::now() < deadline {
                if journal.text("lbl-counter").as_deref() == Some("counter=0") {
                    return Fixture {
                        pid,
                        window_id,
                        journal,
                    };
                }
                thread::sleep(Duration::from_millis(100));
            }
            panic!("Electron harness page never published its initial state");
        }
        thread::sleep(Duration::from_millis(250));
    }
    panic!("Electron harness window did not appear");
}

fn capture_window(driver: &mut McpDriver, fixture: &Fixture) -> (String, Instant) {
    let state = driver.call(
        "get_window_state",
        json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.window_id,
            "include_accessibility_tree": false
        }),
    );
    let captured_at = Instant::now();
    assert!(
        !state.is_error(),
        "get_window_state failed: {}",
        state.text()
    );
    let capture_id = state.structured()["capture_id"]
        .as_str()
        .unwrap_or_else(|| panic!("window observation omitted capture_id: {}", state.text()))
        .to_owned();
    (capture_id, captured_at)
}

fn parse_regions(driver: &mut McpDriver, capture_id: &str) -> ToolResponse {
    driver.call(
        "parse_visual_regions",
        json!({
            "capture_id": capture_id,
            "options": {"kinds": ["text", "icon"], "min_confidence": 0.3, "max_regions": 100}
        }),
    )
}

fn validated_parse(
    parsed: &ToolResponse,
    fixture: &Fixture,
    capture_id: &str,
    version: &str,
) -> Value {
    assert!(
        !parsed.is_error(),
        "parse_visual_regions failed: {}; structured={}",
        parsed.text(),
        parsed.structured()
    );
    let result = parsed.structured().clone();
    assert_eq!(result["schema"], "cua.visual_regions_v1");
    assert_eq!(result["capture"]["capture_id"], capture_id);
    assert_eq!(result["capture"]["source"]["kind"], "window");
    assert_eq!(result["capture"]["source"]["pid"], fixture.pid);
    assert_eq!(result["capture"]["source"]["window_id"], fixture.window_id);
    assert_eq!(result["parser"]["extension_id"], "cua-perception");
    assert_eq!(result["parser"]["extension_version"], version);
    assert_eq!(
        result["capture"]["action_coordinate_space"]["kind"], "screenshot_pixels",
        "region centers are clicked in screenshot pixels: {}",
        result["capture"]
    );
    result
}

fn center(region: &Value) -> (f64, f64) {
    let bounds = &region["bounds"];
    let value = |key: &str| bounds[key].as_f64().expect("numeric region bounds");
    (
        value("x") + value("width") / 2.0,
        value("y") + value("height") / 2.0,
    )
}

/// Build a `cua.jev_choice_request_v1` from parsed regions with the caller
/// policy jev-use documents: every candidate is a supplied region, plus
/// `reobserve` and `abstain`. Text regions that carry letters are offered in
/// reading order; the request carries exactly those regions.
fn choice_request(parse: &Value, capture_id: &str) -> (Value, Vec<Value>) {
    let mut texts: Vec<Value> = parse["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|region| region["kind"] == "text")
        .filter(|region| {
            region["text"]
                .as_str()
                .is_some_and(|text| text.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        })
        .cloned()
        .collect();
    texts.sort_by(|left, right| {
        let key = |region: &Value| {
            (
                region["bounds"]["y"].as_i64().unwrap_or(0) / 12,
                region["bounds"]["x"].as_i64().unwrap_or(0),
            )
        };
        key(left).cmp(&key(right))
    });
    texts.truncate(MAX_REGION_CANDIDATES);
    let regions: Vec<Value> = texts
        .iter()
        .map(|region| {
            json!({
                "id": region["id"],
                "kind": "text",
                "bounds": region["bounds"],
                "text": region["text"],
                "confidence": region["confidence"],
                "interactive": region["interactive"],
            })
        })
        .collect();
    let mut candidates: Vec<Value> = texts
        .iter()
        .map(|region| {
            let (x, y) = center(region);
            let text: String = region["text"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect();
            json!({
                "id": format!("click-{}", region["id"].as_str().unwrap_or("region")),
                "description": format!("Click the text region {text:?} at ({x:.0},{y:.0})."),
            })
        })
        .collect();
    candidates.push(json!({
        "id": "reobserve",
        "description": "Discard this decision set and obtain a fresh observation."
    }));
    candidates.push(json!({
        "id": "abstain",
        "description": "Stop without acting if no supplied action is safe."
    }));
    (
        json!({
            "schema": "cua.jev_choice_request_v1",
            "goal": GOAL,
            "capture_id": capture_id,
            "regions": regions,
            "history": [],
            "candidates": candidates,
        }),
        texts,
    )
}

fn wait_for_counter(journal: &FixtureJournal, expected: &str, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if journal.text("lbl-counter").as_deref() == Some(expected) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn click_args(fixture: &Fixture, x: f64, y: f64, capture_id: &str) -> Value {
    json!({
        "pid": fixture.pid as i64,
        "window_id": fixture.window_id,
        "x": x,
        "y": y,
        "capture_id": capture_id,
        "delivery_mode": "foreground"
    })
}

fn assert_refused(response: &ToolResponse, code: &str, what: &str) {
    assert!(
        response.is_error(),
        "{what} authorized a click: {}",
        response.text()
    );
    assert_eq!(
        response.structured()["code"],
        code,
        "{what} used an unexpected refusal: {}",
        response.structured()
    );
    assert_eq!(response.structured()["effect"], "refused", "{what}");
}

// ---------------------------------------------------------------- row

/// S1 chooses among OmniParser regions of a live capture; Driver clicks the
/// choice with that capture inside its lifetime; the fixture journal verifies
/// the effect; consumed and expired captures are refused without effect.
#[test]
#[ignore]
fn s1_chooses_a_parsed_region_and_the_capture_bound_click_is_state_verified() {
    let route = shared_web_route(
        Platform::current(),
        DisplayServer::current(),
        "left_click",
        Targeting::Px,
        Delivery::Foreground,
    )
    .expect("foreground pixel click route");
    let case = CaseSpec::delivered(
        format!(
            "{}-electron-perception-s1-capture-click-px-foreground",
            std::env::consts::OS
        ),
        "electron",
        "electron",
        "perception_s1_capture_click",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Window,
        route,
        vec![OracleKind::FixtureState, OracleKind::Protocol],
    );
    let cell_id = case.cell_id.clone();
    execute_case(case, |evidence| {
        let version = required_env("CUA_E2E_PERCEPTION_VERSION");
        // Load and warm the model before any capture exists, like a live
        // agent would; its lifetime starts at the capture, not at startup.
        let mut chooser = WarmChooser::start();

        let home = tempfile::Builder::new()
            .prefix("cua-perception-s1-home-")
            .tempdir()
            .expect("create isolated extension home");
        let status = install_published_extension(home.path());
        let home_str = home.path().to_str().expect("UTF-8 extension home");
        let mut driver =
            McpDriver::spawn_named_with_env(&cell_id, &[("CUA_DRIVER_RS_HOME", home_str)])
                .expect("start the source-built Driver with the extension home");
        *evidence = recording_evidence(driver.recording_dir());
        let fixture = launch_electron(&mut driver);
        let raised = driver.call(
            "bring_to_front",
            json!({"pid": fixture.pid as i64, "window_id": fixture.window_id}),
        );
        eprintln!("[s1-perception] bring_to_front: {}", raised.text());
        thread::sleep(Duration::from_millis(500));

        // Warm the extension worker, and keep this capture unused so it can
        // prove expiry at the end.
        let (expiring_capture, expiring_at) = capture_window(&mut driver, &fixture);
        let warm_started = Instant::now();
        let warm = parse_regions(&mut driver, &expiring_capture);
        validated_parse(&warm, &fixture, &expiring_capture, &version);
        let warm_parse_ms = warm_started.elapsed().as_millis() as u64;
        driver.start_behavior_recording();

        // 1. Observe and retain the native capture.
        let (capture_id, captured_at) = capture_window(&mut driver, &fixture);
        // 2. The published extension parses that exact capture.
        let parse_started = Instant::now();
        let parsed = parse_regions(&mut driver, &capture_id);
        let parse_ms = parse_started.elapsed().as_millis() as u64;
        let parse = validated_parse(&parsed, &fixture, &capture_id, &version);
        assert_eq!(
            fixture.journal.text("lbl-counter").as_deref(),
            Some("counter=0"),
            "parsing must not change fixture state"
        );
        // 3. S1 chooses one closed candidate.
        let (request, offered) = choice_request(&parse, &capture_id);
        write_evidence("parse.json", &parse);
        write_evidence("request.json", &request);
        let (decision, s1_ms) = chooser.decide(&request, None);
        write_evidence("decision.json", &decision);
        let decided_age = captured_at.elapsed();
        assert_eq!(decision["schema"], "cua.decision_choice_v1", "{decision}");
        assert_eq!(decision["capture_id"], capture_id, "{decision}");
        assert_eq!(
            decision["kind"], "selected",
            "S1 did not select an action: {decision}"
        );
        let selected = decision["selected_id"].as_str().expect("selected_id");
        let region = offered
            .iter()
            .find(|region| format!("click-{}", region["id"].as_str().unwrap_or("")) == selected)
            .unwrap_or_else(|| panic!("S1 selected an unknown candidate: {decision}"));
        let (x, y) = center(region);

        // 4. One capture-bound click inside the capture lifetime.
        let click_age = captured_at.elapsed();
        assert!(
            click_age < CAPTURE_TTL,
            "the loop took {click_age:?}, beyond the {CAPTURE_TTL:?} capture lifetime"
        );
        let args = click_args(&fixture, x, y, &capture_id);
        let click_started = Instant::now();
        let click = driver.call("click", args.clone());
        let click_ms = click_started.elapsed().as_millis() as u64;
        assert!(
            !click.is_error(),
            "capture-bound click failed: {}; structured={}",
            click.text(),
            click.structured()
        );
        // 5. The independent fixture oracle.
        let delivered = wait_for_counter(&fixture.journal, "counter=1", Duration::from_secs(10));
        let timing = json!({
            "cell_id": cell_id,
            "extension": {"version": status["active_version"], "trust": status["trust"]},
            "parser": parse["parser"],
            "s1": chooser.ready,
            "goal": GOAL,
            "regions_parsed": parse["regions"].as_array().map(Vec::len),
            "candidates": request["candidates"].as_array().map(Vec::len),
            "selected_id": selected,
            "selected_text": region["text"],
            "confidence": decision["confidence"],
            "warm_parse_ms": warm_parse_ms,
            "parse_ms": parse_ms,
            "s1_decision_ms": s1_ms,
            "capture_age_at_decision_ms": decided_age.as_millis() as u64,
            "capture_age_at_click_ms": click_age.as_millis() as u64,
            "click_ms": click_ms,
            "click_route": click.action_route(),
            "fixture_counter": fixture.journal.text("lbl-counter"),
        });
        write_evidence("timing.json", &timing);
        eprintln!("[s1-perception] {timing}");
        assert!(
            delivered,
            "S1 chose {selected} ({:?}) but the fixture never reached counter=1: {}",
            region["text"],
            fixture.journal.snapshot()
        );

        // 6a. The consumed capture is refused.
        let reused = driver.call("click", args);
        assert_refused(&reused, "capture_not_found", "a consumed capture");
        // 6b. An unused capture past its lifetime is refused.
        let remaining = CAPTURE_TTL.saturating_sub(expiring_at.elapsed());
        thread::sleep(remaining + Duration::from_secs(2));
        let expired = driver.call("click", click_args(&fixture, x, y, &expiring_capture));
        assert_refused(&expired, "capture_expired", "an expired capture");
        thread::sleep(Duration::from_millis(750));
        assert_eq!(
            fixture.journal.text("lbl-counter").as_deref(),
            Some("counter=1"),
            "a refused stale capture changed the fixture: {}",
            fixture.journal.snapshot()
        );
        Observation::delivered(
            vec![OracleKind::FixtureState, OracleKind::Protocol],
            Evidence::default(),
        )
    });
}

/// The request builder offers only letter-bearing text regions, in reading
/// order, capped for S1's 26-letter option space, plus reobserve/abstain.
#[test]
fn choice_request_offers_bounded_text_regions_in_reading_order() {
    let region = |id: &str, kind: &str, text: &str, x: i64, y: i64| {
        json!({"id": id, "kind": kind, "text": text, "confidence": 0.9, "interactive": false,
               "bounds": {"x": x, "y": y, "width": 40, "height": 20}})
    };
    let mut regions = vec![
        region("text-2", "text", "Reset", 200, 101),
        region("text-1", "text", "Increment", 100, 100),
        region("text-3", "text", "0", 300, 100),
        region("icon-1", "icon", "", 10, 10),
        region("text-4", "text", "Heading", 20, 20),
    ];
    for index in 0..40 {
        regions.push(region(
            &format!("text-x{index}"),
            "text",
            "Filler",
            10,
            500 + index * 30,
        ));
    }
    let (request, offered) = choice_request(&json!({"regions": regions}), "capture_x");
    let ids: Vec<&str> = request["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["id"].as_str().unwrap())
        .collect();
    assert_eq!(&ids[..3], ["click-text-4", "click-text-1", "click-text-2"]);
    assert_eq!(ids.len(), MAX_REGION_CANDIDATES + 2);
    assert_eq!(&ids[ids.len() - 2..], ["reobserve", "abstain"]);
    assert_eq!(offered.len(), MAX_REGION_CANDIDATES);
    assert_eq!(
        request["regions"].as_array().unwrap().len(),
        MAX_REGION_CANDIDATES
    );
    assert_eq!(request["capture_id"], "capture_x");
    assert_eq!(
        request["candidates"][1]["description"],
        "Click the text region \"Increment\" at (120,110)."
    );
}
