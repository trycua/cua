//! Wayland presentation-timestamp latency evidence on a controlled lane.
//!
//! The Driver already shows when a tool was submitted, when application state
//! changed, and when the tool returned. What it cannot show on Linux/Wayland
//! is when the resulting surface update was actually *presented* by the
//! compositor, which leaves several different causes of a slow action looking
//! identical.
//!
//! This cell joins the two halves of that timeline, in one clock:
//!
//! * the Driver side (`request_started_ns`, `driver_returned_ns`), owned here;
//! * the fixture side (input received, state changed, surface commit,
//!   compositor `presented` or `discarded`), owned by the repository's Wayland
//!   presentation fixture.
//!
//! The point is a falsification boundary. If state and presentation both
//! complete quickly but the Driver returns much later, compositor or scheduler
//! tuning is the wrong intervention. If Driver return is prompt but
//! commit-to-present grows a tail, lower-level tracing becomes justified.
//!
//! Deliberately not here: percentiles from small samples, panel
//! click-to-photon claims, any change to Driver action results, and any
//! cross-compositor performance claim from this one lane.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::{harness_app, Driver, McpDriver};
use serde::Serialize;
use serde_json::{json, Value};

/// Title prefix the fixture publishes; the canonical Sway lane also floats and
/// resizes `CuaTestHarness` windows by this prefix.
const TITLE_PREFIX: &str = "CuaTestHarness Presentation";

/// Presentation-feedback reporting deadline handed to the fixture.
const DEADLINE_MS: u64 = 1_000;

/// Repeats of the measured action. Small on purpose: this cell proves causal
/// attribution, and reporting extreme percentiles from a handful of
/// observations would be misleading.
const REPEATS: usize = 5;

/// How many actions may be issued to collect [`REPEATS`] presented samples. A
/// discarded update is legitimate and retained, but it measures no
/// presentation, so it is repeated rather than counted. The cap keeps a lane
/// that never presents from looping instead of reporting that plainly.
const MAX_ATTEMPTS: usize = REPEATS * 4;

/// `CLOCK_MONOTONIC`, the same clock the fixture stamps its rows with.
fn monotonic_ns() -> u64 {
    let mut stamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` only writes through the provided pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) };
    (stamp.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(stamp.tv_nsec as u64)
}

/// Runner-derived deltas. These are signed: `post_present_wait_ns` is
/// negative exactly when the Driver returned before the compositor presented
/// the update, which is a real and interesting observation rather than an
/// error to clamp away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
struct RowDeltas {
    dispatch_to_app_ns: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_to_commit_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_to_present_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_to_present_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_present_wait_ns: Option<i64>,
    request_to_return_ns: i64,
}

/// One retained raw observation across the presentation boundary.
#[derive(Clone, Debug, Serialize)]
struct EvidenceRow {
    schema: &'static str,
    action: String,
    region: String,
    sequence: u64,
    clock_id: u64,
    request_started_ns: u64,
    fixture_input_received_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    fixture_state_changed_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    surface_commit_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presented_ns: Option<u64>,
    driver_returned_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    presentation: Option<Value>,
    fixture_outcome: String,
    mutated: bool,
    supersede_probe: bool,
    derived: RowDeltas,
}

impl EvidenceRow {
    fn presented_mutation(&self) -> bool {
        self.fixture_outcome == "verified"
    }
}

/// Join one fixture sample with the Driver stamps that bracket it.
fn join(sample: &Value, request_started_ns: u64, driver_returned_ns: u64) -> EvidenceRow {
    let number = |key: &str| sample.get(key).and_then(Value::as_u64);
    let input = number("fixture_input_received_ns").unwrap_or_default();
    let commit = number("surface_commit_ns");
    let presented = number("presented_ns");
    let signed = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    let difference = |later: u64, earlier: u64| signed(later) - signed(earlier);
    let derived = RowDeltas {
        dispatch_to_app_ns: difference(input, request_started_ns),
        app_to_commit_ns: commit.map(|commit| difference(commit, input)),
        commit_to_present_ns: presented
            .and_then(|presented| commit.map(|commit| difference(presented, commit))),
        request_to_present_ns: presented.map(|presented| difference(presented, request_started_ns)),
        post_present_wait_ns: presented
            .map(|presented| difference(driver_returned_ns, presented)),
        request_to_return_ns: difference(driver_returned_ns, request_started_ns),
    };
    EvidenceRow {
        schema: "cua-wayland-presentation-row/v1",
        action: sample
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        region: sample
            .get("region")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        sequence: number("sequence").unwrap_or_default(),
        clock_id: number("clock_id").unwrap_or_default(),
        request_started_ns,
        fixture_input_received_ns: input,
        fixture_state_changed_ns: number("fixture_state_changed_ns"),
        surface_commit_ns: commit,
        presented_ns: presented,
        driver_returned_ns,
        presentation: sample.get("presentation").cloned(),
        fixture_outcome: sample
            .get("fixture_outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        mutated: sample
            .get("mutated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        supersede_probe: sample
            .get("supersede_probe")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        derived,
    }
}

/// Summaries justified by a small sample count: every observation is retained
/// alongside these, and no percentile is reported. Extreme percentiles need
/// far more observations than one fixture run produces.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct Summary {
    rows: usize,
    verified: usize,
    discarded: usize,
    timeout: usize,
    clock_mismatch: usize,
    implausible: usize,
    no_mutation: usize,
    other: usize,
    deadline_ns: u64,
    deadline_misses: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    median_request_to_present_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_request_to_present_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    median_commit_to_present_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_commit_to_present_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    median_post_present_wait_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_post_present_wait_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    median_request_to_return_ns: Option<i64>,
}

/// The lower median of the observed values, so every reported number is a
/// value that was actually measured rather than an interpolation.
fn median(values: &mut Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[(values.len() - 1) / 2])
}

fn summarize(rows: &[EvidenceRow], deadline_ns: u64) -> Summary {
    let mut summary = Summary {
        rows: rows.len(),
        deadline_ns,
        ..Summary::default()
    };
    let mut request_to_present = Vec::new();
    let mut commit_to_present = Vec::new();
    let mut post_present_wait = Vec::new();
    let mut request_to_return = Vec::new();
    for row in rows {
        match row.fixture_outcome.as_str() {
            "verified" => summary.verified += 1,
            "discarded" => summary.discarded += 1,
            "timeout" => summary.timeout += 1,
            "clock_mismatch" => summary.clock_mismatch += 1,
            "implausible" => summary.implausible += 1,
            "no_mutation" => summary.no_mutation += 1,
            _ => summary.other += 1,
        }
        request_to_return.push(row.derived.request_to_return_ns);
        // Only a presented mutation contributes to presentation statistics. A
        // discarded or timed-out update has no presentation time to average.
        if !row.presented_mutation() {
            continue;
        }
        if let Some(value) = row.derived.request_to_present_ns {
            if value > i64::try_from(deadline_ns).unwrap_or(i64::MAX) {
                summary.deadline_misses += 1;
            }
            request_to_present.push(value);
        }
        if let Some(value) = row.derived.commit_to_present_ns {
            commit_to_present.push(value);
        }
        if let Some(value) = row.derived.post_present_wait_ns {
            post_present_wait.push(value);
        }
    }
    summary.max_request_to_present_ns = request_to_present.iter().copied().max();
    summary.max_commit_to_present_ns = commit_to_present.iter().copied().max();
    summary.max_post_present_wait_ns = post_present_wait.iter().copied().max();
    summary.median_request_to_present_ns = median(&mut request_to_present);
    summary.median_commit_to_present_ns = median(&mut commit_to_present);
    summary.median_post_present_wait_ns = median(&mut post_present_wait);
    summary.median_request_to_return_ns = median(&mut request_to_return);
    summary
}

fn artifact_dir() -> PathBuf {
    if let Some(root) = std::env::var_os("CUA_E2E_ARTIFACT_DIR") {
        return PathBuf::from(root);
    }
    if let Some(recordings) = std::env::var_os("CUA_E2E_RECORDINGS_ROOT") {
        if let Some(parent) = Path::new(&recordings).parent() {
            return parent.to_owned();
        }
    }
    std::env::temp_dir()
}

/// Read a JSONL evidence file that the fixture may still be appending to. A
/// trailing partial line is skipped rather than treated as corruption.
fn records(path: &Path) -> Vec<Value> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn samples(path: &Path) -> Vec<Value> {
    records(path)
        .into_iter()
        .filter(|record| record["kind"] == "sample")
        .collect()
}

fn startup(path: &Path) -> Option<Value> {
    records(path)
        .into_iter()
        .find(|record| record["kind"] == "startup")
}

fn state_counter(path: &Path) -> Option<u64> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(contents.trim())
        .ok()?
        .get("counter")
        .and_then(Value::as_u64)
}

/// Wait for the fixture to account for `expected` further samples.
fn await_samples(path: &Path, previous: usize, expected: usize, what: &str) -> Vec<Value> {
    // Generous relative to one refresh interval; a slow compositor should
    // produce a late row, not a missing one.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let rows = samples(path);
        if rows.len() >= previous + expected {
            return rows.into_iter().skip(previous).collect();
        }
        assert!(
            Instant::now() < deadline,
            "{what}: fixture accounted for {} of {} expected samples",
            rows.len().saturating_sub(previous),
            expected
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Fixture {
    pid: u32,
    window_id: u64,
    journal: PathBuf,
    state: PathBuf,
}

impl Fixture {
    /// The region map the fixture is using now.
    ///
    /// The compositor may resize the window after it maps, and the canonical
    /// Sway lane does exactly that: it resizes `CuaTestHarness` windows by
    /// title. The fixture republishes its region map when that happens, so
    /// measuring against the startup map would aim at pixels the regions no
    /// longer occupy.
    fn layout(&self) -> Value {
        records(&self.journal)
            .into_iter()
            .rev()
            .find(|record| record["kind"] == "layout" || record["kind"] == "startup")
            .and_then(|record| record.get("layout").cloned())
            .expect("fixture publishes its region map")
    }

    /// The published center of one region, in the surface-local pixels the
    /// Driver's window-local click frame shares with this undecorated
    /// toplevel.
    fn center(&self, region: &str) -> (i64, i64) {
        let layout = self.layout();
        let rect = &layout[region];
        let value = |key: &str| {
            rect[key]
                .as_i64()
                .unwrap_or_else(|| panic!("fixture layout is missing {region}.{key}"))
        };
        (
            value("x") + value("width") / 2,
            value("y") + value("height") / 2,
        )
    }
}

/// The window's Driver-reported bounds, or `None` while it is not listed.
fn bounds(driver: &mut McpDriver, window_id: u64) -> Option<(i64, i64, i64, i64)> {
    let response = driver.call("list_windows", json!({}));
    let windows = response.structured()["windows"].as_array()?.clone();
    let window = windows
        .iter()
        .find(|window| window["window_id"].as_u64() == Some(window_id))?;
    let read = |key: &str| window[key].as_i64().unwrap_or(0);
    Some((read("x"), read("y"), read("width"), read("height")))
}

/// Wait until the compositor has finished placing the window.
///
/// A tiling compositor maps the surface, then moves and resizes it. During that
/// transaction the Driver can read the container's new origin while the surface
/// is still drawn at the old one, so a window-local click is translated against
/// an origin the surface does not have yet and lands in the wrong region. The
/// measurement would then be of the wrong pixels.
///
/// `click_region` asserts the two frames agree and would fail loudly, but that
/// is a guard against a real defect, not a reason to measure during a move.
/// Wait for two consecutive identical reads before returning.
fn settle_window(driver: &mut McpDriver, window_id: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut previous = bounds(driver, window_id);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let current = bounds(driver, window_id);
        if current.is_some() && current == previous {
            break;
        }
        previous = current;
        assert!(
            Instant::now() < deadline,
            "fixture window bounds never stopped changing; the compositor did \
             not finish placing the window"
        );
    }
    // The bounds are stable; give the compositor the same brief grace the other
    // Linux harness cells use before the first measured action.
    std::thread::sleep(Duration::from_millis(500));
}

fn launch(driver: &mut McpDriver, directory: &Path) -> Fixture {
    let path = harness_app(
        "harness-wayland-presentation",
        "CuaTestHarness.WaylandPresentation",
    );
    assert!(path.exists(), "required fixture is missing: {path:?}");
    let journal = directory.join("fixture-journal.jsonl");
    let state = directory.join("fixture-state.json");
    // A stale journal is refused by the fixture; keep the directory clean so a
    // rerun cannot read a previous run's rows.
    let _ = std::fs::remove_file(&journal);
    let _ = std::fs::remove_file(&state);
    driver
        .reaper()
        .spawn(
            Command::new(&path)
                .arg("--journal")
                .arg(&journal)
                .arg("--state")
                .arg(&state)
                .arg("--deadline-ms")
                .arg(DEADLINE_MS.to_string())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit()),
        )
        .unwrap_or_else(|error| panic!("launch fixture {path:?}: {error}"));

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let response = driver.call("list_windows", json!({}));
        let window = response.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["title"]
                        .as_str()
                        .is_some_and(|title| title.contains(TITLE_PREFIX))
                })
            })
            .cloned();
        if let Some(window) = window {
            let pid = window["pid"].as_u64().unwrap_or(0) as u32;
            let window_id = window["window_id"].as_u64().unwrap_or(0);
            if pid != 0 && window_id != 0 {
                if let Some(record) = startup(&journal) {
                    driver.reaper().track_pid(pid);
                    assert_eq!(
                        record["pid"].as_u64(),
                        Some(u64::from(pid)),
                        "the listed window must belong to the fixture process"
                    );
                    settle_window(driver, window_id);
                    return Fixture {
                        pid,
                        window_id,
                        journal,
                        state,
                    };
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "fixture window {TITLE_PREFIX:?} never appeared"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Click one published region center, bracketing the call with Driver-side
/// monotonic stamps, and return the new fixture rows joined to them.
fn click_region(
    driver: &mut McpDriver,
    fixture: &Fixture,
    region: &str,
    expected_rows: usize,
) -> Vec<EvidenceRow> {
    let (x, y) = fixture.center(region);
    let previous = samples(&fixture.journal).len();
    let request_started_ns = monotonic_ns();
    let response = driver.call(
        "click",
        json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.window_id,
            "x": x,
            "y": y,
            "delivery_mode": "foreground"
        }),
    );
    let driver_returned_ns = monotonic_ns();
    assert!(
        !response.is_error(),
        "click on the {region} region failed: {}",
        response.text()
    );
    let rows = await_samples(&fixture.journal, previous, expected_rows, region);
    assert_eq!(
        rows.len(),
        expected_rows,
        "{region}: one Driver action must account for exactly {expected_rows} content update(s): {rows:?}"
    );
    rows.iter()
        .map(|sample| {
            let row = join(sample, request_started_ns, driver_returned_ns);
            // The Driver's window-local frame and the fixture's surface-local
            // frame must agree, or the measurement is of the wrong pixels.
            assert_eq!(
                row.region, region,
                "the Driver clicked the published {region} center but the fixture \
                 attributed the input to {}; the coordinate frames disagree",
                row.region
            );
            assert!(
                row.derived.dispatch_to_app_ns >= 0,
                "{region}: the fixture cannot receive input before the request started: {row:?}"
            );
            assert_eq!(
                row.clock_id, 1,
                "{region}: fixture rows must be stamped in CLOCK_MONOTONIC"
            );
            row
        })
        .collect()
}

fn write_evidence(directory: &Path, rows: &[EvidenceRow], summary: &Summary) {
    let mut lines = String::new();
    for row in rows {
        lines.push_str(&serde_json::to_string(row).expect("serialize row"));
        lines.push('\n');
    }
    std::fs::write(directory.join("rows.jsonl"), lines).expect("retain raw timing rows");
    std::fs::write(
        directory.join("summary.json"),
        serde_json::to_vec_pretty(summary).expect("serialize summary"),
    )
    .expect("retain summary");
    let optional = |value: Option<i64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_owned())
    };
    let markdown = format!(
        "# Wayland presentation latency\n\n\
         Raw rows: `rows.jsonl` ({} retained).\n\n\
         | Measure | Median (ns) | Max (ns) |\n\
         | --- | ---: | ---: |\n\
         | request -> present | {} | {} |\n\
         | commit -> present | {} | {} |\n\
         | post-present wait | {} | {} |\n\
         | request -> return | {} | - |\n\n\
         Outcomes: verified {}, discarded {}, timeout {}, clock_mismatch {}, \
         implausible {}, no_mutation {}, other {}.\n\n\
         Deadline {} ns, misses {}.\n\n\
         No percentiles are reported: this sample count cannot support them.\n",
        summary.rows,
        optional(summary.median_request_to_present_ns),
        optional(summary.max_request_to_present_ns),
        optional(summary.median_commit_to_present_ns),
        optional(summary.max_commit_to_present_ns),
        optional(summary.median_post_present_wait_ns),
        optional(summary.max_post_present_wait_ns),
        optional(summary.median_request_to_return_ns),
        summary.verified,
        summary.discarded,
        summary.timeout,
        summary.clock_mismatch,
        summary.implausible,
        summary.no_mutation,
        summary.other,
        summary.deadline_ns,
        summary.deadline_misses,
    );
    std::fs::write(directory.join("summary.md"), markdown).expect("retain summary markdown");
}

#[test]
#[ignore]
fn wayland_presentation_feedback_attributes_action_latency_across_the_boundary() {
    let case = CaseSpec::delivered(
        "linux-wayland-presentation-click-px-foreground",
        "wayland-presentation",
        "wayland-presentation",
        "click",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::LinuxWaylandVirtualPointer,
        vec![OracleKind::FixtureState],
    );
    execute_case(case, |evidence| {
        assert!(
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
            "this cell requires a native Wayland session"
        );
        let directory = artifact_dir().join("wayland-presentation");
        std::fs::create_dir_all(&directory).expect("artifact directory");

        let mut driver = McpDriver::spawn_named("linux-wayland-presentation-latency")
            .expect("start source-built Linux driver");
        *evidence = recording_evidence(driver.recording_dir());
        let fixture = launch(&mut driver, &directory);
        driver.start_behavior_recording();

        let startup_record = startup(&fixture.journal).expect("fixture startup record");
        // The clock domain is recorded, not assumed. A foreign presentation
        // clock is a legitimate observation; silently comparing across clocks
        // would not be.
        let presentation_clock = startup_record["presentation_clock_id"].as_u64();
        assert_eq!(
            presentation_clock,
            Some(1),
            "the compositor must advertise CLOCK_MONOTONIC presentation for this \
             lane's deltas to be comparable; advertised: {presentation_clock:?}"
        );
        assert_eq!(state_counter(&fixture.state), Some(0));

        let mut rows = Vec::new();

        // 1. The measured action: one click, one content update, presented.
        //
        //    A compositor may legitimately never show an update: when a later
        //    commit supersedes it within the same refresh, the earlier one is
        //    discarded. That is correct compositor behaviour and it is retained
        //    as evidence, but it is not a presentation-latency sample, so it
        //    does not count towards the measured set. Only failing to gather
        //    the samples at all is a failure of the lane.
        let mut verified = 0usize;
        let mut attempt = 0usize;
        while verified < REPEATS {
            attempt += 1;
            assert!(
                attempt <= MAX_ATTEMPTS,
                "only {verified} of {REPEATS} actions reached a presented content \
                 update within {MAX_ATTEMPTS} attempts; this lane cannot attribute \
                 a presentation to an action"
            );
            let counter_before = state_counter(&fixture.state).expect("fixture state");
            let measured = click_region(&mut driver, &fixture, "active", 1);
            let row = measured.into_iter().next().expect("one row");
            // Independently readable application state, on a channel the timing
            // rows do not write. It advances for every delivered action,
            // whether or not the compositor went on to present that update.
            assert_eq!(
                state_counter(&fixture.state),
                Some(counter_before + 1),
                "attempt {attempt}: one Driver action must change fixture state exactly once"
            );
            if !row.presented_mutation() {
                assert_eq!(
                    row.fixture_outcome, "discarded",
                    "attempt {attempt}: an update that was not presented must be typed \
                     as discarded, never reported as a presented mutation: {row:?}"
                );
                assert!(
                    row.presented_ns.is_none() && row.derived.commit_to_present_ns.is_none(),
                    "attempt {attempt}: a discarded update must carry no presentation \
                     time: {row:?}"
                );
                rows.push(row);
                continue;
            }
            assert!(
                row.presented_ns.is_some() && row.surface_commit_ns.is_some(),
                "attempt {attempt}: a verified row must carry both its commit and its \
                 presentation timestamp: {row:?}"
            );
            assert!(
                row.derived.commit_to_present_ns.unwrap_or(-1) >= 0,
                "attempt {attempt}: presentation cannot precede its own commit: {row:?}"
            );
            let presentation = row
                .presentation
                .as_ref()
                .expect("verified rows retain compositor presentation metadata");
            assert_eq!(
                presentation["clock_id"].as_u64(),
                Some(1),
                "attempt {attempt}: presentation clock domain must be retained"
            );
            assert!(
                presentation.get("refresh_ns").is_some()
                    && presentation.get("sequence").is_some()
                    && presentation.get("vsync").is_some(),
                "attempt {attempt}: refresh interval, sequence, and flags must be \
                 retained when supplied: {presentation}"
            );
            verified += 1;
            rows.push(row);
        }

        // 2. An action the fixture deliberately ignores cannot be reported as a
        //    presented mutation.
        let counter_before = state_counter(&fixture.state).expect("fixture state");
        let inert = click_region(&mut driver, &fixture, "inert", 1);
        let inert_row = inert.into_iter().next().expect("one row");
        assert_eq!(
            inert_row.fixture_outcome, "no_mutation",
            "a delivered inert action must be typed as a non-mutation: {inert_row:?}"
        );
        assert!(
            !inert_row.presented_mutation()
                && inert_row.presented_ns.is_none()
                && inert_row.surface_commit_ns.is_none()
                && !inert_row.mutated,
            "an inert action must not acquire a commit or a presentation: {inert_row:?}"
        );
        assert_eq!(
            state_counter(&fixture.state),
            Some(counter_before),
            "an inert action must leave fixture state unchanged"
        );
        rows.push(inert_row);

        // 3. A superseded content update is not counted as presented. The
        //    compositor may legitimately present both updates; what must never
        //    happen is a discarded update carrying a presentation time.
        let counter_before = state_counter(&fixture.state).expect("fixture state");
        let probe = click_region(&mut driver, &fixture, "supersede", 2);
        assert_eq!(
            probe.iter().filter(|row| row.supersede_probe).count(),
            1,
            "exactly one of the two probe updates is the superseded candidate: {probe:?}"
        );
        for row in &probe {
            assert!(
                matches!(row.fixture_outcome.as_str(), "verified" | "discarded"),
                "a probe update must be presented or discarded, not {:?}: {row:?}",
                row.fixture_outcome
            );
            if row.fixture_outcome == "discarded" {
                assert!(
                    row.presented_ns.is_none()
                        && row.derived.commit_to_present_ns.is_none()
                        && row.derived.request_to_present_ns.is_none(),
                    "a discarded update must carry no presentation time: {row:?}"
                );
            }
        }
        assert_eq!(
            state_counter(&fixture.state),
            Some(counter_before + 2),
            "both probe updates mutate application state even if one is superseded"
        );
        rows.extend(probe);

        // 4. The window title mirrors the counter, so the mutation is also
        //    readable through the public Driver surface rather than only from
        //    fixture-owned files.
        let counter = state_counter(&fixture.state).expect("fixture state");
        let deadline = Instant::now() + Duration::from_secs(5);
        let expected = format!("{TITLE_PREFIX} [n={counter}]");
        loop {
            let titles: Vec<String> = driver.call("list_windows", json!({})).structured()
                ["windows"]
                .as_array()
                .map(|windows| {
                    windows
                        .iter()
                        .filter_map(|window| window["title"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            // Matched as a prefix, not for equality: the Linux Wayland backend
            // folds the app id into the reported title (`"<title> [<app id>]"`)
            // so that callers matching on either still match.
            if titles.iter().any(|title| title.contains(&expected)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the Driver never observed the mutated title {expected:?}; saw {titles:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        let summary = summarize(&rows, DEADLINE_MS * 1_000_000);
        write_evidence(&directory, &rows, &summary);
        assert_eq!(
            summary.verified + summary.discarded + summary.no_mutation,
            summary.rows,
            "every retained row must have a typed outcome: {summary:?}"
        );
        assert!(
            summary.verified >= REPEATS,
            "the measured action must produce at least {REPEATS} presented \
             mutations: {summary:?}"
        );
        assert!(
            summary.median_commit_to_present_ns.is_some()
                && summary.median_request_to_present_ns.is_some(),
            "presentation statistics must be derivable from the retained rows: {summary:?}"
        );
        Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
    });
}

/// Accounting checks that do not need a compositor. They are not `#[ignore]`d,
/// so ordinary CI covers the rules that decide "presented" without a Wayland
/// lane.
mod evidence_tests {
    use super::*;

    fn sample(outcome: &str, commit: Option<u64>, presented: Option<u64>) -> Value {
        let mut value = json!({
            "kind": "sample",
            "action": "click",
            "region": "active",
            "sequence": 1,
            "clock_id": 1,
            "fixture_input_received_ns": 2_000,
            "fixture_state_changed_ns": 2_100,
            "fixture_outcome": outcome,
            "mutated": true,
            "supersede_probe": false,
        });
        if let Some(commit) = commit {
            value["surface_commit_ns"] = json!(commit);
        }
        if let Some(presented) = presented {
            value["presented_ns"] = json!(presented);
        }
        value
    }

    #[test]
    fn joining_derives_every_delta_the_evidence_supports() {
        let row = join(&sample("verified", Some(3_000), Some(20_000)), 1_000, 25_000);
        assert_eq!(row.derived.dispatch_to_app_ns, 1_000);
        assert_eq!(row.derived.app_to_commit_ns, Some(1_000));
        assert_eq!(row.derived.commit_to_present_ns, Some(17_000));
        assert_eq!(row.derived.request_to_present_ns, Some(19_000));
        assert_eq!(row.derived.post_present_wait_ns, Some(5_000));
        assert_eq!(row.derived.request_to_return_ns, 24_000);
        assert!(row.presented_mutation());
    }

    #[test]
    fn a_driver_returning_before_presentation_is_reported_as_a_negative_wait() {
        // This is the observation the fixture exists to make possible: the
        // Driver returned 5 us before the compositor presented the update.
        let row = join(&sample("verified", Some(3_000), Some(20_000)), 1_000, 15_000);
        assert_eq!(row.derived.post_present_wait_ns, Some(-5_000));
    }

    #[test]
    fn rows_without_a_presentation_derive_no_presentation_deltas() {
        for outcome in ["discarded", "timeout", "clock_mismatch", "implausible"] {
            let row = join(&sample(outcome, Some(3_000), None), 1_000, 25_000);
            assert!(!row.presented_mutation(), "{outcome}");
            assert_eq!(row.derived.commit_to_present_ns, None, "{outcome}");
            assert_eq!(row.derived.request_to_present_ns, None, "{outcome}");
            assert_eq!(row.derived.post_present_wait_ns, None, "{outcome}");
            // The application-owned half of the timeline survives.
            assert_eq!(row.derived.app_to_commit_ns, Some(1_000), "{outcome}");
        }
    }

    #[test]
    fn an_inert_row_has_neither_commit_nor_presentation_deltas() {
        let mut inert = sample("no_mutation", None, None);
        inert["mutated"] = json!(false);
        let row = join(&inert, 1_000, 25_000);
        assert!(!row.mutated);
        assert_eq!(row.derived.app_to_commit_ns, None);
        assert_eq!(row.derived.request_to_present_ns, None);
        assert_eq!(row.derived.request_to_return_ns, 24_000);
    }

    #[test]
    fn only_presented_mutations_reach_the_presentation_statistics() {
        let rows = vec![
            join(&sample("verified", Some(3_000), Some(20_000)), 1_000, 25_000),
            join(&sample("verified", Some(3_000), Some(40_000)), 1_000, 45_000),
            join(&sample("verified", Some(3_000), Some(30_000)), 1_000, 35_000),
            join(&sample("discarded", Some(3_000), None), 1_000, 900_000_000),
            join(&sample("no_mutation", None, None), 1_000, 25_000),
        ];
        let summary = summarize(&rows, 1_000_000_000);
        assert_eq!(summary.rows, 5);
        assert_eq!(summary.verified, 3);
        assert_eq!(summary.discarded, 1);
        assert_eq!(summary.no_mutation, 1);
        // 19_000 / 29_000 / 39_000 — the discarded and inert rows contribute
        // nothing, so neither can flatter or inflate a presentation number.
        assert_eq!(summary.median_request_to_present_ns, Some(29_000));
        assert_eq!(summary.max_request_to_present_ns, Some(39_000));
        assert_eq!(summary.median_commit_to_present_ns, Some(27_000));
        assert_eq!(summary.deadline_misses, 0);
        // Every row contributes to the Driver-owned round trip, including the
        // slow discarded one.
        assert_eq!(summary.median_request_to_return_ns, Some(34_000));
    }

    #[test]
    fn a_late_presentation_is_counted_as_a_deadline_miss() {
        let rows = vec![join(
            &sample("verified", Some(3_000), Some(2_000_000_000)),
            1_000,
            2_100_000_000,
        )];
        let summary = summarize(&rows, 1_000_000_000);
        assert_eq!(summary.deadline_misses, 1);
        assert_eq!(summary.verified, 1);
    }

    #[test]
    fn an_empty_run_reports_no_statistics_instead_of_zeros() {
        let summary = summarize(&[], 1_000_000_000);
        assert_eq!(summary.rows, 0);
        assert_eq!(summary.median_request_to_present_ns, None);
        assert_eq!(summary.max_commit_to_present_ns, None);
        let encoded = serde_json::to_value(&summary).expect("serialize summary");
        assert!(encoded.get("median_request_to_present_ns").is_none());
        assert_eq!(encoded["rows"], 0);
    }

    #[test]
    fn the_median_is_an_observed_value_not_an_interpolation() {
        assert_eq!(median(&mut vec![]), None);
        assert_eq!(median(&mut vec![7]), Some(7));
        assert_eq!(median(&mut vec![9, 1]), Some(1));
        assert_eq!(median(&mut vec![5, 1, 9]), Some(5));
        assert_eq!(median(&mut vec![4, 1, 9, 5]), Some(4));
    }

    #[test]
    fn summaries_report_no_percentiles() {
        let rows = vec![join(
            &sample("verified", Some(3_000), Some(20_000)),
            1_000,
            25_000,
        )];
        let encoded = serde_json::to_value(summarize(&rows, 1_000_000_000))
            .expect("serialize summary")
            .to_string();
        for forbidden in ["p95", "p99", "percentile"] {
            assert!(
                !encoded.contains(forbidden),
                "small runs must not report {forbidden}: {encoded}"
            );
        }
    }
}
