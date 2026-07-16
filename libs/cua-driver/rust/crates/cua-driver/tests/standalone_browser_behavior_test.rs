//! Optional standalone Chrome/Edge browser-tool E2E coverage.
//!
//! These rows use real installed browsers with a fresh driver-owned profile.
//! They are intentionally outside canonical `all`: maintainers dispatch them
//! on interactive desktops where the requested browser is installed.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, write_environment_from_env, CaseSpec, Delivery, DriverRoute,
    EnvironmentRecord, Evidence, Observation, OracleKind, RefusalCode, Scope, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{spawn_in_job, BrowserFixtureServer, Driver, McpDriver, ToolResponse};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

const FIXTURE_HTML: &str = include_str!("../../../../tests/fixtures/shared/web/index.html");
static STANDALONE_BROWSER_TEST_LOCK: Mutex<()> = Mutex::new(());

fn standalone_fixture_html() -> String {
    FIXTURE_HTML.replace(
        "</body>",
        r#"<a id="standalone-new-tab" data-cua-id="standalone-new-tab" href="/fixture?tab=second" target="_blank" onclick="document.getElementById('standalone-tab-state').textContent='new_tab=open'">Open standalone tab</a>
<span id="standalone-tab-state" data-cua-id="standalone-tab-state">new_tab=closed</span>
<script>
  if (new URLSearchParams(window.location.search).get('tab') === 'second') {
    document.getElementById('standalone-tab-state').textContent = 'new_tab=open';
  }
</script>
</body>"#,
    )
}

fn standalone_frame_fixture_html(oopif_url: &str) -> String {
    let oopif_url = serde_json::to_string(oopif_url).expect("serialize OOPIF fixture URL");
    FIXTURE_HTML.replace(
        "</body>",
        &format!(
            r#"<fieldset>
  <legend>browser frame routes</legend>
  <div id="standalone-shadow-host"></div>
  <span id="standalone-shadow-state" data-cua-id="standalone-shadow-state">shadow=loading</span>
  <iframe id="standalone-same-frame" title="standalone same-process frame"></iframe>
  <span id="standalone-frame-state" data-cua-id="standalone-frame-state">iframe=loading</span>
  <iframe id="standalone-oopif" title="standalone out-of-process frame" src={oopif_url}></iframe>
</fieldset>
<script>
  const shadowState = document.getElementById('standalone-shadow-state');
  const shadow = document.getElementById('standalone-shadow-host').attachShadow({{ mode: 'open' }});
  shadow.innerHTML = `<button id="standalone-shadow-button" aria-label="standalone-shadow-button">Shadow button</button>
    <input id="standalone-shadow-input" aria-label="standalone-shadow-input">`;
  shadow.getElementById('standalone-shadow-button').addEventListener('click', () => {{
    shadowState.textContent = 'shadow=clicked';
  }});
  shadow.getElementById('standalone-shadow-input').addEventListener('input', event => {{
    shadowState.textContent = `shadow=typed:${{event.target.value}}`;
  }});

  const frameState = document.getElementById('standalone-frame-state');
  const sameFrame = document.getElementById('standalone-same-frame');
  sameFrame.addEventListener('load', () => {{ frameState.textContent = 'iframe=ready'; }});
  sameFrame.srcdoc = `<!doctype html><button id="standalone-frame-button" aria-label="standalone-frame-button">Frame button</button>
    <input id="standalone-frame-input" aria-label="standalone-frame-input">
    <script>
      document.getElementById('standalone-frame-button').addEventListener('click', () => {{
        parent.document.getElementById('standalone-frame-state').textContent = 'iframe=clicked';
      }});
      document.getElementById('standalone-frame-input').addEventListener('input', event => {{
        parent.document.getElementById('standalone-frame-state').textContent = 'iframe=typed:' + event.target.value;
      }});
    <\/script>`;
  shadowState.textContent = 'shadow=ready';
</script>
</body>"#
        ),
    )
}

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
    {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        return [
            (
                "chrome",
                PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            ),
            (
                "chrome",
                home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            ),
            (
                "edge",
                PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            ),
            (
                "edge",
                home.join("Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            ),
        ]
        .into_iter()
        .filter(|(_, executable)| executable.is_file())
        .map(|(name, executable)| BrowserSpec {
            name: name.to_owned(),
            executable,
        })
        .collect();
    }
    #[cfg(target_os = "linux")]
    {
        let mut candidates = vec![
            ("chrome", PathBuf::from("/usr/bin/google-chrome")),
            ("chromium", PathBuf::from("/usr/bin/chromium")),
            ("chromium", PathBuf::from("/usr/bin/chromium-browser")),
            ("edge", PathBuf::from("/usr/bin/microsoft-edge")),
        ];
        if let Some(path) = std::env::var_os("PATH") {
            for (name, executable_name) in [
                ("chrome", "google-chrome"),
                ("chromium", "chromium"),
                ("chromium", "chromium-browser"),
                ("edge", "microsoft-edge"),
            ] {
                if let Some(executable) = std::env::split_paths(&path)
                    .map(|directory| directory.join(executable_name))
                    .find(|candidate| candidate.is_file())
                {
                    candidates.push((name, executable));
                }
            }
        }
        return candidates
            .into_iter()
            .filter(|(_, executable)| executable.is_file())
            .map(|(name, executable)| {
                let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
                (name, executable)
            })
            .fold(Vec::new(), |mut specs, (name, executable)| {
                if !specs
                    .iter()
                    .any(|spec: &BrowserSpec| spec.executable == executable)
                {
                    specs.push(BrowserSpec {
                        name: name.to_owned(),
                        executable,
                    });
                }
                specs
            });
    }
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
}

fn allocate_loopback_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("allocate standalone browser CDP port")
        .port()
}

fn browser_endpoint_url(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|error| panic!("connect to harness CDP endpoint {port}: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set CDP HTTP read timeout");
    write!(
        stream,
        "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("request harness CDP version");
    let mut response = Vec::new();
    if let Err(error) = stream.read_to_end(&mut response) {
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "read harness CDP version: {error}"
        );
    }
    let body_start = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .expect("CDP HTTP response headers");
    let body: serde_json::Value =
        serde_json::from_slice(&response[body_start..]).expect("parse CDP version response");
    body["webSocketDebuggerUrl"]
        .as_str()
        .expect("browser websocket URL")
        .to_owned()
}

fn harness_cdp_call(port: u16, method: &str, params: serde_json::Value) -> serde_json::Value {
    let ws_url = browser_endpoint_url(port);
    tokio::runtime::Runtime::new()
        .expect("create harness CDP runtime")
        .block_on(async move {
            let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url)
                .await
                .expect("connect harness browser websocket");
            socket
                .send(Message::Text(
                    serde_json::json!({"id": 1, "method": method, "params": params}).to_string(),
                ))
                .await
                .expect("send harness CDP command");
            while let Some(frame) = socket.next().await {
                let frame = frame.expect("read harness CDP frame");
                let Message::Text(text) = frame else {
                    continue;
                };
                let value: serde_json::Value =
                    serde_json::from_str(&text).expect("parse harness CDP response");
                if value["id"] == 1 {
                    assert!(value.get("error").is_none(), "CDP {method}: {value}");
                    return value["result"].clone();
                }
            }
            panic!("browser websocket closed before CDP {method} replied")
        })
}

fn cdp_target_for_url(port: u16, url: &str) -> String {
    harness_cdp_call(port, "Target.getTargets", serde_json::json!({}))["targetInfos"]
        .as_array()
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target["type"] == "page" && target["url"] == url)
        })
        .and_then(|target| target["targetId"].as_str())
        .unwrap_or_else(|| panic!("no CDP page target for {url}"))
        .to_owned()
}

fn driver_profile_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
            .join("CuaDriver")
            .join("BrowserProfiles");
    }
    #[cfg(target_os = "macos")]
    {
        return PathBuf::from(std::env::var_os("HOME").expect("HOME"))
            .join("Library")
            .join("Application Support")
            .join("CuaDriver")
            .join("BrowserProfiles");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                    .join(".local")
                    .join("state")
            });
        state.join("cua-driver").join("browser-profiles")
    }
}

fn profile_entries(root: &Path) -> HashSet<std::ffi::OsString> {
    std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect()
}

fn spawn_driver(label: &str) -> McpDriver {
    #[cfg(target_os = "macos")]
    let driver = McpDriver::spawn_macos_daemon_proxy_named(label);
    #[cfg(not(target_os = "macos"))]
    let driver = McpDriver::spawn_named(label);
    driver.expect("cua-driver binary/daemon is required for standalone browser E2E")
}

#[cfg(target_os = "linux")]
fn configure_linux_browser_command(command: &mut Command) {
    command.arg("--password-store=basic");
    let native_wayland = std::env::var("XDG_SESSION_TYPE")
        .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_none();
    if native_wayland {
        command.arg("--ozone-platform=wayland");
    }
}

fn configure_test_browser_sandbox(command: &mut Command) {
    if std::env::var("CUA_E2E_BROWSER_NO_SANDBOX").as_deref() == Ok("1") {
        command.arg("--no-sandbox");
    }
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
        .arg("--disable-extensions")
        .arg("--disable-default-apps")
        .arg("--site-per-process")
        .arg("--new-window")
        .arg(format!("--window-position={},{}", position.0, position.1))
        .arg("--window-size=980,760");
    configure_test_browser_sandbox(&mut command);
    #[cfg(target_os = "linux")]
    configure_linux_browser_command(&mut command);
    command.arg(url).stdout(Stdio::null()).stderr(output);
    command
}

fn command_for_unprepared_browser(
    spec: &BrowserSpec,
    profile: &Path,
    url: &str,
    position: (i32, i32),
) -> Command {
    let mut command = Command::new(&spec.executable);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-extensions")
        .arg("--disable-default-apps")
        .arg("--new-window")
        .arg(format!("--window-position={},{}", position.0, position.1))
        .arg("--window-size=980,760");
    configure_test_browser_sandbox(&mut command);
    #[cfg(target_os = "linux")]
    configure_linux_browser_command(&mut command);
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
    // A first temporary-profile launch can take longer than 20 seconds on a
    // cold macOS host. Waiting is topology-neutral; relaunching is not, since
    // the first process may eventually map and leave an extra CDP page.
    let deadline = Instant::now() + Duration::from_secs(40);
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

fn wait_for_exact_browser_binding(
    driver: &mut McpDriver,
    pid: u32,
    session: &str,
) -> Option<(u64, ToolResponse)> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({ "pid": pid }));
        for window_id in windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|window| window["window_id"].as_u64())
        {
            let state = driver.call(
                "get_browser_state",
                serde_json::json!({
                    "pid": pid as i64,
                    "window_id": window_id,
                    "session": session,
                }),
            );
            if state.structured()["binding_quality"] == "exact" {
                return Some((window_id, state));
            }
        }
        if Instant::now() >= deadline {
            eprintln!(
                "no exact browser window binding for prepared pid {pid}; last windows={}",
                windows.raw
            );
            return None;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_pid_windows_to_close(driver: &mut McpDriver, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({ "pid": pid }));
        let is_empty = windows.structured()["windows"]
            .as_array()
            .is_none_or(Vec::is_empty);
        if is_empty {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "prepared browser windows remained after end_session: {}",
            windows.raw
        );
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
    launch_browser_with_html(spec, label, standalone_fixture_html())
}

fn launch_browser_with_html(spec: &BrowserSpec, label: &str, html: String) -> BrowserFixture {
    let mut driver = spawn_driver(label);
    let server = BrowserFixtureServer::start(&html);
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
    let window = wait_for_fixture_window(&mut driver, &before, &server);
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

fn launch_additional_window(fixture: &mut BrowserFixture) -> (BrowserFixtureServer, u32, u64) {
    let html = standalone_fixture_html();
    let server = BrowserFixtureServer::start(&html);
    let before = window_ids(&mut fixture.driver);
    let first_target = cdp_target_for_url(fixture.cdp_port, fixture.server.page_url());
    let first_window = harness_cdp_call(
        fixture.cdp_port,
        "Browser.getWindowForTarget",
        serde_json::json!({"targetId": first_target}),
    );
    let bounds = &first_window["bounds"];
    let created = harness_cdp_call(
        fixture.cdp_port,
        "Target.createTarget",
        serde_json::json!({
            "url": server.page_url(),
            "newWindow": true,
            "left": bounds["left"],
            "top": bounds["top"],
            "width": bounds["width"],
            "height": bounds["height"],
        }),
    );
    assert!(created["targetId"].is_string(), "{created}");
    let window = wait_for_fixture_window(&mut fixture.driver, &before, &server);
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

fn wait_for_observed(server: &BrowserFixtureServer, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !server.has_observed(marker) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        server.has_observed(marker),
        "fixture never observed {marker:?}: {}",
        server.snapshot()
    );
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

fn ref_by_frame_label(snapshot: &ToolResponse, frame: &str, fragment: &str) -> String {
    snapshot.structured()["refs"]
        .as_array()
        .and_then(|refs| {
            refs.iter().find(|entry| {
                entry["frame"] == frame
                    && entry["label"]
                        .as_str()
                        .is_some_and(|label| label.contains(fragment))
            })
        })
        .and_then(|entry| entry["ref"].as_str())
        .unwrap_or_else(|| panic!("missing {frame} browser ref {fragment:?}: {}", snapshot.raw))
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
                    "input_route": "dom_event",
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

fn run_background_type(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-background-type",
        std::env::consts::OS,
        spec.name
    );
    execute_case(case(&spec.name, "background_type"), |evidence| {
        let mut fixture = launch_browser(spec, &scenario);
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-background-type-{}", fixture.pid);
            let (target, tab, snapshot) = bind(fixture, &session);
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

fn run_trusted_click(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-trusted-click",
        std::env::consts::OS,
        spec.name
    );
    let case = if cfg!(any(target_os = "linux", target_os = "macos")) {
        refusal_case(
            &spec.name,
            "trusted_click",
            RefusalCode::BrowserInputTrustUnavailable,
        )
    } else {
        case(&spec.name, "trusted_click")
    };
    execute_case(case, |evidence| {
        let mut fixture = launch_browser(spec, &scenario);
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-trusted-click-{}", fixture.pid);
            let (target, tab, snapshot) = bind(fixture, &session);
            let click_ref = ref_by_label(&snapshot, "id=btn-increment");
            let click = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": click_ref,
                    "input_route": "trusted",
                    "session": session,
                }),
            );
            if cfg!(any(target_os = "linux", target_os = "macos")) {
                assert_eq!(
                    click.structured()["refusal"]["code"],
                    "browser_input_trust_unavailable",
                    "{}",
                    click.raw
                );
                wait_for_text(&fixture.server, "lbl-counter", "counter=0");
                Observation::refused(
                    RefusalCode::BrowserInputTrustUnavailable,
                    vec![OracleKind::FixtureState],
                    click.text(),
                    Evidence::default(),
                )
            } else {
                assert_eq!(click.structured()["status"], "ok", "{}", click.raw);
                wait_for_text(&fixture.server, "lbl-counter", "counter=1");
                Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
            }
        })
    });
}

fn run_prepare_isolated_launch(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-prepare-isolated",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        case(&spec.name, "browser_prepare_isolated_launch"),
        |evidence| {
            let source_server = BrowserFixtureServer::start(&standalone_fixture_html());
            let target_server = BrowserFixtureServer::start(&standalone_fixture_html());
            let source_profile = tempfile::Builder::new()
                .prefix("cua-e2e-user-browser-")
                .tempdir()
                .expect("create ordinary browser profile");
            let driver_profiles = driver_profile_root();
            let profiles_before = profile_entries(&driver_profiles);
            let mut driver = spawn_driver(&scenario);
            *evidence = recording_evidence(driver.recording_dir());

            let before = window_ids(&mut driver);
            let mut source_command = command_for_unprepared_browser(
                spec,
                source_profile.path(),
                source_server.page_url(),
                (80, 80),
            );
            let source_child = spawn_in_job(&mut source_command).expect("launch ordinary browser");
            driver.reaper().push(source_child);
            let (source_pid, source_window_id) =
                wait_for_fixture_window(&mut driver, &before, &source_server)
                    .expect("ordinary browser fixture window");
            driver.reaper().track_pid(source_pid);

            let session = format!("standalone-prepare-{source_pid}");
            let started = driver.call("start_session", serde_json::json!({ "session": session }));
            assert!(!started.is_error(), "start_session failed: {}", started.raw);
            driver.start_behavior_recording();
            let prepared = driver.call(
                "browser_prepare",
                serde_json::json!({
                    "pid": source_pid as i64,
                    "session": session,
                    "allow_launch": true,
                    "profile": {"mode": "isolated_new"},
                }),
            );
            assert_eq!(prepared.structured()["status"], "ok", "{}", prepared.raw);
            assert_eq!(
                prepared.structured()["action"],
                "launched_isolated_browser",
                "{}",
                prepared.raw
            );
            assert_eq!(
                prepared.structured()["side_effects"]["launched_browser"],
                true
            );
            assert_eq!(
                prepared.structured()["side_effects"]["created_profile"],
                true
            );
            let prepared_json = prepared.raw.to_string();
            assert!(
                !prepared_json.contains(&driver_profiles.display().to_string()),
                "browser_prepare disclosed its private profile path: {}",
                prepared.raw
            );
            assert!(
                !prepared_json.contains("approval_token"),
                "{}",
                prepared.raw
            );

            let prepared_pid = prepared.structured()["prepared_pid"]
                .as_u64()
                .expect("prepared browser pid") as u32;
            assert_ne!(prepared_pid, source_pid);
            let (prepared_window_id, state) =
                wait_for_exact_browser_binding(&mut driver, prepared_pid, &session)
                    .expect("isolated browser did not expose an exactly bindable window");
            let target = state.structured()["target_id"]
                .as_str()
                .expect("prepared target id")
                .to_owned();
            let tab = state.structured()["tabs"]
                .as_array()
                .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
                .and_then(|tab| tab["tab_id"].as_str())
                .expect("prepared active tab")
                .to_owned();
            let navigated = driver.call(
                "browser_navigate",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "url": target_server.page_url(),
                    "session": session,
                }),
            );
            assert_eq!(navigated.structured()["status"], "ok", "{}", navigated.raw);
            wait_for_observed(&target_server, "WEB_HARNESS_MARKER_v1");

            let target_window = TargetWindow {
                pid: prepared_pid,
                native_id: prepared_window_id,
            };
            let sentinel = ForegroundSentinel::launch(&mut driver);
            sentinel
                .assert_background_posture(target_window)
                .expect("establish isolated browser background posture");
            sentinel
                .prepare_background_observation(&mut driver, target_window)
                .expect("restore isolated browser background posture");
            let (mut observation, passed) = sentinel
                .observe_background(target_window, || {
                    let snapshot = driver.call(
                        "get_browser_state",
                        serde_json::json!({
                            "target_id": target,
                            "tab_id": tab,
                            "session": session,
                        }),
                    );
                    let click_ref = ref_by_label(&snapshot, "id=btn-increment");
                    let clicked = driver.call(
                        "browser_click",
                        serde_json::json!({
                            "target_id": target,
                            "tab_id": tab,
                            "ref": click_ref,
                            "input_route": "dom_event",
                            "session": session,
                        }),
                    );
                    assert_eq!(clicked.structured()["status"], "ok", "{}", clicked.raw);
                    wait_for_text(&target_server, "lbl-counter", "counter=1");
                    wait_for_text(&source_server, "lbl-counter", "counter=0");
                    let source_windows =
                        driver.call("list_windows", serde_json::json!({"pid": source_pid}));
                    assert!(
                        source_windows.structured()["windows"]
                            .as_array()
                            .is_some_and(|windows| windows.iter().any(|window| {
                                window["window_id"].as_u64() == Some(source_window_id)
                            })),
                        "ordinary browser was modified or terminated: {}",
                        source_windows.raw
                    );
                    Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
                })
                .expect("observe isolated browser desktop effects");
            observation.passed_oracles.extend(passed);

            let ended = driver.call("end_session", serde_json::json!({ "session": session }));
            assert!(!ended.is_error(), "end_session failed: {}", ended.raw);
            wait_for_pid_windows_to_close(&mut driver, prepared_pid);
            let profile_deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if profile_entries(&driver_profiles) == profiles_before {
                    break;
                }
                assert!(
                    Instant::now() < profile_deadline,
                    "isolated_new profile remained after end_session"
                );
                thread::sleep(Duration::from_millis(100));
            }
            observation
        },
    );
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
                    "input_route": "dom_event",
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

fn run_frame_roundtrip(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-frame-roundtrip",
        std::env::consts::OS,
        spec.name
    );
    let child_server = BrowserFixtureServer::start(FIXTURE_HTML);
    let child_url = child_server.page_url_on_host("localhost");
    execute_case(case(&spec.name, "browser_frame_roundtrip"), |evidence| {
        let html = standalone_frame_fixture_html(&child_url);
        let mut fixture = launch_browser_with_html(spec, &scenario, html);
        *evidence = recording_evidence(fixture.driver.recording_dir());
        wait_for_text(&fixture.server, "standalone-shadow-state", "shadow=ready");
        wait_for_text(&fixture.server, "standalone-frame-state", "iframe=ready");
        wait_for_observed(&child_server, "WEB_HARNESS_MARKER_v1");

        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-frame-roundtrip-{}", fixture.pid);
            let (target, tab, snapshot) = bind(fixture, &session);
            assert_eq!(
                snapshot.structured()["oopif"]["status"],
                "attached",
                "real Chromium did not expose a capability-tested OOPIF: {}",
                snapshot.raw
            );

            let shadow_button = ref_by_frame_label(&snapshot, "main", "standalone-shadow-button");
            let shadow_input = ref_by_frame_label(&snapshot, "main", "standalone-shadow-input");
            let frame_button = ref_by_frame_label(&snapshot, "iframe", "standalone-frame-button");
            let frame_input = ref_by_frame_label(&snapshot, "iframe", "standalone-frame-input");
            let oopif_button = ref_by_frame_label(&snapshot, "oopif", "id=btn-increment");
            let oopif_input = ref_by_frame_label(&snapshot, "oopif", "id=txt-input");

            for reference in [&shadow_button, &frame_button, &oopif_button] {
                let clicked = fixture.driver.call(
                    "browser_click",
                    serde_json::json!({
                        "target_id": target,
                        "tab_id": tab,
                        "ref": reference,
                        "input_route": "dom_event",
                        "session": session,
                    }),
                );
                assert_eq!(clicked.structured()["status"], "ok", "{}", clicked.raw);
            }
            wait_for_text(&fixture.server, "standalone-shadow-state", "shadow=clicked");
            wait_for_text(&fixture.server, "standalone-frame-state", "iframe=clicked");
            wait_for_text(&child_server, "lbl-counter", "counter=1");

            for (reference, text) in [
                (&shadow_input, "shadow-route"),
                (&frame_input, "iframe-route"),
                (&oopif_input, "oopif-route"),
            ] {
                let typed = fixture.driver.call(
                    "browser_type",
                    serde_json::json!({
                        "target_id": target,
                        "tab_id": tab,
                        "ref": reference,
                        "text": text,
                        "session": session,
                    }),
                );
                assert_eq!(typed.structured()["status"], "ok", "{}", typed.raw);
            }
            wait_for_text(
                &fixture.server,
                "standalone-shadow-state",
                "shadow=typed:shadow-route",
            );
            wait_for_text(
                &fixture.server,
                "standalone-frame-state",
                "iframe=typed:iframe-route",
            );
            wait_for_value(&child_server, "txt-input", "oopif-route");

            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        })
    });
}

fn run_multi_tab(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-multi-tab",
        std::env::consts::OS,
        spec.name
    );
    execute_case(case(&spec.name, "multi_tab_exact_binding"), |evidence| {
        let mut fixture = launch_browser(spec, &scenario);
        *evidence = recording_evidence(fixture.driver.recording_dir());
        let session = format!("standalone-multi-tab-{}", fixture.pid);
        let _ = bind(&mut fixture, &session);
        let created = harness_cdp_call(
            fixture.cdp_port,
            "Target.createTarget",
            serde_json::json!({
                "url": format!("{}?tab=second", fixture.server.page_url()),
                "newWindow": false,
            }),
        );
        assert!(created["targetId"].is_string(), "{created}");
        wait_for_observed(&fixture.server, "new_tab=open");

        run_with_background_oracles(&mut fixture, |fixture| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let rebound = loop {
                let state = fixture.driver.call(
                    "get_browser_state",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.window_id,
                        "session": session,
                    }),
                );
                if state.structured()["status"] == "ok"
                    && state.structured()["tabs"]
                        .as_array()
                        .is_some_and(|tabs| tabs.len() >= 2)
                {
                    break state;
                }
                assert!(
                    Instant::now() < deadline,
                    "multi-tab bind did not enumerate both tabs: {}",
                    state.raw
                );
                thread::sleep(Duration::from_millis(100));
            };
            assert_eq!(rebound.structured()["binding_quality"], "exact");
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
                launch_additional_window(&mut fixture);
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

fn settle_between_browser_rows() {
    #[cfg(target_os = "macos")]
    thread::sleep(Duration::from_secs(2));
}

fn run_browser_scenario(run: fn(&BrowserSpec)) {
    let _guard = STANDALONE_BROWSER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_environment_from_env(&EnvironmentRecord::ready(Duration::ZERO))
        .expect("write standalone-browser environment evidence");
    let specs = browser_specs();
    if specs.is_empty() {
        if std::env::var_os("CUA_TEST_REQUIRE_EXTERNAL_BROWSERS").is_some() {
            panic!("no standalone Chrome, Edge, or Chromium executable was found");
        }
        eprintln!("no standalone Chromium browser installed; optional suite skipped");
        return;
    }
    let mut failed_browsers = Vec::new();
    for spec in specs {
        eprintln!(
            "[standalone-browser] {} at {}",
            spec.name,
            spec.executable.display()
        );
        if catch_unwind(AssertUnwindSafe(|| run(&spec))).is_err() {
            failed_browsers.push(spec.name.clone());
        }
        settle_between_browser_rows();
    }
    assert!(
        failed_browsers.is_empty(),
        "standalone browser scenario failed for: {}",
        failed_browsers.join(", ")
    );
}

macro_rules! standalone_browser_test {
    ($name:ident, $run:ident) => {
        #[test]
        #[ignore = "requires an installed standalone Chromium browser and an interactive desktop"]
        fn $name() {
            run_browser_scenario($run);
        }
    };
}

standalone_browser_test!(standalone_browser_roundtrip, run_roundtrip);
standalone_browser_test!(standalone_browser_background_type, run_background_type);
standalone_browser_test!(standalone_browser_trusted_click, run_trusted_click);
standalone_browser_test!(
    standalone_browser_prepare_isolated,
    run_prepare_isolated_launch
);
standalone_browser_test!(standalone_browser_stale_ref, run_stale_ref);
standalone_browser_test!(standalone_browser_frames, run_frame_roundtrip);
standalone_browser_test!(standalone_browser_multi_tab, run_multi_tab);
standalone_browser_test!(
    standalone_browser_window_collision,
    run_two_window_collision
);
