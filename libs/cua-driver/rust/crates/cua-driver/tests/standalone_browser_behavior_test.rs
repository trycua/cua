//! Optional standalone Chrome/Edge browser-tool E2E coverage.
//!
//! These rows use real installed browsers with a fresh driver-owned profile.
//! They are intentionally outside canonical `all`: maintainers dispatch them
//! on interactive desktops where the requested browser is installed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, RefusalCode, Scope, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{spawn_in_job, BrowserFixtureServer, Driver, McpDriver, ToolResponse};

const FIXTURE_HTML: &str = include_str!("../../../../tests/fixtures/shared/web/index.html");

#[derive(Clone, Debug)]
struct BrowserSpec {
    name: String,
    executable: PathBuf,
}

struct BrowserFixture {
    driver: McpDriver,
    server: BrowserFixtureServer,
    _profile: tempfile::TempDir,
    cdp_port: u16,
    pid: u32,
    window_id: u64,
}

fn browser_specs() -> Vec<BrowserSpec> {
    if let Some(path) = std::env::var_os("CUA_E2E_BROWSER_BIN") {
        let executable = PathBuf::from(path);
        assert!(
            executable.is_file(),
            "CUA_E2E_BROWSER_BIN is not a file: {}",
            executable.display()
        );
        let name = std::env::var("CUA_E2E_BROWSER_NAME").unwrap_or_else(|_| "chromium".to_owned());
        return vec![BrowserSpec { name, executable }];
    }

    #[cfg(target_os = "macos")]
    let candidates = [
        (
            "chrome",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ),
        (
            "edge",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ),
    ];
    #[cfg(target_os = "linux")]
    let candidates = [
        ("chrome", "/usr/bin/google-chrome"),
        ("chromium", "/usr/bin/chromium"),
        ("chromium", "/usr/bin/chromium-browser"),
        ("edge", "/usr/bin/microsoft-edge"),
    ];
    #[cfg(target_os = "windows")]
    let candidates = {
        let program_files = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let program_files_x86 = std::env::var_os("ProgramFiles(x86)")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        return [
            (
                "chrome",
                program_files.join(r"Google\Chrome\Application\chrome.exe"),
            ),
            (
                "chrome",
                program_files_x86.join(r"Google\Chrome\Application\chrome.exe"),
            ),
            (
                "chrome",
                local_app_data.join(r"Google\Chrome\Application\chrome.exe"),
            ),
            (
                "edge",
                program_files_x86.join(r"Microsoft\Edge\Application\msedge.exe"),
            ),
            (
                "edge",
                program_files.join(r"Microsoft\Edge\Application\msedge.exe"),
            ),
        ]
        .into_iter()
        .filter(|(_, executable)| executable.is_file())
        .map(|(name, executable)| BrowserSpec {
            name: name.to_owned(),
            executable,
        })
        .collect();
    };

    #[cfg(not(target_os = "windows"))]
    candidates
        .into_iter()
        .map(|(name, executable)| BrowserSpec {
            name: name.to_owned(),
            executable: PathBuf::from(executable),
        })
        .filter(|spec| spec.executable.is_file())
        .collect()
}

fn allocate_loopback_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("allocate standalone browser CDP port")
        .port()
}

fn spawn_driver(label: &str) -> McpDriver {
    #[cfg(target_os = "macos")]
    let driver = McpDriver::spawn_macos_daemon_proxy_named(label);
    #[cfg(not(target_os = "macos"))]
    let driver = McpDriver::spawn_named(label);
    driver.expect("cua-driver binary/daemon is required for standalone browser E2E")
}

fn command_for_browser(
    spec: &BrowserSpec,
    profile: &Path,
    cdp_port: u16,
    url: &str,
    position: (i32, i32),
) -> Command {
    let mut command = Command::new(&spec.executable);
    let output = if std::env::var_os("CUA_E2E_BROWSER_STDERR").is_some() {
        Stdio::inherit()
    } else {
        Stdio::null()
    };
    command
        .arg(format!("--remote-debugging-port={cdp_port}"))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--new-window")
        .arg(format!("--window-position={},{}", position.0, position.1))
        .arg("--window-size=980,760")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(output);
    command
}

fn window_ids(driver: &mut McpDriver) -> HashSet<u64> {
    driver
        .call("list_windows", serde_json::json!({}))
        .structured()["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|window| window["window_id"].as_u64())
        .collect()
}

fn wait_for_fixture_window(
    driver: &mut McpDriver,
    before: &HashSet<u64>,
    server: &BrowserFixtureServer,
) -> Option<(u32, u64)> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({}));
        if let Some(window) = windows.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["window_id"]
                        .as_u64()
                        .is_some_and(|id| !before.contains(&id))
                        && window["title"]
                            .as_str()
                            .is_some_and(|title| title.contains("cua-driver Web Harness"))
                })
            })
        {
            if server.contains("WEB_HARNESS_MARKER_v1") {
                return Some((
                    window["pid"].as_u64().expect("browser window pid") as u32,
                    window["window_id"].as_u64().expect("browser window id"),
                ));
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_browser_command(
    driver: &mut McpDriver,
    spec: &BrowserSpec,
    profile: &Path,
    cdp_port: u16,
    url: &str,
    position: (i32, i32),
) {
    let mut command = command_for_browser(spec, profile, cdp_port, url, position);
    let child = spawn_in_job(&mut command).expect("launch standalone browser");
    eprintln!(
        "[standalone-browser] spawned {} pid={} profile={} cdp_port={cdp_port}",
        spec.name,
        child.id(),
        profile.display()
    );
    driver.reaper().push(child);
}

fn launch_browser(spec: &BrowserSpec, label: &str) -> BrowserFixture {
    let mut driver = spawn_driver(label);
    let server = BrowserFixtureServer::start(FIXTURE_HTML);
    let profile = tempfile::Builder::new()
        .prefix("cua-e2e-browser-")
        .tempdir()
        .expect("create isolated browser profile");
    let cdp_port = allocate_loopback_port();
    let before = window_ids(&mut driver);
    spawn_browser_command(
        &mut driver,
        spec,
        profile.path(),
        cdp_port,
        server.page_url(),
        (80, 80),
    );
    let window = wait_for_fixture_window(&mut driver, &before, &server).or_else(|| {
        eprintln!(
            "[standalone-browser] retrying bounded URL handoff for {}",
            spec.name
        );
        spawn_browser_command(
            &mut driver,
            spec,
            profile.path(),
            cdp_port,
            server.page_url(),
            (80, 80),
        );
        wait_for_fixture_window(&mut driver, &before, &server)
    });
    let (pid, window_id) = window.unwrap_or_else(|| {
        panic!(
            "standalone browser fixture did not become visible after bounded URL handoff; journal={}",
            server.snapshot()
        )
    });
    driver.reaper().track_pid(pid);
    BrowserFixture {
        driver,
        server,
        _profile: profile,
        cdp_port,
        pid,
        window_id,
    }
}

fn launch_additional_window(
    fixture: &mut BrowserFixture,
    spec: &BrowserSpec,
) -> (BrowserFixtureServer, u32, u64) {
    let server = BrowserFixtureServer::start(FIXTURE_HTML);
    let before = window_ids(&mut fixture.driver);
    spawn_browser_command(
        &mut fixture.driver,
        spec,
        fixture._profile.path(),
        fixture.cdp_port,
        server.page_url(),
        (520, 120),
    );
    let window = wait_for_fixture_window(&mut fixture.driver, &before, &server).or_else(|| {
        spawn_browser_command(
            &mut fixture.driver,
            spec,
            fixture._profile.path(),
            fixture.cdp_port,
            server.page_url(),
            (520, 120),
        );
        wait_for_fixture_window(&mut fixture.driver, &before, &server)
    });
    let (pid, window_id) = window.expect("additional standalone browser window did not appear");
    assert_eq!(pid, fixture.pid, "additional window must share browser pid");
    (server, pid, window_id)
}

fn wait_for_text(server: &BrowserFixtureServer, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.text(id).as_deref() != Some(expected) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(server.text(id).as_deref(), Some(expected));
}

fn wait_for_value(server: &BrowserFixtureServer, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.value(id).as_deref() != Some(expected) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(server.value(id).as_deref(), Some(expected));
}

fn ref_by_label(snapshot: &ToolResponse, fragment: &str) -> String {
    snapshot.structured()["refs"]
        .as_array()
        .and_then(|refs| {
            refs.iter().find(|entry| {
                entry["label"]
                    .as_str()
                    .is_some_and(|label| label.contains(fragment))
            })
        })
        .and_then(|entry| entry["ref"].as_str())
        .unwrap_or_else(|| panic!("missing browser ref {fragment:?}: {}", snapshot.raw))
        .to_owned()
}

fn bind(fixture: &mut BrowserFixture, session: &str) -> (String, String, ToolResponse) {
    let started = fixture
        .driver
        .call("start_session", serde_json::json!({ "session": session }));
    assert!(!started.is_error(), "start_session failed: {}", started.raw);
    let prepared = fixture.driver.call(
        "browser_prepare",
        serde_json::json!({ "pid": fixture.pid as i64, "session": session }),
    );
    assert_eq!(prepared.structured()["prepared"], true, "{}", prepared.raw);
    let state = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "pid": fixture.pid as i64,
            "window_id": fixture.window_id,
            "session": session,
        }),
    );
    assert_eq!(state.structured()["status"], "ok", "{}", state.raw);
    assert_eq!(
        state.structured()["binding_quality"],
        "exact",
        "{}",
        state.raw
    );
    let target = state.structured()["target_id"]
        .as_str()
        .expect("target_id")
        .to_owned();
    let tab = state.structured()["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
        .and_then(|tab| tab["tab_id"].as_str())
        .expect("active tab")
        .to_owned();
    let snapshot = fixture.driver.call(
        "get_browser_state",
        serde_json::json!({
            "target_id": target,
            "tab_id": tab,
            "session": session,
        }),
    );
    assert_eq!(snapshot.structured()["status"], "ok", "{}", snapshot.raw);
    (target, tab, snapshot)
}

fn case(browser: &str, action: &str) -> CaseSpec {
    let mut oracles = vec![
        OracleKind::FixtureState,
        OracleKind::Focus,
        OracleKind::ZOrder,
        OracleKind::NoLeakedInput,
    ];
    if cua_driver_testkit::e2e::DisplayServer::current()
        != cua_driver_testkit::e2e::DisplayServer::Wayland
    {
        oracles.push(OracleKind::Cursor);
    }
    CaseSpec::delivered(
        format!("{}-{browser}-standalone-{action}", std::env::consts::OS),
        browser,
        "standalone-chromium",
        action,
        Targeting::Page,
        Delivery::Background,
        Scope::Window,
        DriverRoute::Cdp,
        oracles,
    )
}

fn refusal_case(browser: &str, action: &str, code: RefusalCode) -> CaseSpec {
    case(browser, action).expecting_refusal(vec![code])
}

fn run_with_background_oracles(
    fixture: &mut BrowserFixture,
    action: impl FnOnce(&mut BrowserFixture) -> Observation,
) -> Observation {
    let sentinel = ForegroundSentinel::launch(&mut fixture.driver);
    let target = TargetWindow {
        pid: fixture.pid,
        native_id: fixture.window_id,
    };
    sentinel
        .assert_background_posture(target)
        .expect("establish standalone browser background posture");
    fixture.driver.start_behavior_recording();
    sentinel
        .prepare_background_observation(&mut fixture.driver, target)
        .expect("restore standalone browser background posture");
    let (mut observation, passed) = sentinel
        .observe_background(target, || action(fixture))
        .expect("observe standalone browser desktop effects");
    observation.passed_oracles.extend(passed);
    observation
}

fn run_roundtrip(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-roundtrip",
        std::env::consts::OS,
        spec.name
    );
    execute_case(case(&spec.name, "browser_tool_roundtrip"), |evidence| {
        let mut fixture = launch_browser(spec, &scenario);
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-roundtrip-{}", fixture.pid);
            let (target, tab, snapshot) = bind(fixture, &session);
            let click_ref = ref_by_label(&snapshot, "id=btn-increment");
            let click = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": click_ref,
                    "session": session,
                }),
            );
            assert_eq!(click.structured()["status"], "ok", "{}", click.raw);
            wait_for_text(&fixture.server, "lbl-counter", "counter=1");

            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "session": session,
                }),
            );
            let input_ref = ref_by_label(&snapshot, "id=txt-input");
            let typed = fixture.driver.call(
                "browser_type",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": input_ref,
                    "text": "standalone-browser",
                    "session": session,
                }),
            );
            assert_eq!(typed.structured()["status"], "ok", "{}", typed.raw);
            wait_for_value(&fixture.server, "txt-input", "standalone-browser");
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        })
    });
}

fn run_stale_ref(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-stale-ref",
        std::env::consts::OS,
        spec.name
    );
    execute_case(case(&spec.name, "browser_ref_stale"), |evidence| {
        let mut fixture = launch_browser(spec, &scenario);
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-stale-ref-{}", fixture.pid);
            let (target, tab, first) = bind(fixture, &session);
            let stale_ref = ref_by_label(&first, "id=btn-increment");
            let newer = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "session": session,
                }),
            );
            assert_eq!(newer.structured()["status"], "ok", "{}", newer.raw);
            let refused = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": stale_ref,
                    "session": session,
                }),
            );
            assert_eq!(
                refused.structured()["refusal"]["code"],
                "browser_ref_stale",
                "{}",
                refused.raw
            );
            wait_for_text(&fixture.server, "lbl-counter", "counter=0");
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        })
    });
}

fn run_two_window_collision(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-two-window",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        refusal_case(
            &spec.name,
            "same_bounds_binding_ambiguous",
            RefusalCode::BrowserBindingAmbiguous,
        ),
        |evidence| {
            let mut fixture = launch_browser(spec, &scenario);
            *evidence = recording_evidence(fixture.driver.recording_dir());
            let (second_server, second_pid, second_window_id) =
                launch_additional_window(&mut fixture, spec);
            assert_eq!(second_pid, fixture.pid);
            assert_ne!(second_window_id, fixture.window_id);
            run_with_background_oracles(&mut fixture, |fixture| {
                let session = format!("standalone-two-window-{}", fixture.pid);
                let started = fixture
                    .driver
                    .call("start_session", serde_json::json!({ "session": session }));
                assert!(!started.is_error(), "start_session failed: {}", started.raw);
                let prepared = fixture.driver.call(
                    "browser_prepare",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "session": session,
                    }),
                );
                assert_eq!(prepared.structured()["prepared"], true, "{}", prepared.raw);
                let refused = fixture.driver.call(
                    "get_browser_state",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.window_id,
                        "session": session,
                    }),
                );
                assert_eq!(
                    refused.structured()["refusal"]["code"],
                    "browser_binding_ambiguous",
                    "{}",
                    refused.raw
                );
                wait_for_text(&fixture.server, "lbl-counter", "counter=0");
                wait_for_text(&second_server, "lbl-counter", "counter=0");
                Observation::refused(
                    RefusalCode::BrowserBindingAmbiguous,
                    vec![OracleKind::FixtureState],
                    refused.text(),
                    Evidence::default(),
                )
            })
        },
    );
}

#[test]
#[ignore = "requires an installed standalone Chromium browser and an interactive desktop"]
fn standalone_browser_matrix() {
    let specs = browser_specs();
    if specs.is_empty() {
        if std::env::var_os("CUA_TEST_REQUIRE_EXTERNAL_BROWSERS").is_some() {
            panic!("no standalone Chrome, Edge, or Chromium executable was found");
        }
        eprintln!("no standalone Chromium browser installed; optional suite skipped");
        return;
    }
    for spec in specs {
        eprintln!(
            "[standalone-browser] {} at {}",
            spec.name,
            spec.executable.display()
        );
        run_roundtrip(&spec);
        run_stale_ref(&spec);
        run_two_window_collision(&spec);
    }
}
