//! Full-desktop foreground sentinel used by background E2E cells.

use std::fs;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::e2e::OracleKind;
use crate::observer::{DesktopObserver, NativeObserver, TargetWindow};
use crate::{harness_app, spawn_in_job, BehaviorRecording, ChildReaper, Driver};

/// A foreground Electron window that journals focus and leaked input while it
/// fully occludes the background target.
pub struct ForegroundSentinel {
    journal_path: std::path::PathBuf,
    target: TargetWindow,
    _process_heartbeat: Option<ProcessHeartbeat>,
    _reaper: ChildReaper,
    _user_data: tempfile::TempDir,
}

struct ProcessHeartbeat {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProcessHeartbeat {
    fn launch(pid: u32, journal_path: &std::path::Path) -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            if std::env::var("CUA_E2E_WAYLAND_SESSION")
                .is_ok_and(|session| session == "cua-compositor")
            {
                let (_, started) = linux_process_state(pid)?;
                let journal_path = journal_path.to_owned();
                let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let thread_stop = stop.clone();
                let thread = std::thread::spawn(move || {
                    while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let Some((state, current_started)) = linux_process_state(pid) else {
                            break;
                        };
                        if state == 'Z' || current_started != started {
                            break;
                        }
                        let at_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|duration| duration.as_millis())
                            .unwrap_or_default();
                        if let Ok(mut journal) =
                            std::fs::OpenOptions::new().append(true).open(&journal_path)
                        {
                            use std::io::Write;
                            let _ = writeln!(
                                journal,
                                "{{\"kind\":\"heartbeat\",\"at_ms\":{at_ms},\"source\":\"process-liveness\"}}"
                            );
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
                return Some(Self {
                    stop,
                    thread: Some(thread),
                });
            }
        }
        let _ = (pid, journal_path);
        None
    }
}

impl Drop for ProcessHeartbeat {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_process_state(pid: u32) -> Option<(char, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(')')?.1.trim();
    let mut fields = tail.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let started = fields.nth(18)?.parse::<u64>().ok()?;
    Some((state, started))
}

impl ForegroundSentinel {
    pub fn launch(driver: &mut impl Driver) -> Self {
        let electron = electron_fixture();
        assert!(
            electron.path.exists(),
            "Electron sentinel fixture is missing at {}",
            electron.path.display()
        );
        let user_data = tempfile::Builder::new()
            .prefix("cua-e2e-sentinel-")
            .tempdir()
            .expect("create sentinel user-data directory");
        let journal_path = user_data.path().join("sentinel-events.jsonl");
        fs::write(&journal_path, "").expect("initialize sentinel event journal");
        let cdp_port = TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .expect("allocate sentinel CDP port")
            .port();
        let mut command = Command::new(&electron.path);
        command
            .args(&electron.args)
            .env("CUA_E2E_SENTINEL", "1")
            .env("CUA_E2E_SENTINEL_JOURNAL", &journal_path)
            .env("CUA_E2E_USER_DATA_DIR", user_data.path())
            .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_in_job(&mut command).expect("launch foreground sentinel");
        let launched_pid = child.id();
        // The deliberately minimal compositor can block both Chromium event
        // loops during a synchronous configure/focus transition. In that one
        // lane, prove liveness from the exact child PID + Linux start time.
        // Renderer input and native focus canaries below remain independent
        // GUI-responsiveness oracles.
        let process_heartbeat = ProcessHeartbeat::launch(launched_pid, &journal_path);
        let mut reaper = ChildReaper::new();
        reaper.push(child);
        let expected_title = format!("CuaTestHarness Sentinel [cdp={cdp_port}]");

        let window_deadline = Instant::now() + Duration::from_secs(15);
        let target = loop {
            let windows = driver.call("list_windows", serde_json::json!({}));
            if let Some(target) = windows.structured()["windows"]
                .as_array()
                .and_then(|windows| {
                    windows.iter().find_map(|window| {
                        let id = window["window_id"].as_u64()?;
                        let title = window["title"].as_str().unwrap_or("");
                        title.contains(&expected_title).then(|| TargetWindow {
                            pid: window["pid"].as_u64().unwrap_or(launched_pid as u64) as u32,
                            native_id: id,
                        })
                    })
                })
            {
                assert_ne!(
                    target.pid, 0,
                    "foreground sentinel window has no process id"
                );
                break target;
            }
            assert!(
                Instant::now() < window_deadline,
                "foreground sentinel window did not appear"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        reaper.track_pid(target.pid);

        let focus_deadline = Instant::now() + Duration::from_secs(10);
        if is_wayland_session() {
            wait_for_journal(&journal_path, focus_deadline, r#""kind":"ready""#, "ready");
            activate_native_foreground(driver, target);
            // Electron may already be focused before its preload listener is ready.
            // The compositor observation is the authoritative Wayland focus gate.
            wait_for_native_focus_stable(target);
        } else {
            wait_for_journal(&journal_path, focus_deadline, r#""kind":"ready""#, "ready");
            activate_native_foreground(driver, target);
            wait_for_native_focus_stable(target);
            wait_for_journal(&journal_path, focus_deadline, r#""kind":"focus""#, "focus");
        }
        fs::write(&journal_path, "").expect("reset focused sentinel journal");

        Self {
            journal_path,
            target,
            _process_heartbeat: process_heartbeat,
            _reaper: reaper,
            _user_data: user_data,
        }
    }

    pub fn observe(&self) -> (Vec<OracleKind>, Vec<String>) {
        std::thread::sleep(Duration::from_millis(200));
        let events = match read_journal_events(&self.journal_path) {
            Ok(events) => events,
            Err(error) => {
                return (
                    Vec::new(),
                    vec![format!("foreground sentinel journal is invalid: {error}")],
                )
            }
        };
        let mut passed = Vec::new();
        let mut violations = Vec::new();
        if !events
            .iter()
            .any(|event| event_kind(event) == Some("heartbeat"))
        {
            violations.push("foreground sentinel heartbeat stopped".to_owned());
        }
        if !is_wayland_session() {
            if events.iter().any(|event| {
                event_kind(event) == Some("blur")
                    || (event_kind(event) == Some("visibility")
                        && event["state"].as_str() == Some("hidden"))
            }) {
                violations.push("foreground sentinel lost focus".to_owned());
            } else {
                passed.push(OracleKind::Focus);
            }
        }
        let leaked = events
            .iter()
            .filter_map(|event| event_kind(event))
            .filter(|kind| {
                matches!(
                    *kind,
                    "keydown"
                        | "keyup"
                        | "pointerdown"
                        | "pointerup"
                        | "click"
                        | "wheel"
                        | "contextmenu"
                        | "input"
                )
            })
            .collect::<Vec<_>>();
        if leaked.is_empty() {
            passed.push(OracleKind::NoLeakedInput);
        } else {
            violations.push(format!(
                "foreground sentinel received input events: {}",
                leaked.join(", ")
            ));
        }
        (passed, violations)
    }

    /// Prove once per canonical lane that the foreground guard can detect both
    /// leaked input and transient focus loss. A guard that cannot observe its
    /// deliberate violations must never be trusted to certify background rows.
    pub fn assert_guard_canaries(
        &self,
        driver: &mut impl Driver,
        background_target: TargetWindow,
    ) -> Result<(), String> {
        wait_for_event(&self.journal_path, "heartbeat", Duration::from_secs(2))?;
        reset_journal(&self.journal_path)?;

        let canary_key = if std::env::var("CUA_E2E_WAYLAND_SESSION")
            .is_ok_and(|session| session == "cua-compositor")
        {
            // The intentionally small injection protocol exposes navigation
            // and control keys, not printable letters. Space still exercises
            // the renderer keydown leak detector without broadening it.
            "space"
        } else {
            "a"
        };
        let leaked_key = driver.call(
            "press_key",
            serde_json::json!({
                "pid": self.target.pid,
                "window_id": self.target.native_id,
                "key": canary_key,
                "delivery_mode": "foreground",
            }),
        );
        if leaked_key.is_error() {
            return Err(format!(
                "foreground input canary could not inject a key: {}",
                leaked_key.text()
            ));
        }
        wait_for_event(&self.journal_path, "keydown", Duration::from_secs(2))?;
        let (_, leaked_input_violations) = self.observe();
        if !leaked_input_violations
            .iter()
            .any(|violation| violation.contains("received input events"))
        {
            return Err(format!(
                "foreground input canary was not detected: {leaked_input_violations:?}"
            ));
        }
        reset_journal(&self.journal_path)?;

        #[cfg(target_os = "linux")]
        set_sway_fullscreen(driver, self.target, false)?;
        let raised = driver.call(
            "bring_to_front",
            serde_json::json!({
                "pid": background_target.pid,
                "window_id": background_target.native_id,
            }),
        );
        if raised.is_error() {
            return Err(format!(
                "focus-loss canary could not raise the background target: {}",
                raised.text()
            ));
        }
        #[cfg(target_os = "linux")]
        focus_sway_target(driver, background_target)?;
        if is_wayland_session() {
            wait_for_native_focus_lost(self.target)?;
        } else {
            wait_for_event(&self.journal_path, "blur", Duration::from_secs(3))?;
            let (_, focus_violations) = self.observe();
            if !focus_violations
                .iter()
                .any(|violation| violation.contains("lost focus"))
            {
                return Err(format!(
                    "focus-loss canary was not detected: {focus_violations:?}"
                ));
            }
        }

        activate_native_foreground(driver, self.target);
        #[cfg(target_os = "linux")]
        set_sway_fullscreen(driver, self.target, true)?;
        wait_for_native_focus_stable(self.target);
        std::thread::sleep(Duration::from_millis(150));
        reset_journal(&self.journal_path)?;
        wait_for_event(&self.journal_path, "heartbeat", Duration::from_secs(2))?;
        reset_journal(&self.journal_path)?;
        self.assert_background_posture(background_target)
    }

    pub fn target(&self) -> TargetWindow {
        self.target
    }

    /// Confirm the target is fully behind the ready foreground sentinel before
    /// the behavioral video boundary is crossed.
    pub fn assert_background_posture(&self, target: TargetWindow) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let observer = DesktopObserver::new(NativeObserver::new(), target);
            let before = observer.snapshot().map_err(|error| error.to_string())?;
            if before.target_z == crate::observer::TargetZ::BackgroundOccluded {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "background target was not fully occluded before recording: {:?}",
                    before.target_z
                ));
            }
            // Native maximize/fullscreen transitions can report focus before
            // their final geometry. This wait is outside the action boundary;
            // dispatch-time occlusion remains an immediate strict assertion.
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Re-establish the observation boundary after video capture starts.
    /// Capture backends may briefly perturb foreground state while they attach;
    /// those setup events are not part of the action under test.
    pub fn prepare_background_observation(
        &self,
        driver: &mut impl Driver,
        target: TargetWindow,
    ) -> Result<(), String> {
        activate_native_foreground(driver, self.target);
        wait_for_native_focus_stable(self.target);
        std::thread::sleep(Duration::from_millis(100));
        reset_journal(&self.journal_path)?;
        // Windows establishes focus with a physical click. Its DOM `click`
        // can arrive after the native focus transition and the first journal
        // reset, falsely attributing setup input to the background action.
        // A later heartbeat is an event-loop barrier: once observed, clear the
        // journal again so the action boundary starts from a quiet sentinel.
        wait_for_event(&self.journal_path, "heartbeat", Duration::from_secs(2))?;
        reset_journal(&self.journal_path)?;
        self.assert_background_posture(target)
    }

    /// Run one background action while checking the native desktop and the
    /// sentinel journal. The returned oracle list is suitable for a typed E2E
    /// result; any unsupported observation or side effect is an error.
    pub fn observe_background<R>(
        &self,
        target: TargetWindow,
        action: impl FnOnce() -> R,
    ) -> Result<(R, Vec<OracleKind>), String> {
        self.observe_target(target, true, action)
    }

    /// Observe a desktop-wide action against the foreground sentinel itself.
    /// Launch and cursor-overlay cells have no pre-existing background target,
    /// so they verify desktop stability without the target-occlusion precondition.
    pub fn observe_desktop<R>(
        &self,
        action: impl FnOnce() -> R,
    ) -> Result<(R, Vec<OracleKind>), String> {
        self.observe_target(self.target, false, action)
    }

    fn observe_target<R>(
        &self,
        target: TargetWindow,
        require_occluded_target: bool,
        action: impl FnOnce() -> R,
    ) -> Result<(R, Vec<OracleKind>), String> {
        let mut observer = DesktopObserver::new(NativeObserver::new(), target);
        let before = observer.snapshot().map_err(|error| error.to_string())?;
        if require_occluded_target
            && before.target_z != crate::observer::TargetZ::BackgroundOccluded
        {
            return Err(format!(
                "background target was not fully occluded before dispatch: {:?}",
                before.target_z
            ));
        }
        let mut native_oracles = vec![OracleKind::Focus, OracleKind::ZOrder];
        if std::env::var("XDG_SESSION_TYPE")
            .map(|session| !session.eq_ignore_ascii_case("wayland"))
            .unwrap_or(true)
        {
            native_oracles.push(OracleKind::Cursor);
        }
        let (result, delta) = observer
            .observe(&native_oracles, action)
            .map_err(|error| error.to_string())?;
        if require_occluded_target
            && delta.before.target_z != crate::observer::TargetZ::BackgroundOccluded
        {
            return Err(format!(
                "background target was not fully occluded at dispatch: {:?}",
                delta.before.target_z
            ));
        }
        delta
            .ensure_supported()
            .map_err(|error| error.to_string())?;

        let mut passed = delta.passed().to_vec();
        let mut violations = delta.violations().to_vec();
        let (sentinel_passed, sentinel_violations) = self.observe();
        passed.extend(sentinel_passed);
        violations.extend(sentinel_violations);
        passed.sort();
        passed.dedup();
        if violations.is_empty() {
            Ok((result, passed))
        } else {
            Err(violations.join("; "))
        }
    }
}

fn wait_for_journal(path: &std::path::Path, deadline: Instant, marker: &str, state: &str) {
    loop {
        let journal = fs::read_to_string(path).unwrap_or_default();
        if journal.contains(marker) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "foreground sentinel did not become {state}: {journal}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn reset_journal(path: &std::path::Path) -> Result<(), String> {
    fs::write(path, "").map_err(|error| format!("reset foreground sentinel journal: {error}"))
}

fn read_journal_events(path: &std::path::Path) -> Result<Vec<serde_json::Value>, String> {
    let journal =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    journal
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|error| format!("parse sentinel event {line:?}: {error}"))
        })
        .collect()
}

fn event_kind(event: &serde_json::Value) -> Option<&str> {
    event["kind"].as_str()
}

fn wait_for_event(path: &std::path::Path, kind: &str, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let events = read_journal_events(path)?;
        if events.iter().any(|event| event_kind(event) == Some(kind)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "foreground sentinel did not emit {kind:?} within {timeout:?}: {events:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn is_wayland_session() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
}

fn activate_native_foreground(driver: &mut impl Driver, target: TargetWindow) {
    let response = driver.call(
        "bring_to_front",
        serde_json::json!({
            "pid": target.pid,
            "window_id": target.native_id,
        }),
    );
    assert!(
        !response.is_error(),
        "could not activate foreground sentinel: {}",
        response.text()
    );
    #[cfg(target_os = "linux")]
    focus_sway_target(driver, target)
        .expect("could not focus foreground sentinel through Sway IPC");
    #[cfg(target_os = "windows")]
    physically_focus_windows_sentinel(target);
}

#[cfg(target_os = "linux")]
fn focus_sway_target(driver: &mut impl Driver, target: TargetWindow) -> Result<(), String> {
    let is_sway = is_wayland_session()
        && std::env::var("CUA_E2E_WAYLAND_SESSION").is_ok_and(|session| session == "sway");
    if !is_sway {
        return Ok(());
    }

    let (_, con_id) = sway_tree_and_container_for_target(driver, target)?;
    run_sway_container_command(con_id, &["focus"], "focus canary")
}

#[cfg(target_os = "linux")]
fn set_sway_fullscreen(
    driver: &mut impl Driver,
    target: TargetWindow,
    enabled: bool,
) -> Result<(), String> {
    let is_sway = is_wayland_session()
        && std::env::var("CUA_E2E_WAYLAND_SESSION").is_ok_and(|session| session == "sway");
    if !is_sway {
        return Ok(());
    }

    let (tree, view_id) = sway_tree_and_container_for_target(driver, target)?;
    let con_id = if enabled {
        view_id
    } else {
        sway_fullscreen_holder_id(&tree, view_id).unwrap_or(view_id)
    };
    let state = if enabled { "enable" } else { "disable" };
    run_sway_container_command(con_id, &["fullscreen", state], "fullscreen canary")?;
    if !enabled {
        wait_for_sway_fullscreen_cleared(view_id)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn sway_tree_and_container_for_target(
    driver: &mut impl Driver,
    target: TargetWindow,
) -> Result<(serde_json::Value, i64), String> {
    let tree = read_sway_tree()?;

    let windows = driver.call("list_windows", serde_json::json!({}));
    if windows.is_error() {
        return Err(format!(
            "list windows before Sway focus canary: {}",
            windows.text()
        ));
    }
    let title = sway_target_title(windows.structured(), target);
    let con_id = title
        .and_then(|title| sway_container_id(&tree, title))
        .or_else(|| sway_container_id_by_unique_pid(&tree, target.pid))
        .ok_or_else(|| {
            format!(
                "Sway focus canary could not resolve container for pid {} window {}",
                target.pid, target.native_id
            )
        })?;
    Ok((tree, con_id))
}

#[cfg(target_os = "linux")]
fn read_sway_tree() -> Result<serde_json::Value, String> {
    let tree_output = Command::new("swaymsg")
        .args(["-r", "-t", "get_tree"])
        .output()
        .map_err(|error| format!("read Sway tree for focus canary: {error}"))?;
    if !tree_output.status.success() {
        return Err(format!(
            "read Sway tree for focus canary: {}",
            String::from_utf8_lossy(&tree_output.stderr).trim()
        ));
    }
    serde_json::from_slice(&tree_output.stdout)
        .map_err(|error| format!("parse Sway tree for focus canary: {error}"))
}

#[cfg(target_os = "linux")]
fn sway_path_to_container<'a>(
    node: &'a serde_json::Value,
    con_id: i64,
    path: &mut Vec<&'a serde_json::Value>,
) -> bool {
    path.push(node);
    if node["id"].as_i64() == Some(con_id) {
        return true;
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node[key].as_array() {
            for child in children {
                if sway_path_to_container(child, con_id, path) {
                    return true;
                }
            }
        }
    }
    path.pop();
    false
}

#[cfg(target_os = "linux")]
fn sway_fullscreen_holder_id(tree: &serde_json::Value, view_id: i64) -> Option<i64> {
    let mut path = Vec::new();
    sway_path_to_container(tree, view_id, &mut path).then_some(())?;
    path.into_iter()
        .rev()
        .find(|node| node["fullscreen_mode"].as_i64().unwrap_or(0) != 0)
        .and_then(|node| node["id"].as_i64())
}

#[cfg(target_os = "linux")]
fn sway_container_fullscreen_mode(tree: &serde_json::Value, con_id: i64) -> Option<i64> {
    if tree["id"].as_i64() == Some(con_id) {
        return Some(tree["fullscreen_mode"].as_i64().unwrap_or(0));
    }
    ["nodes", "floating_nodes"].into_iter().find_map(|key| {
        tree[key].as_array().and_then(|children| {
            children
                .iter()
                .find_map(|child| sway_container_fullscreen_mode(child, con_id))
        })
    })
}

#[cfg(target_os = "linux")]
fn wait_for_sway_fullscreen_cleared(view_id: i64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let tree = read_sway_tree()?;
        let fullscreen_mode = sway_container_fullscreen_mode(&tree, view_id)
            .ok_or_else(|| format!("Sway fullscreen canary lost view {view_id}"))?;
        if fullscreen_mode == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Sway fullscreen canary did not clear fullscreen mode {fullscreen_mode} for view {view_id}"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
fn run_sway_container_command(con_id: i64, command: &[&str], purpose: &str) -> Result<(), String> {
    let criterion = format!("[con_id={con_id}]");
    let output = Command::new("swaymsg")
        .arg("-r")
        .arg(criterion.as_str())
        .args(command)
        .output()
        .map_err(|error| format!("run Sway {purpose} for con_id {con_id}: {error}"))?;
    let result: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse Sway {purpose} result for con_id {con_id}: {error}"))?;
    let succeeded = output.status.success()
        && result.as_array().is_some_and(|results| {
            !results.is_empty() && results.iter().all(|item| item["success"] == true)
        });
    if !succeeded {
        Err(format!(
            "Sway {purpose} for con_id {con_id} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
    } else {
        if std::env::var_os("CUA_E2E_DEBUG_SWAY").is_some() {
            let tree = read_sway_tree()?;
            let mut rows = Vec::new();
            collect_sway_debug_rows(&tree, &mut rows);
            eprintln!(
                "[testkit] Sway {purpose}: con_id={con_id} command={command:?} tree={rows:?}"
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn collect_sway_debug_rows(node: &serde_json::Value, rows: &mut Vec<String>) {
    let focused = node["focused"].as_bool().unwrap_or(false);
    let fullscreen = node["fullscreen_mode"].as_i64().unwrap_or(0);
    if focused || fullscreen != 0 || node["pid"].as_u64().is_some() {
        rows.push(format!(
            "id={} pid={} name={:?} focused={focused} fullscreen={fullscreen}",
            node["id"].as_i64().unwrap_or_default(),
            node["pid"].as_u64().unwrap_or_default(),
            node["name"].as_str().unwrap_or("")
        ));
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node[key].as_array() {
            for child in children {
                collect_sway_debug_rows(child, rows);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn sway_target_title(windows: &serde_json::Value, target: TargetWindow) -> Option<&str> {
    let windows = windows["windows"].as_array()?;
    windows
        .iter()
        .find(|window| {
            window["pid"].as_u64() == Some(target.pid as u64)
                && window["window_id"].as_u64() == Some(target.native_id)
        })
        .and_then(|window| window["title"].as_str())
        .or_else(|| {
            let mut titles = windows
                .iter()
                .filter(|window| window["pid"].as_u64() == Some(target.pid as u64))
                .filter_map(|window| window["title"].as_str())
                .filter(|title| !title.is_empty());
            let title = titles.next()?;
            titles.next().is_none().then_some(title)
        })
}

#[cfg(target_os = "linux")]
fn collect_sway_container_ids_by_pid(node: &serde_json::Value, pid: u32, ids: &mut Vec<i64>) {
    if node["pid"].as_u64() == Some(pid as u64) {
        if let Some(id) = node["id"].as_i64() {
            ids.push(id);
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node[key].as_array() {
            for child in children {
                collect_sway_container_ids_by_pid(child, pid, ids);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn sway_container_id_by_unique_pid(node: &serde_json::Value, pid: u32) -> Option<i64> {
    let mut ids = Vec::new();
    collect_sway_container_ids_by_pid(node, pid, &mut ids);
    (ids.len() == 1).then(|| ids[0])
}

#[cfg(target_os = "linux")]
fn sway_container_id(node: &serde_json::Value, title: &str) -> Option<i64> {
    let raw_title = title
        .rsplit_once(" [")
        .map(|(candidate, _)| candidate)
        .unwrap_or(title);
    if node["name"]
        .as_str()
        .is_some_and(|name| name == title || name == raw_title)
    {
        return node["id"].as_i64();
    }
    ["nodes", "floating_nodes"].into_iter().find_map(|key| {
        node[key].as_array().and_then(|children| {
            children
                .iter()
                .find_map(|child| sway_container_id(child, title))
        })
    })
}

#[cfg(all(test, target_os = "linux"))]
mod sway_tests {
    use super::*;

    #[test]
    fn fullscreen_command_targets_the_holder_ancestor() {
        let tree = serde_json::json!({
            "id": 1,
            "nodes": [{
                "id": 30,
                "fullscreen_mode": 1,
                "nodes": [{
                    "id": 31,
                    "pid": 1234,
                    "name": "CuaTestHarness Sentinel",
                    "focused": true,
                    "fullscreen_mode": 0
                }]
            }]
        });
        assert_eq!(sway_fullscreen_holder_id(&tree, 31), Some(30));
    }

    #[test]
    fn fullscreen_command_prefers_the_actionable_view() {
        let tree = serde_json::json!({
            "id": 1,
            "fullscreen_mode": 1,
            "nodes": [{
                "id": 30,
                "fullscreen_mode": 1,
                "nodes": [{
                    "id": 31,
                    "pid": 1234,
                    "name": "CuaTestHarness Sentinel",
                    "focused": true,
                    "fullscreen_mode": 1
                }]
            }]
        });
        assert_eq!(sway_fullscreen_holder_id(&tree, 31), Some(31));
    }

    #[test]
    fn fullscreen_clear_checks_the_view_not_inherited_workspace_state() {
        let tree = serde_json::json!({
            "id": 1,
            "fullscreen_mode": 1,
            "nodes": [{
                "id": 30,
                "fullscreen_mode": 1,
                "nodes": [{
                    "id": 31,
                    "pid": 1234,
                    "name": "CuaTestHarness Sentinel",
                    "focused": true,
                    "fullscreen_mode": 0
                }]
            }]
        });
        assert_eq!(sway_container_fullscreen_mode(&tree, 31), Some(0));
    }

    #[test]
    fn focus_target_uses_unique_sway_pid_when_title_is_unavailable() {
        let tree = serde_json::json!({
            "id": 1,
            "nodes": [{
                "id": 9,
                "nodes": [{
                    "id": 42,
                    "pid": 1234,
                    "name": "CuaTestHarness Electron"
                }]
            }]
        });
        assert_eq!(sway_container_id_by_unique_pid(&tree, 1234), Some(42));
    }

    #[test]
    fn focus_target_does_not_guess_when_pid_owns_multiple_windows() {
        let tree = serde_json::json!({
            "id": 1,
            "nodes": [
                {"id": 41, "pid": 1234, "name": "First"},
                {"id": 42, "pid": 1234, "name": "Second"}
            ]
        });
        assert_eq!(sway_container_id_by_unique_pid(&tree, 1234), None);
        assert_eq!(sway_container_id(&tree, "Second"), Some(42));
    }

    #[test]
    fn focus_target_uses_unique_same_process_title_when_window_id_changes() {
        let windows = serde_json::json!({
            "windows": [{
                "pid": 1234,
                "window_id": 88,
                "title": "CuaTestHarness Electron [cdp=9223]"
            }]
        });
        let target = TargetWindow {
            pid: 1234,
            native_id: 99,
        };
        assert_eq!(
            sway_target_title(&windows, target),
            Some("CuaTestHarness Electron [cdp=9223]")
        );
    }
}

#[cfg(target_os = "windows")]
fn physically_focus_windows_sentinel(target: TargetWindow) {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, SetCursorPos};

    let hwnd = HWND(target.native_id as *mut _);
    let mut rect = RECT::default();
    let mut original_cursor = POINT::default();
    unsafe {
        GetWindowRect(hwnd, &mut rect).expect("read foreground sentinel bounds");
        GetCursorPos(&mut original_cursor).expect("read cursor before focusing sentinel");
    }
    let x = (rect.left + rect.right) / 2;
    let y = (rect.top + rect.bottom) / 2;
    assert!(
        rect.right > rect.left && rect.bottom > rect.top,
        "foreground sentinel has invalid bounds: {rect:?}"
    );

    let inputs = [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    ..Default::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: MOUSEEVENTF_LEFTUP,
                    ..Default::default()
                },
            },
        },
    ];
    unsafe {
        SetCursorPos(x, y).expect("move cursor onto foreground sentinel");
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        assert_eq!(sent, inputs.len() as u32, "click foreground sentinel");
        SetCursorPos(original_cursor.x, original_cursor.y)
            .expect("restore cursor after focusing sentinel");
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn wait_for_native_focus_stable(target: TargetWindow) {
    use crate::observer::{ObserverBackend, TargetZ};

    let backend = NativeObserver::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stable_since = None;
    loop {
        let foreground = backend
            .snapshot(target)
            .map(|snapshot| snapshot.target_z == TargetZ::Foreground)
            .unwrap_or(false);
        if foreground {
            let since = stable_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(300) {
                return;
            }
        } else {
            stable_since = None;
        }
        assert!(
            Instant::now() < deadline,
            "foreground sentinel did not remain natively focused"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "linux")]
fn wait_for_native_focus_lost(target: TargetWindow) -> Result<(), String> {
    use crate::observer::{ObserverBackend, TargetZ};

    let backend = NativeObserver::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = backend.snapshot(target).ok();
        let lost = snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.target_z != TargetZ::Foreground);
        if lost {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "native Wayland focus-loss canary was not detected; last snapshot: {snapshot:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(not(target_os = "linux"))]
fn wait_for_native_focus_lost(_target: TargetWindow) -> Result<(), String> {
    Err("native Wayland focus observation is only available on Linux".to_owned())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn wait_for_native_focus_stable(_target: TargetWindow) {}

pub fn run_with_background_oracles<D: Driver + BehaviorRecording, R>(
    driver: &mut D,
    target: TargetWindow,
    action: impl FnOnce(&mut D) -> R,
) -> Result<(R, Vec<OracleKind>), String> {
    let sentinel = ForegroundSentinel::launch(driver);
    sentinel.assert_background_posture(target)?;
    driver.start_behavior_recording();
    sentinel.prepare_background_observation(driver, target)?;
    sentinel.observe_background(target, || action(driver))
}

struct ElectronFixture {
    path: std::path::PathBuf,
    args: Vec<&'static str>,
}

fn electron_fixture() -> ElectronFixture {
    #[cfg(target_os = "windows")]
    {
        ElectronFixture {
            path: harness_app("harness-electron", "CuaTestHarness.Electron.exe"),
            args: vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
        }
    }
    #[cfg(target_os = "macos")]
    {
        ElectronFixture {
            path: harness_app(
                "harness-electron",
                "CuaTestHarness.Electron.app/Contents/MacOS/Electron",
            ),
            args: vec!["--force-renderer-accessibility"],
        }
    }
    #[cfg(target_os = "linux")]
    {
        ElectronFixture {
            path: harness_app("harness-electron", "CuaTestHarness.Electron"),
            args: vec![
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
        }
    }
}
