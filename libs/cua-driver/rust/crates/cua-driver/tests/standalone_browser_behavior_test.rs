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
    append_json_line, execute_case, recording_evidence, write_environment_from_env, CaseSpec,
    Delivery, DriverRoute, EnvironmentRecord, Evidence, Observation, OracleKind, RefusalCode,
    Scope, Targeting,
};
use cua_driver_testkit::observer::TargetWindow;
use cua_driver_testkit::sentinel::ForegroundSentinel;
use cua_driver_testkit::{spawn_in_job, BrowserFixtureServer, Driver, McpDriver, ToolResponse};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use cua_driver_core::browser::approval::{
    mint_existing_profile_approval, ExistingProfileApprovalScope,
};

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

fn standalone_browser_completeness_html() -> String {
    standalone_fixture_html().replace(
        "</body>",
        r#"<fieldset>
  <legend>browser completeness</legend>
  <button id="standalone-alert" data-cua-id="standalone-alert">Open alert</button>
  <button id="standalone-confirm" data-cua-id="standalone-confirm">Open confirm</button>
  <button id="standalone-prompt" data-cua-id="standalone-prompt">Open prompt</button>
  <span id="standalone-dialog-state" data-cua-id="standalone-dialog-state">dialog=none</span>
  <input id="standalone-upload" data-cua-id="standalone-upload" aria-label="standalone-upload" type="file" multiple>
  <span id="standalone-upload-state" data-cua-id="standalone-upload-state">upload=0</span>
  <a id="standalone-download" data-cua-id="standalone-download" aria-label="standalone-download" href="/download" download>Download fixture</a>
  <span id="standalone-hover-state" data-cua-id="standalone-hover-state">hover=false</span>
</fieldset>
<script>
  const dialogState = document.getElementById('standalone-dialog-state');
  document.getElementById('standalone-alert').addEventListener('click', () => {
    setTimeout(() => { alert('fixture alert'); dialogState.textContent = 'dialog=alert:accepted'; }, 0);
  });
  document.getElementById('standalone-confirm').addEventListener('click', () => {
    setTimeout(() => { dialogState.textContent = `dialog=confirm:${confirm('fixture confirm')}`; }, 0);
  });
  document.getElementById('standalone-prompt').addEventListener('click', () => {
    setTimeout(() => { dialogState.textContent = `dialog=prompt:${prompt('fixture prompt', '')}`; }, 0);
  });
  document.getElementById('standalone-upload').addEventListener('change', event => {
    const names = Array.from(event.target.files).map(file => file.name).join(',');
    document.getElementById('standalone-upload-state').textContent = `upload=${event.target.files.length}:${names}`;
  });
  document.getElementById('drag-source').setAttribute('tabindex', '0');
  document.getElementById('drag-source').setAttribute('role', 'button');
  document.getElementById('drag-source').setAttribute('draggable', 'true');
  document.getElementById('drop-target').setAttribute('tabindex', '0');
  document.getElementById('drop-target').setAttribute('role', 'button');
  document.getElementById('click-target').addEventListener('pointerover', () => {
    document.getElementById('standalone-hover-state').textContent = 'hover=true';
  });
  document.getElementById('drop-target').addEventListener('dragover', event => event.preventDefault());
  document.getElementById('drop-target').addEventListener('drop', event => {
    event.preventDefault();
    document.getElementById('drag-status').textContent = 'drag_status=dropped';
  });
</script>
</body>"#,
    )
}

fn standalone_semantic_fixture_html() -> String {
    let hidden = (0..320)
        .map(|index| {
            format!(
                r#"<button hidden aria-hidden="true" aria-label="Retained control {index}">hidden</button>"#
            )
        })
        .collect::<String>();
    let offscreen = (0..305)
        .map(|index| {
            format!(r#"<button aria-label="Archive item {index}">Archive item {index}</button>"#)
        })
        .collect::<String>();
    standalone_fixture_html()
        .replace("<body>", &format!("<body>{hidden}"))
        .replace(
            "</body>",
            &format!(
                r#"<section aria-label="Retained archive" style="margin-top:2000px">{offscreen}</section></body>"#
            ),
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

const SUPPORTED_BROWSER_PRODUCTS: &[&str] = &["chrome", "chromium", "edge"];

fn parse_browser_products(raw: &str) -> Result<Vec<String>, String> {
    let mut products = Vec::new();
    for product in raw
        .split(',')
        .map(str::trim)
        .filter(|product| !product.is_empty())
    {
        let product = product.to_ascii_lowercase();
        if !SUPPORTED_BROWSER_PRODUCTS.contains(&product.as_str()) {
            return Err(format!(
                "unsupported browser product {product:?}; expected one of {}",
                SUPPORTED_BROWSER_PRODUCTS.join(", ")
            ));
        }
        if products.contains(&product) {
            return Err(format!("duplicate browser product {product:?}"));
        }
        products.push(product);
    }
    if products.is_empty() {
        return Err("at least one browser product is required".to_owned());
    }
    Ok(products)
}

fn requested_browser_products() -> Option<Vec<String>> {
    let raw = std::env::var("CUA_E2E_BROWSER_PRODUCTS").ok()?;
    Some(
        parse_browser_products(&raw)
            .unwrap_or_else(|error| panic!("invalid CUA_E2E_BROWSER_PRODUCTS={raw:?}: {error}")),
    )
}

fn dedupe_browser_products(browsers: Vec<BrowserSpec>) -> Vec<BrowserSpec> {
    browsers.into_iter().fold(Vec::new(), |mut unique, spec| {
        if !unique
            .iter()
            .any(|existing: &BrowserSpec| existing.name == spec.name)
        {
            unique.push(spec);
        }
        unique
    })
}

#[test]
fn browser_product_selection_is_ordered_and_strict() {
    assert_eq!(
        parse_browser_products("Edge, chrome,chromium").unwrap(),
        ["edge", "chrome", "chromium"]
    );
    assert!(parse_browser_products("").is_err());
    assert!(parse_browser_products("chrome,chrome").is_err());
    assert!(parse_browser_products("firefox").is_err());

    let deduped = dedupe_browser_products(vec![
        BrowserSpec {
            name: "chromium".to_owned(),
            executable: PathBuf::from("/first/chromium"),
        },
        BrowserSpec {
            name: "edge".to_owned(),
            executable: PathBuf::from("/edge"),
        },
        BrowserSpec {
            name: "chromium".to_owned(),
            executable: PathBuf::from("/second/chromium"),
        },
    ]);
    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].executable, PathBuf::from("/first/chromium"));
}

fn select_browser_products(
    mut browsers: Vec<BrowserSpec>,
    prefer_chrome_over_chromium: bool,
) -> Vec<BrowserSpec> {
    if let Some(requested) = requested_browser_products() {
        let missing = requested
            .iter()
            .filter(|product| !browsers.iter().any(|spec| &spec.name == *product))
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "requested standalone browser products were not found: {}",
            missing.join(", ")
        );
        browsers.retain(|spec| requested.contains(&spec.name));
        browsers.sort_by_key(|spec| {
            requested
                .iter()
                .position(|product| product == &spec.name)
                .unwrap_or(usize::MAX)
        });
    } else if prefer_chrome_over_chromium && browsers.iter().any(|spec| spec.name == "chrome") {
        // Preserve the historical default lane: Chromium is the fallback when
        // Chrome is absent. Certification runs opt into each product.
        browsers.retain(|spec| spec.name != "chromium");
    }
    // A distro may expose one browser installation through several wrappers
    // (for example /usr/bin/chromium-browser and /snap/bin/chromium). The
    // certification matrix is product-scoped, so keep the first viable
    // executable for each requested product instead of reporting duplicates.
    dedupe_browser_products(browsers)
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
        let name = std::env::var("CUA_E2E_BROWSER_NAME")
            .unwrap_or_else(|_| "chromium".to_owned())
            .to_ascii_lowercase();
        return select_browser_products(vec![BrowserSpec { name, executable }], false);
    }

    #[cfg(target_os = "macos")]
    {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        return select_browser_products(
            [
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
            .collect(),
            false,
        );
    }
    #[cfg(target_os = "linux")]
    {
        let mut candidates = vec![
            ("chrome", PathBuf::from("/usr/bin/google-chrome")),
            ("chromium", PathBuf::from("/usr/bin/chromium")),
            ("chromium", PathBuf::from("/usr/bin/chromium-browser")),
            ("edge", PathBuf::from("/usr/bin/microsoft-edge")),
            ("edge", PathBuf::from("/usr/bin/microsoft-edge-stable")),
        ];
        if let Some(path) = std::env::var_os("PATH") {
            for (name, executable_name) in [
                ("chrome", "google-chrome"),
                ("chromium", "chromium"),
                ("chromium", "chromium-browser"),
                ("edge", "microsoft-edge"),
                ("edge", "microsoft-edge-stable"),
            ] {
                if let Some(executable) = std::env::split_paths(&path)
                    .map(|directory| directory.join(executable_name))
                    .find(|candidate| candidate.is_file())
                {
                    candidates.push((name, executable));
                }
            }
        }
        let browsers = candidates
            .into_iter()
            .filter(|(_, executable)| executable.is_file())
            .map(|(name, executable)| {
                let identity =
                    std::fs::canonicalize(&executable).unwrap_or_else(|_| executable.clone());
                (name, executable, identity)
            })
            .fold(Vec::new(), |mut entries, (name, executable, identity)| {
                if !entries
                    .iter()
                    .any(|(_, existing_identity): &(BrowserSpec, PathBuf)| {
                        *existing_identity == identity
                    })
                {
                    entries.push((
                        BrowserSpec {
                            name: name.to_owned(),
                            executable,
                        },
                        identity,
                    ));
                }
                entries
            })
            .into_iter()
            .map(|(spec, _)| spec)
            .collect::<Vec<_>>();
        return select_browser_products(browsers, true);
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
        return select_browser_products(
            [
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
            .collect(),
            false,
        );
    };
}

fn allocate_loopback_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("allocate standalone browser CDP port")
        .port()
}

fn browser_http_json(port: u16, path: &str) -> Result<serde_json::Value, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|error| format!("connect to harness CDP endpoint {port}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("set CDP HTTP read timeout: {error}"))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| format!("request harness CDP endpoint {path}: {error}"))?;
    let mut response = Vec::new();
    if let Err(error) = stream.read_to_end(&mut response) {
        if !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            return Err(format!("read harness CDP endpoint {path}: {error}"));
        }
    }
    let body_start = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| format!("CDP endpoint {path} returned no HTTP headers"))?;
    serde_json::from_slice(&response[body_start..])
        .map_err(|error| format!("parse CDP endpoint {path}: {error}"))
}

fn browser_endpoint_url(port: u16) -> String {
    let body = browser_http_json(port, "/json/version")
        .unwrap_or_else(|error| panic!("read harness CDP version: {error}"));
    body["webSocketDebuggerUrl"]
        .as_str()
        .expect("browser websocket URL")
        .to_owned()
}

fn wait_for_browser_endpoint(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let error = match browser_http_json(port, "/json/version") {
            Ok(body) if body["webSocketDebuggerUrl"].is_string() => return,
            Ok(body) => format!("missing browser websocket URL: {body}"),
            Err(error) => error,
        };
        assert!(
            Instant::now() < deadline,
            "standalone browser never exposed its harness CDP endpoint: {error}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn record_browser_provenance(spec: &BrowserSpec, port: u16) {
    let Some(path) = std::env::var_os("CUA_E2E_BROWSER_PROVENANCE_FILE") else {
        return;
    };
    let version = browser_http_json(port, "/json/version")
        .unwrap_or_else(|error| panic!("read browser provenance: {error}"));
    let record = serde_json::json!({
        "schema": "cua-driver-browser-provenance-v1",
        "source_sha": std::env::var("CUA_E2E_SOURCE_SHA").ok(),
        "product": spec.name,
        "browser": version.get("Browser"),
        "protocol_version": version.get("Protocol-Version"),
        "user_agent": version.get("User-Agent"),
    });
    append_json_line(Path::new(&path), &record)
        .unwrap_or_else(|error| panic!("write browser provenance: {error}"));
}

fn harness_cdp_call_at_url(
    ws_url: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let ws_url = ws_url.to_owned();
    let method = method.to_owned();
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

fn harness_cdp_call(port: u16, method: &str, params: serde_json::Value) -> serde_json::Value {
    harness_cdp_call_at_url(&browser_endpoint_url(port), method, params)
}

fn navigate_initial_page(port: u16, server: &BrowserFixtureServer) {
    let url = server.page_url();
    wait_for_browser_endpoint(port);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_error = None;
    let mut last_navigate = None;
    loop {
        let targets = browser_http_json(port, "/json/list")
            .unwrap_or_else(|error| panic!("read harness CDP targets: {error}"));
        let pages = targets
            .as_array()
            .into_iter()
            .flatten()
            .filter(|target| target["type"] == "page")
            .collect::<Vec<_>>();
        assert!(
            pages.len() <= 1,
            "fresh standalone browser exposed multiple page targets before fixture setup: {targets}"
        );
        if pages.first().is_some_and(|target| target["url"] == url)
            && server.contains("WEB_HARNESS_MARKER_v1")
        {
            return;
        }
        if let Some(ws_url) = pages
            .first()
            .and_then(|target| target["webSocketDebuggerUrl"].as_str())
            .filter(|_| {
                last_navigate
                    .is_none_or(|issued: Instant| issued.elapsed() >= Duration::from_secs(2))
            })
        {
            let navigated =
                harness_cdp_call_at_url(ws_url, "Page.navigate", serde_json::json!({"url": url}));
            last_navigate = Some(Instant::now());
            last_error = Some(if navigated.get("errorText").is_some() {
                navigated
            } else {
                serde_json::json!({"status": "acknowledged; waiting for target URL commit"})
            });
        }
        assert!(
            Instant::now() < deadline,
            "initial fixture navigation did not commit on the fresh page target: {}; targets={targets}",
            last_error
                .as_ref()
                .map_or_else(|| "no page target".to_owned(), serde_json::Value::to_string)
        );
        thread::sleep(Duration::from_millis(100));
    }
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

fn bring_harness_page_to_front(port: u16, url: &str) {
    let targets = browser_http_json(port, "/json/list")
        .unwrap_or_else(|error| panic!("read harness CDP targets: {error}"));
    let ws_url = targets
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| target["type"] == "page" && target["url"] == url)
        .and_then(|target| target["webSocketDebuggerUrl"].as_str())
        .unwrap_or_else(|| panic!("no harness page websocket for {url}"));
    harness_cdp_call_at_url(ws_url, "Page.bringToFront", serde_json::json!({}));
}

fn harness_page_visibility(port: u16, url: &str) -> String {
    let targets = browser_http_json(port, "/json/list")
        .unwrap_or_else(|error| panic!("read harness CDP targets: {error}"));
    let ws_url = targets
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| target["type"] == "page" && target["url"] == url)
        .and_then(|target| target["webSocketDebuggerUrl"].as_str())
        .unwrap_or_else(|| panic!("no harness page websocket for {url}"));
    harness_cdp_call_at_url(
        ws_url,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "document.visibilityState",
            "returnByValue": true,
        }),
    )["result"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("no document.visibilityState for {url}"))
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
    #[cfg(target_os = "linux")]
    let driver = if std::env::var("CUA_E2E_WAYLAND_SESSION").as_deref() == Ok("generic") {
        // Keep Sway IPC available to the out-of-band test oracle while the
        // product under test sees only standard/generic Wayland capabilities.
        McpDriver::spawn_named_with_env(label, &[("SWAYSOCK", "/dev/null/cua-e2e-withheld")])
    } else {
        McpDriver::spawn_named(label)
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
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

#[cfg(target_os = "windows")]
const TEST_BROWSER_WINDOW_SIZE: &str = "900,640";
#[cfg(not(target_os = "windows"))]
const TEST_BROWSER_WINDOW_SIZE: &str = "980,760";

#[cfg(target_os = "windows")]
const TEST_BROWSER_INITIAL_POSITION: (i32, i32) = (40, 40);
#[cfg(not(target_os = "windows"))]
const TEST_BROWSER_INITIAL_POSITION: (i32, i32) = (80, 80);

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
        .arg(format!("--window-size={TEST_BROWSER_WINDOW_SIZE}"));
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
        .arg(format!("--window-size={TEST_BROWSER_WINDOW_SIZE}"));
    configure_test_browser_sandbox(&mut command);
    #[cfg(target_os = "linux")]
    {
        configure_linux_browser_command(&mut command);
        // Linux existing-profile setup is authorized through the exact
        // Chromium checkbox in AT-SPI. Chromium otherwise commonly exposes
        // only its top-level frame when no screen reader is active.
        command.arg("--force-renderer-accessibility");
    }
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
    // Fixture navigation is complete before this wait starts. Waiting is
    // topology-neutral; relaunching is not, since the first process may
    // eventually map and leave an extra CDP page.
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
            let has_unique_active_tab = state.structured()["tabs"]
                .as_array()
                .is_some_and(|tabs| tabs.iter().filter(|tab| tab["active"] == true).count() == 1);
            if state.structured()["binding_quality"] == "exact" && has_unique_active_tab {
                return Some((window_id, state));
            }
        }
        if Instant::now() >= deadline {
            eprintln!(
                "no exact browser window binding with one active tab for prepared pid {pid}; last windows={}",
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
        "about:blank",
        TEST_BROWSER_INITIAL_POSITION,
    );
    navigate_initial_page(cdp_port, &server);
    record_browser_provenance(spec, cdp_port);
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

fn launch_unprepared_browser(spec: &BrowserSpec, label: &str) -> BrowserFixture {
    let mut driver = spawn_driver(label);
    let server = BrowserFixtureServer::start(&standalone_fixture_html());
    let profile = tempfile::Builder::new()
        .prefix("cua-e2e-existing-browser-")
        .tempdir()
        .expect("create existing browser profile");
    let before = window_ids(&mut driver);
    let mut command = command_for_unprepared_browser(
        spec,
        profile.path(),
        server.page_url(),
        TEST_BROWSER_INITIAL_POSITION,
    );
    let child = spawn_in_job(&mut command).expect("launch unprepared standalone browser");
    driver.reaper().push(child);
    let (pid, window_id) = wait_for_fixture_window(&mut driver, &before, &server)
        .expect("unprepared browser fixture window");
    driver.reaper().track_pid(pid);
    BrowserFixture {
        driver,
        server,
        _profile: profile,
        cdp_port: 0,
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
    let deadline = Instant::now() + Duration::from_secs(15);
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

fn semantic_ref_by_name(snapshot: &ToolResponse, name: &str, action: &str) -> String {
    snapshot.structured()["refs"]
        .as_array()
        .and_then(|refs| {
            refs.iter().find(|entry| {
                entry["name"] == name
                    && entry["actions"]
                        .as_array()
                        .is_some_and(|actions| actions.iter().any(|value| value == action))
            })
        })
        .and_then(|entry| entry["ref"].as_str())
        .unwrap_or_else(|| panic!("missing semantic {action} ref {name:?}: {}", snapshot.raw))
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
    // A newly mapped Wayland toplevel can briefly appear with a protocol-local
    // id before the compositor publishes its stable pid/geometry identity.
    // Model the real client preamble: re-list windows and bind only the id the
    // driver can prove exactly. Refusals remain strict; fixture setup merely
    // waits for the launched browser to finish registering with the desktop.
    let (window_id, state) =
        wait_for_exact_browser_binding(&mut fixture.driver, fixture.pid, session)
            .expect("standalone browser did not expose an exactly bindable window");
    fixture.window_id = window_id;
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

fn foreground_page_case(browser: &str, action: &str) -> CaseSpec {
    CaseSpec::delivered(
        format!("{}-{browser}-standalone-{action}", std::env::consts::OS),
        browser,
        "standalone-chromium",
        action,
        Targeting::Page,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::Cdp,
        vec![OracleKind::FixtureState],
    )
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

    // Chromium can occasionally map a fresh Windows profile as iconic even
    // when no minimized launch flag was requested. Normalize the test-owned
    // window before the recording boundary, then put the sentinel back in
    // front. Dispatch still requires the target to be visibly mapped and
    // fully occluded; this does not retry or weaken the action assertion.
    let raised = fixture.driver.call(
        "bring_to_front",
        serde_json::json!({
            "pid": target.pid,
            "window_id": target.native_id,
        }),
    );
    assert!(
        !raised.is_error(),
        "normalize standalone browser window posture: {}",
        raised.raw
    );
    sentinel
        .prepare_background_observation(&mut fixture.driver, target)
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

fn run_semantic_state(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-semantic-state",
        std::env::consts::OS,
        spec.name
    );
    execute_case(case(&spec.name, "browser_semantic_state"), |evidence| {
        let mut fixture =
            launch_browser_with_html(spec, &scenario, standalone_semantic_fixture_html());
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-semantic-state-{}", fixture.pid);
            let (target, tab, _) = bind(fixture, &session);
            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                }),
            );
            assert_eq!(snapshot.structured()["status"], "ok", "{}", snapshot.raw);
            assert_eq!(
                snapshot.structured()["snapshot"]["format"],
                "semantic_v2",
                "{}",
                snapshot.raw
            );
            assert!(
                snapshot.structured()["outline"]
                    .as_str()
                    .is_some_and(|outline| outline.contains("WEB_HARNESS_MARKER_v1")),
                "visible fixture marker missing from semantic outline: {}",
                snapshot.raw
            );
            assert!(
                snapshot.structured()["snapshot"]["omitted"]["css_hidden"]
                    .as_u64()
                    .is_some_and(|count| count >= 320),
                "hidden retained controls were not accounted for: {}",
                snapshot.raw
            );
            assert!(
                snapshot.structured()["snapshot"]["continuation"].is_string(),
                "offscreen fixture state did not produce continuation: {}",
                snapshot.raw
            );
            assert!(
                snapshot.structured()["refs"]
                    .as_array()
                    .is_some_and(|refs| refs.iter().all(|entry| {
                        !entry["name"]
                            .as_str()
                            .unwrap_or_default()
                            .starts_with("Retained control")
                    })),
                "hidden retained action leaked into semantic refs: {}",
                snapshot.raw
            );

            let increment_ref = semantic_ref_by_name(&snapshot, "Increment", "click");
            let clicked = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": increment_ref,
                    "input_route": "dom_event",
                    "session": session,
                }),
            );
            assert_eq!(clicked.structured()["status"], "ok", "{}", clicked.raw);
            wait_for_text(&fixture.server, "lbl-counter", "counter=1");

            let refreshed = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                    "query": "txt-input",
                }),
            );
            let input_ref = semantic_ref_by_name(&refreshed, "txt-input", "type");
            let typed = fixture.driver.call(
                "browser_type",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": input_ref,
                    "text": "semantic-browser",
                    "session": session,
                }),
            );
            assert_eq!(typed.structured()["status"], "ok", "{}", typed.raw);
            wait_for_value(&fixture.server, "txt-input", "semantic-browser");

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
                assert_eq!(
                    click.structured()["refusal"]["detail"]["alternative_route"],
                    "dom_event",
                    "{}",
                    click.raw
                );
                assert_eq!(
                    click.structured()["refusal"]["detail"]["trusted_delivery_attempted"],
                    false,
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
                TEST_BROWSER_INITIAL_POSITION,
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

fn run_existing_profile_attach(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-existing-profile",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        case(&spec.name, "browser_prepare_existing_profile"),
        |evidence| {
            let mut fixture = launch_browser(spec, &scenario);
            *evidence = recording_evidence(fixture.driver.recording_dir());
            run_with_background_oracles(&mut fixture, |fixture| {
                let session = format!("standalone-existing-profile-{}", fixture.pid);
                let started = fixture
                    .driver
                    .call("start_session", serde_json::json!({ "session": session }));
                assert!(!started.is_error(), "start_session failed: {}", started.raw);

                // A live MCP proxy proves transport provenance, not a person's
                // approval to attach an authenticated profile.
                let unapproved = fixture.driver.call(
                    "browser_prepare",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.window_id,
                        "session": session,
                        "strategy": {"kind": "existing_profile"},
                    }),
                );
                assert_eq!(
                    unapproved.structured()["refusal"]["code"],
                    "browser_consent_required",
                    "{}",
                    unapproved.raw
                );

                let approval_token = mint_existing_profile_approval(ExistingProfileApprovalScope {
                    pid: fixture.pid as i64,
                    window_id: fixture.window_id,
                    session: session.clone(),
                })
                .expect("mint exact existing-profile approval");
                fixture.driver.start_behavior_recording();
                let prepared = fixture.driver.call(
                    "browser_prepare",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.window_id,
                        "session": session,
                        "strategy": {"kind": "existing_profile"},
                        "approval_token": approval_token,
                    }),
                );
                assert_eq!(prepared.structured()["status"], "ok", "{}", prepared.raw);
                assert_eq!(
                    prepared.structured()["action"],
                    "attached_existing_profile",
                    "{}",
                    prepared.raw
                );
                assert_eq!(
                    prepared.structured()["attachment"]["kind"],
                    "existing_profile"
                );
                assert_eq!(
                    prepared.structured()["attachment"]["capabilities_invalidated"],
                    true
                );
                assert_eq!(
                    prepared.structured()["attachment"]["next_action"],
                    "get_browser_state"
                );
                assert_eq!(
                    prepared.structured()["side_effects"]["launched_browser"],
                    false
                );
                assert_eq!(
                    prepared.structured()["side_effects"]["created_profile"],
                    false
                );
                assert_eq!(
                    prepared.structured()["side_effects"]["copied_profile_data"],
                    false
                );

                let public_result = prepared.raw.to_string();
                assert!(!public_result.contains("ws://"), "{}", prepared.raw);
                assert!(
                    !public_result.contains("webSocketDebuggerUrl"),
                    "{}",
                    prepared.raw
                );
                assert!(!public_result.contains(&approval_token), "{}", prepared.raw);
                assert!(
                    !public_result.contains(&fixture._profile.path().display().to_string()),
                    "{}",
                    prepared.raw
                );

                let state = fixture.driver.call(
                    "get_browser_state",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.window_id,
                        "session": session,
                    }),
                );
                assert_eq!(
                    state.structured()["binding_quality"],
                    "exact",
                    "{}",
                    state.raw
                );
                let target = state.structured()["target_id"]
                    .as_str()
                    .expect("existing-profile target")
                    .to_owned();
                let tab = state.structured()["tabs"]
                    .as_array()
                    .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
                    .and_then(|tab| tab["tab_id"].as_str())
                    .expect("existing-profile active tab")
                    .to_owned();
                let snapshot = fixture.driver.call(
                    "get_browser_state",
                    serde_json::json!({
                        "target_id": target,
                        "tab_id": tab,
                        "session": session,
                    }),
                );
                let click_ref = ref_by_label(&snapshot, "id=btn-increment");
                let clicked = fixture.driver.call(
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
                wait_for_text(&fixture.server, "lbl-counter", "counter=1");

                let ended = fixture
                    .driver
                    .call("end_session", serde_json::json!({ "session": session }));
                assert!(!ended.is_error(), "end_session failed: {}", ended.raw);
                let windows = fixture
                    .driver
                    .call("list_windows", serde_json::json!({"pid": fixture.pid}));
                assert!(
                    windows.structured()["windows"]
                        .as_array()
                        .is_some_and(|windows| windows.iter().any(|window| {
                            window["window_id"].as_u64() == Some(fixture.window_id)
                        })),
                    "ending an attachment grant must not close the user-owned browser: {}",
                    windows.raw
                );

                Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
            })
        },
    );
}

fn run_existing_profile_setup(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-existing-profile-setup",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        case(&spec.name, "browser_prepare_existing_profile_setup"),
        |evidence| {
            let mut fixture = launch_unprepared_browser(spec, &scenario);
            *evidence = recording_evidence(fixture.driver.recording_dir());
            let session = format!("standalone-existing-profile-setup-{}", fixture.pid);
            let started = fixture
                .driver
                .call("start_session", serde_json::json!({ "session": session }));
            assert!(!started.is_error(), "start_session failed: {}", started.raw);

            let approval_token = mint_existing_profile_approval(ExistingProfileApprovalScope {
                pid: fixture.pid as i64,
                window_id: fixture.window_id,
                session: session.clone(),
            })
            .expect("mint exact existing-profile setup approval");
            fixture.driver.start_behavior_recording();
            let prepared = fixture.driver.call(
                "browser_prepare",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.window_id,
                    "session": session,
                    "strategy": {"kind": "existing_profile"},
                    "approval_token": approval_token,
                }),
            );
            assert_eq!(prepared.structured()["status"], "ok", "{}", prepared.raw);
            assert_eq!(
                prepared.structured()["action"],
                "attached_existing_profile",
                "{}",
                prepared.raw
            );
            assert_eq!(
                prepared.structured()["side_effects"]["opened_setup_page"],
                true,
                "{}",
                prepared.raw
            );
            assert_eq!(
                prepared.structured()["side_effects"]["closed_setup_page"],
                true,
                "{}",
                prepared.raw
            );
            assert_eq!(
                prepared.structured()["side_effects"]["enabled_remote_debugging"],
                true,
                "{}",
                prepared.raw
            );
            assert_eq!(
                prepared.structured()["side_effects"]["launched_browser"],
                false
            );
            assert_eq!(
                prepared.structured()["side_effects"]["created_profile"],
                false
            );

            let public_result = prepared.raw.to_string();
            assert!(!public_result.contains("ws://"), "{}", prepared.raw);
            assert!(!public_result.contains(&approval_token), "{}", prepared.raw);
            assert!(
                !public_result.contains(&fixture._profile.path().display().to_string()),
                "{}",
                prepared.raw
            );

            run_with_background_oracles(&mut fixture, |fixture| {
                let state = fixture.driver.call(
                    "get_browser_state",
                    serde_json::json!({
                        "pid": fixture.pid as i64,
                        "window_id": fixture.window_id,
                        "session": session,
                    }),
                );
                assert_eq!(
                    state.structured()["binding_quality"],
                    "exact",
                    "{}",
                    state.raw
                );
                let target = state.structured()["target_id"]
                    .as_str()
                    .expect("existing-profile setup target")
                    .to_owned();
                let tab = state.structured()["tabs"]
                    .as_array()
                    .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
                    .and_then(|tab| tab["tab_id"].as_str())
                    .expect("existing-profile setup active tab")
                    .to_owned();
                let snapshot = fixture.driver.call(
                    "get_browser_state",
                    serde_json::json!({
                        "target_id": target,
                        "tab_id": tab,
                        "session": session,
                    }),
                );
                let click_ref = ref_by_label(&snapshot, "id=btn-increment");
                let clicked = fixture.driver.call(
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
                wait_for_text(&fixture.server, "lbl-counter", "counter=1");

                let ended = fixture
                    .driver
                    .call("end_session", serde_json::json!({ "session": session }));
                assert!(!ended.is_error(), "end_session failed: {}", ended.raw);
                let windows = fixture
                    .driver
                    .call("list_windows", serde_json::json!({"pid": fixture.pid}));
                assert!(
                    windows.structured()["windows"]
                        .as_array()
                        .is_some_and(|windows| windows.iter().any(|window| {
                            window["window_id"].as_u64() == Some(fixture.window_id)
                        })),
                    "ending the setup grant must not close the user-owned browser: {}",
                    windows.raw
                );
                Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
            })
        },
    );
}

#[cfg(target_os = "linux")]
fn run_generic_wayland_existing_profile_refusal(spec: &BrowserSpec) {
    assert!(
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            && std::env::var("CUA_E2E_WAYLAND_SESSION").as_deref() == Ok("generic"),
        "the generic-Wayland refusal row requires its explicit native-Wayland lane"
    );
    assert!(
        platform_linux::wayland::shell_helper::trusted_window_ids_for_pid(std::process::id())
            .is_none(),
        "the generic-Wayland refusal row requires the attested GNOME helper to be unavailable"
    );
    let scenario = format!(
        "{}-{}-standalone-generic-wayland-existing-profile-refusal",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        refusal_case(
            &spec.name,
            "browser_prepare_existing_profile_generic_wayland",
            RefusalCode::BrowserBindingAmbiguous,
        ),
        |evidence| {
            let mut driver = spawn_driver(&scenario);
            let server = BrowserFixtureServer::start(&standalone_fixture_html());
            let profile = tempfile::Builder::new()
                .prefix("cua-e2e-generic-wayland-browser-")
                .tempdir()
                .expect("create generic Wayland browser profile");
            let mut command = command_for_unprepared_browser(
                spec,
                profile.path(),
                server.page_url(),
                TEST_BROWSER_INITIAL_POSITION,
            );
            let child = spawn_in_job(&mut command).expect("launch generic Wayland browser");
            let pid = child.id();
            driver.reaper().push(child);
            driver.reaper().track_pid(pid);
            wait_for_observed(&server, "WEB_HARNESS_MARKER_v1");

            let windows = driver.call("list_windows", serde_json::json!({"pid": pid}));
            let opaque_window = windows.structured()["windows"]
                .as_array()
                .and_then(|windows| windows.iter().find(|window| window["pid"] == pid))
                .unwrap_or_else(|| {
                    panic!(
                        "generic Wayland did not expose the browser's opaque native window: {}",
                        windows.raw
                    )
                });
            let opaque_unattested_window_id = opaque_window["window_id"]
                .as_u64()
                .expect("opaque generic Wayland window id");
            assert!(
                opaque_window["z_index"].is_null(),
                "generic Wayland must not treat an AT-SPI window as compositor-attested: {}",
                windows.raw
            );

            let sentinel = ForegroundSentinel::launch(&mut driver);
            *evidence = recording_evidence(driver.recording_dir());
            driver.start_behavior_recording();
            let (mut observation, passed) = sentinel
                .observe_desktop(|| {
                    let session = format!("generic-wayland-refusal-{pid}");
                    let started =
                        driver.call("start_session", serde_json::json!({ "session": session }));
                    assert!(!started.is_error(), "start_session failed: {}", started.raw);
                    let approval_token =
                        mint_existing_profile_approval(ExistingProfileApprovalScope {
                            pid: pid as i64,
                            window_id: opaque_unattested_window_id,
                            session: session.clone(),
                        })
                        .expect("mint generic-Wayland adversarial approval");
                    let refused = driver.call(
                        "browser_prepare",
                        serde_json::json!({
                            "pid": pid as i64,
                            "window_id": opaque_unattested_window_id,
                            "session": session,
                            "strategy": {"kind": "existing_profile"},
                            "approval_token": approval_token,
                        }),
                    );
                    assert_eq!(
                        refused.structured()["refusal"]["code"],
                        "browser_binding_ambiguous",
                        "{}",
                        refused.raw
                    );
                    assert!(
                        refused.structured()["refusal"]["detail"]["setup_side_effects"].is_null(),
                        "generic Wayland must refuse before setup mutation: {}",
                        refused.raw
                    );
                    wait_for_text(&server, "lbl-counter", "counter=0");
                    Observation::refused(
                        RefusalCode::BrowserBindingAmbiguous,
                        vec![OracleKind::FixtureState],
                        refused.text(),
                        Evidence::default(),
                    )
                })
                .expect("observe generic Wayland refusal desktop effects");
            observation.passed_oracles.extend(passed);
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
        let second_html = standalone_fixture_html().replace(
            "<title>cua-driver Web Harness</title>",
            "<title>cua-driver Background Harness</title>",
        );
        let second_server = BrowserFixtureServer::start(&second_html);
        let created = harness_cdp_call(
            fixture.cdp_port,
            "Target.createTarget",
            serde_json::json!({
                "url": second_server.page_url(),
                "newWindow": false,
                "background": true,
            }),
        );
        assert!(created["targetId"].is_string(), "{created}");
        wait_for_observed(&second_server, "WEB_HARNESS_MARKER_v1");

        // Establish a deterministic selected-tab baseline before the
        // foreground sentinel starts. Chromium does not consistently honor
        // createTarget(background=true) on every host, but Page.bringToFront
        // reliably selects the fixture tab. No browser action below this
        // setup point may activate a target.
        bring_harness_page_to_front(fixture.cdp_port, &fixture.server.page_url());

        // Begin the background proof only after Chromium has honored the
        // explicit selected-tab setup and the first fixture is observably
        // active.
        let setup_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let foreground_visibility =
                harness_page_visibility(fixture.cdp_port, &fixture.server.page_url());
            let background_visibility =
                harness_page_visibility(fixture.cdp_port, &second_server.page_url());
            if foreground_visibility == "visible" && background_visibility == "hidden" {
                break;
            }
            assert!(
                Instant::now() < setup_deadline,
                "fixture setup did not select the first tab: foreground={foreground_visibility}, background={background_visibility}"
            );
            thread::sleep(Duration::from_millis(100));
        }

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
            let target = rebound.structured()["target_id"]
                .as_str()
                .expect("multi-tab target")
                .to_owned();
            let tabs = rebound.structured()["tabs"]
                .as_array()
                .expect("multi-tab list");
            let background_tab = tabs
                .iter()
                .find(|tab| tab["url"] == second_server.page_url())
                .and_then(|tab| tab["tab_id"].as_str())
                .expect("inactive second tab")
                .to_owned();

            let navigated_url = format!("{}?background=navigated", second_server.page_url());
            let navigated = fixture.driver.call(
                "browser_navigate",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "url": navigated_url,
                    "session": session,
                }),
            );
            assert_eq!(navigated.structured()["status"], "ok", "{}", navigated.raw);
            wait_for_observed(&second_server, "WEB_HARNESS_MARKER_v1");

            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                    "query": "Increment txt-input",
                }),
            );
            assert_eq!(snapshot.structured()["status"], "ok", "{}", snapshot.raw);
            let click_ref = semantic_ref_by_name(&snapshot, "Increment", "click");
            let trusted = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "ref": click_ref,
                    "session": session,
                }),
            );
            let trusted_clicks = if cfg!(target_os = "windows") {
                assert_eq!(trusted.structured()["status"], "ok", "{}", trusted.raw);
                1
            } else {
                assert_eq!(
                    trusted.structured()["refusal"]["code"],
                    "browser_input_trust_unavailable",
                    "{}",
                    trusted.raw
                );
                0
            };
            wait_for_text(
                &second_server,
                "lbl-counter",
                &format!("counter={trusted_clicks}"),
            );

            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                    "query": "Increment",
                }),
            );
            let click_ref = semantic_ref_by_name(&snapshot, "Increment", "click");
            let clicked = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "ref": click_ref,
                    "input_route": "dom_event",
                    "session": session,
                }),
            );
            assert_eq!(clicked.structured()["status"], "ok", "{}", clicked.raw);

            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                    "query": "txt-input",
                }),
            );
            let input_ref = semantic_ref_by_name(&snapshot, "txt-input", "type");
            let typed = fixture.driver.call(
                "browser_type",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "ref": input_ref,
                    "text": "inactive-tab",
                    "session": session,
                }),
            );
            assert_eq!(typed.structured()["status"], "ok", "{}", typed.raw);

            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                    "query": "txt-input",
                }),
            );
            let input_ref = semantic_ref_by_name(&snapshot, "txt-input", "type");
            let keyed = fixture.driver.call(
                "browser_type",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": background_tab,
                    "ref": input_ref,
                    "text": "keys",
                    "mode": "keystrokes",
                    "session": session,
                }),
            );
            assert_eq!(keyed.structured()["status"], "ok", "{}", keyed.raw);

            wait_for_text(
                &second_server,
                "lbl-counter",
                &format!("counter={}", trusted_clicks + 1),
            );
            wait_for_value(&second_server, "txt-input", "inactive-tabkeys");
            wait_for_text(&fixture.server, "lbl-counter", "counter=0");
            wait_for_value(&fixture.server, "txt-input", "");

            let verified = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "pid": fixture.pid as i64,
                    "window_id": fixture.window_id,
                    "session": session,
                }),
            );
            let tabs = verified.structured()["tabs"]
                .as_array()
                .expect("verified multi-tab list");
            assert!(
                tabs.iter().any(|tab| {
                    tab["active"] == true && tab["url"] == fixture.server.page_url()
                }),
                "the first tab must remain selected: {}",
                verified.raw
            );
            assert!(
                tabs.iter()
                    .any(|tab| { tab["active"] == false && tab["url"] == navigated_url }),
                "the mutated second tab must remain unselected: {}",
                verified.raw
            );
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        })
    });
}

fn run_same_title_tabs(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-same-title-tabs",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        case(&spec.name, "same_title_tab_selection_unknown"),
        |evidence| {
            let mut fixture = launch_browser(spec, &scenario);
            *evidence = recording_evidence(fixture.driver.recording_dir());
            let session = format!("standalone-same-title-{}", fixture.pid);
            let _ = bind(&mut fixture, &session);
            let second_server = BrowserFixtureServer::start(&standalone_fixture_html());
            let created = harness_cdp_call(
                fixture.cdp_port,
                "Target.createTarget",
                serde_json::json!({
                    "url": second_server.page_url(),
                    "newWindow": false,
                    "background": true,
                }),
            );
            assert!(created["targetId"].is_string(), "{created}");
            wait_for_observed(&second_server, "WEB_HARNESS_MARKER_v1");
            bring_harness_page_to_front(fixture.cdp_port, &fixture.server.page_url());

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
                    if state.structured()["tabs"]
                        .as_array()
                        .is_some_and(|tabs| tabs.len() >= 2)
                    {
                        break state;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "same-title bind failed: {}",
                        state.raw
                    );
                    thread::sleep(Duration::from_millis(100));
                };
                assert_eq!(rebound.structured()["binding_quality"], "exact");
                let tabs = rebound.structured()["tabs"]
                    .as_array()
                    .expect("same-title tab list");
                assert!(tabs
                    .iter()
                    .any(|tab| tab["url"] == fixture.server.page_url()));
                assert!(tabs
                    .iter()
                    .any(|tab| tab["url"] == second_server.page_url()));
                assert!(
                    tabs.iter().all(|tab| tab["active"].is_null()),
                    "same-title selection must be unknown instead of guessed: {}",
                    rebound.raw
                );
                Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
            })
        },
    );
}

fn run_dialogs(spec: &BrowserSpec) {
    let scenario = format!("{}-{}-standalone-dialogs", std::env::consts::OS, spec.name);
    let spec_case = if cfg!(target_os = "linux") {
        foreground_page_case(&spec.name, "browser_dialogs")
    } else {
        case(&spec.name, "browser_dialogs")
    };
    execute_case(spec_case, |evidence| {
        let mut fixture =
            launch_browser_with_html(spec, &scenario, standalone_browser_completeness_html());
        *evidence = recording_evidence(fixture.driver.recording_dir());
        let session = format!("standalone-dialogs-{}", fixture.pid);
        let (target_id, tab_id, snapshot) = bind(&mut fixture, &session);
        let primed = fixture.driver.call(
            "browser_dialog",
            serde_json::json!({
                "target_id": target_id,
                "tab_id": tab_id,
                "action": "inspect",
                "session": session,
            }),
        );
        assert_eq!(primed.structured()["present"], false, "{}", primed.raw);

        let sentinel = if cfg!(target_os = "linux") {
            None
        } else {
            Some(ForegroundSentinel::launch(&mut fixture.driver))
        };
        let target = TargetWindow {
            pid: fixture.pid,
            native_id: fixture.window_id,
        };
        fixture.driver.start_behavior_recording();
        let mut passed_oracles = vec![OracleKind::FixtureState];

        for (button, kind, action, prompt_text, expected) in [
            (
                "id=standalone-alert",
                "alert",
                "accept",
                None,
                "dialog=alert:accepted",
            ),
            (
                "id=standalone-confirm",
                "confirm",
                "dismiss",
                None,
                "dialog=confirm:false",
            ),
            (
                "id=standalone-prompt",
                "prompt",
                "accept",
                Some("fixture response"),
                "dialog=prompt:fixture response",
            ),
        ] {
            // Opening a native browser modal can activate Chromium. Treat that
            // as fixture setup, then prove the typed inspect/resolve operation
            // itself preserves a fully occluded background posture.
            let clicked = fixture.driver.call(
                "browser_click",
                serde_json::json!({
                    "target_id": target_id,
                    "tab_id": tab_id,
                    "ref": ref_by_label(&snapshot, button),
                    "input_route": "dom_event",
                    "session": session,
                }),
            );
            assert_eq!(clicked.structured()["status"], "ok", "{}", clicked.raw);
            thread::sleep(Duration::from_millis(100));

            if cfg!(target_os = "linux") {
                let deadline = Instant::now() + Duration::from_secs(5);
                let current = loop {
                    let current = fixture.driver.call(
                        "browser_dialog",
                        serde_json::json!({
                            "target_id": target_id,
                            "tab_id": tab_id,
                            "action": "inspect",
                            "session": session,
                        }),
                    );
                    if current.structured()["present"] == true {
                        break current;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{kind} dialog was not observed: {}",
                        current.raw
                    );
                    thread::sleep(Duration::from_millis(25));
                };
                assert_eq!(current.structured()["kind"], kind, "{}", current.raw);
                let dialog_id = current.structured()["dialog_id"]
                    .as_str()
                    .expect("opaque dialog id")
                    .to_owned();
                let mut args = serde_json::json!({
                    "target_id": target_id,
                    "tab_id": tab_id,
                    "action": action,
                    "dialog_id": dialog_id,
                    "delivery_mode": "foreground",
                    "session": session,
                });
                if let Some(prompt_text) = prompt_text {
                    args["prompt_text"] = serde_json::Value::String(prompt_text.to_owned());
                }
                let handled = fixture.driver.call("browser_dialog", args);
                assert_eq!(handled.structured()["status"], "ok", "{}", handled.raw);
                wait_for_text(&fixture.server, "standalone-dialog-state", expected);
                continue;
            }

            sentinel
                .as_ref()
                .expect("background dialog sentinel")
                .prepare_background_observation(&mut fixture.driver, target)
                .expect("occlude the open JavaScript dialog");

            let (_, passed) = sentinel
                .as_ref()
                .expect("background dialog sentinel")
                .observe_background(target, || {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let current = loop {
                        let current = fixture.driver.call(
                            "browser_dialog",
                            serde_json::json!({
                                "target_id": target_id,
                                "tab_id": tab_id,
                                "action": "inspect",
                                "session": session,
                            }),
                        );
                        if current.structured()["present"] == true {
                            break current;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "{kind} dialog was not observed: {}",
                            current.raw
                        );
                        thread::sleep(Duration::from_millis(25));
                    };
                    assert_eq!(current.structured()["kind"], kind, "{}", current.raw);
                    let dialog_id = current.structured()["dialog_id"]
                        .as_str()
                        .expect("opaque dialog id")
                        .to_owned();
                    let mut args = serde_json::json!({
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "action": action,
                        "dialog_id": dialog_id,
                        "session": session,
                    });
                    if let Some(prompt_text) = prompt_text {
                        args["prompt_text"] = serde_json::Value::String(prompt_text.to_owned());
                    }
                    let handled = fixture.driver.call("browser_dialog", args);
                    assert_eq!(handled.structured()["status"], "ok", "{}", handled.raw);
                    wait_for_text(&fixture.server, "standalone-dialog-state", expected);
                    Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
                })
                .expect("observe background JavaScript dialog resolution");
            passed_oracles.extend(passed);
        }

        Observation::delivered(passed_oracles, Evidence::default())
    });
}

#[cfg(target_os = "linux")]
fn run_dialog_background_refusal(spec: &BrowserSpec) {
    let scenario = format!(
        "{}-{}-standalone-dialog-background-refusal",
        std::env::consts::OS,
        spec.name
    );
    execute_case(
        refusal_case(
            &spec.name,
            "browser_dialogs_background",
            RefusalCode::BrowserInputTrustUnavailable,
        ),
        |evidence| {
            let mut fixture =
                launch_browser_with_html(spec, &scenario, standalone_browser_completeness_html());
            *evidence = recording_evidence(fixture.driver.recording_dir());
            run_with_background_oracles(&mut fixture, |fixture| {
                let session = format!("standalone-dialog-refusal-{}", fixture.pid);
                let (target_id, tab_id, _) = bind(fixture, &session);
                let refused = fixture.driver.call(
                    "browser_dialog",
                    serde_json::json!({
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "action": "accept",
                        "dialog_id": "dialog-unavailable",
                        "session": session,
                    }),
                );
                assert_eq!(refused.structured()["status"], "refused", "{}", refused.raw);
                assert_eq!(
                    refused.structured()["refusal"]["code"],
                    "browser_input_trust_unavailable",
                    "{}",
                    refused.raw
                );
                wait_for_text(&fixture.server, "standalone-dialog-state", "dialog=none");
                Observation::refused(
                    RefusalCode::BrowserInputTrustUnavailable,
                    vec![OracleKind::FixtureState],
                    refused.text(),
                    Evidence::default(),
                )
            })
        },
    );
}

fn run_upload(spec: &BrowserSpec) {
    let scenario = format!("{}-{}-standalone-upload", std::env::consts::OS, spec.name);
    execute_case(case(&spec.name, "browser_file_upload"), |evidence| {
        let mut fixture =
            launch_browser_with_html(spec, &scenario, standalone_browser_completeness_html());
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-upload-{}", fixture.pid);
            let (target, tab, _) = bind(fixture, &session);
            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                    "query": "standalone-upload",
                }),
            );
            let upload_ref = semantic_ref_by_name(&snapshot, "standalone-upload", "upload");
            let directory = tempfile::tempdir().expect("create upload fixture directory");
            let path = directory.path().join("fixture-upload.txt");
            std::fs::write(&path, b"fixture upload payload").expect("write upload fixture");
            let canonical = std::fs::canonicalize(&path).expect("canonical upload fixture");
            let uploaded = fixture.driver.call(
                "browser_set_input_files",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": upload_ref,
                    "files": [canonical],
                    "session": session,
                }),
            );
            assert_eq!(uploaded.structured()["status"], "ok", "{}", uploaded.raw);
            assert_eq!(uploaded.structured()["file_count"], 1, "{}", uploaded.raw);
            let public = uploaded.raw.to_string();
            assert!(
                !public.contains(&path.display().to_string()),
                "{}",
                uploaded.raw
            );
            assert!(!public.contains("fixture-upload.txt"), "{}", uploaded.raw);
            wait_for_text(
                &fixture.server,
                "standalone-upload-state",
                "upload=1:fixture-upload.txt",
            );
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        })
    });
}

fn run_pointer_actions(spec: &BrowserSpec) {
    let scenario = format!("{}-{}-standalone-pointer", std::env::consts::OS, spec.name);
    execute_case(case(&spec.name, "browser_pointer_actions"), |evidence| {
        let mut fixture =
            launch_browser_with_html(spec, &scenario, standalone_browser_completeness_html());
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-pointer-{}", fixture.pid);
            let (target, tab, _) = bind(fixture, &session);
            let snapshot = fixture.driver.call(
                "get_browser_state",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "session": session,
                    "snapshot_format": "semantic_v2",
                }),
            );
            assert_eq!(snapshot.structured()["status"], "ok", "{}", snapshot.raw);

            let content_ref = snapshot.structured()["content_refs"]
                .as_array()
                .and_then(|refs| refs.first())
                .and_then(|entry| entry["ref"].as_str())
                .expect("semantic snapshot content ref");
            let refused = fixture.driver.call(
                "browser_pointer",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": content_ref,
                    "action": "hover",
                    "input_route": "dom_event",
                    "session": session,
                }),
            );
            assert_eq!(
                refused.structured()["refusal"]["code"],
                "browser_action_unavailable",
                "{}",
                refused.raw
            );

            let click_ref = semantic_ref_by_name(&snapshot, "border-click-target", "pointer");

            for action in ["hover", "right_click", "double_click"] {
                let response = fixture.driver.call(
                    "browser_pointer",
                    serde_json::json!({
                        "target_id": target,
                        "tab_id": tab,
                        "ref": click_ref,
                        "action": action,
                        "input_route": "dom_event",
                        "session": session,
                    }),
                );
                assert_eq!(response.structured()["status"], "ok", "{}", response.raw);
            }
            wait_for_text(&fixture.server, "standalone-hover-state", "hover=true");
            wait_for_text(
                &fixture.server,
                "lbl-last-action",
                "last_action=double_click",
            );

            let scrolled = fixture.driver.call(
                "browser_pointer",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": semantic_ref_by_name(&snapshot, "scroll-tall", "scroll"),
                    "action": "scroll",
                    "input_route": "dom_event",
                    "delta_y": 240,
                    "session": session,
                }),
            );
            assert_eq!(scrolled.structured()["status"], "ok", "{}", scrolled.raw);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if fixture
                    .server
                    .text("lbl-scroll-offset")
                    .is_some_and(|value| value != "scroll_offset=0")
                {
                    break;
                }
                assert!(Instant::now() < deadline, "scroll oracle did not change");
                thread::sleep(Duration::from_millis(25));
            }

            let dragged = fixture.driver.call(
                "browser_pointer",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": semantic_ref_by_name(&snapshot, "drag-source", "pointer"),
                    "destination_ref": semantic_ref_by_name(&snapshot, "drop-target", "pointer"),
                    "action": "drag",
                    "input_route": "dom_event",
                    "session": session,
                }),
            );
            assert_eq!(dragged.structured()["status"], "ok", "{}", dragged.raw);
            wait_for_text(&fixture.server, "drag-status", "drag_status=dropped");
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
        })
    });
}

fn run_download(spec: &BrowserSpec) {
    let scenario = format!("{}-{}-standalone-download", std::env::consts::OS, spec.name);
    execute_case(case(&spec.name, "browser_download"), |evidence| {
        let mut fixture =
            launch_browser_with_html(spec, &scenario, standalone_browser_completeness_html());
        *evidence = recording_evidence(fixture.driver.recording_dir());
        run_with_background_oracles(&mut fixture, |fixture| {
            let session = format!("standalone-download-{}", fixture.pid);
            let (target, tab, snapshot) = bind(fixture, &session);
            // Sandboxed browser packages such as Ubuntu's Chromium snap use a
            // private /tmp namespace. A temporary directory under HOME remains
            // visible to both the browser and the driver while still being
            // removed automatically when the fixture exits.
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .expect("browser E2E home directory");
            let destination = tempfile::Builder::new()
                // Snap grants ordinary home-directory access but intentionally
                // excludes arbitrary dotfiles and hidden directories.
                .prefix("cua-e2e-download-")
                .tempdir_in(home)
                .expect("create approved download directory");
            let canonical =
                std::fs::canonicalize(destination.path()).expect("canonical download directory");
            let downloaded = fixture.driver.call(
                "browser_download",
                serde_json::json!({
                    "target_id": target,
                    "tab_id": tab,
                    "ref": ref_by_label(&snapshot, "id=standalone-download"),
                    "destination_root": canonical,
                    "session": session,
                }),
            );
            assert_eq!(
                downloaded.structured()["status"],
                "completed",
                "{}",
                downloaded.raw
            );
            let entries = std::fs::read_dir(destination.path())
                .expect("read approved download directory")
                .collect::<Result<Vec<_>, _>>()
                .expect("enumerate approved download directory");
            assert_eq!(entries.len(), 1, "expected one completed download");
            let bytes = std::fs::read(entries[0].path()).expect("read completed download");
            assert_eq!(bytes, b"CUA_DRIVER_BROWSER_DOWNLOAD_FIXTURE_V1\n");
            assert_eq!(
                downloaded.structured()["bytes"],
                bytes.len() as u64,
                "{}",
                downloaded.raw
            );
            let public = downloaded.raw.to_string();
            assert!(!public.contains(&destination.path().display().to_string()));
            assert!(!public.contains("fixture.txt"));
            assert!(!public.contains(fixture.server.download_url()));
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
standalone_browser_test!(standalone_browser_semantic_state, run_semantic_state);
standalone_browser_test!(standalone_browser_background_type, run_background_type);
standalone_browser_test!(standalone_browser_trusted_click, run_trusted_click);
standalone_browser_test!(
    standalone_browser_prepare_isolated,
    run_prepare_isolated_launch
);
standalone_browser_test!(
    standalone_browser_existing_profile,
    run_existing_profile_attach
);
standalone_browser_test!(
    standalone_browser_existing_profile_setup,
    run_existing_profile_setup
);
#[cfg(target_os = "linux")]
standalone_browser_test!(
    standalone_browser_generic_wayland_existing_profile_refusal,
    run_generic_wayland_existing_profile_refusal
);
standalone_browser_test!(standalone_browser_stale_ref, run_stale_ref);
standalone_browser_test!(standalone_browser_frames, run_frame_roundtrip);
standalone_browser_test!(standalone_browser_multi_tab, run_multi_tab);
standalone_browser_test!(standalone_browser_same_title_tabs, run_same_title_tabs);
standalone_browser_test!(standalone_browser_dialogs, run_dialogs);
#[cfg(target_os = "linux")]
standalone_browser_test!(
    standalone_browser_dialog_background_refusal,
    run_dialog_background_refusal
);
standalone_browser_test!(standalone_browser_upload, run_upload);
standalone_browser_test!(standalone_browser_pointer_actions, run_pointer_actions);
standalone_browser_test!(standalone_browser_download, run_download);
standalone_browser_test!(
    standalone_browser_window_collision,
    run_two_window_collision
);
