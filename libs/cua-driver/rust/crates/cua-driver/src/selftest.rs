// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver test` - prove a fresh install works, end to end.
//!
//! Each step runs through the running Cua Driver the same way `cua-driver
//! call` does, using existing tools only. The steps talk to a [`Backend`], so
//! the runner and the output can be tested with a fake.
//!
//! Exit codes: 0 everything works, 1 something needs fixing, 64 usage error.

use serde_json::{json, Value};

const PHRASE: &str = "Hello from Cua Driver";
const EXAMPLE_PROMPT: &str = "Use Cua Driver to open Calculator and work out 17 times 23.";

/// What the steps need from the machine. The real one talks to the daemon.
pub trait Backend {
    /// True when this machine can run the Mac text-editor check.
    fn runs_text_editor_check(&self) -> bool;
    /// True when the app (or a running daemon) is there to talk to.
    fn app_installed(&self) -> bool;
    fn daemon_running(&self) -> bool;
    fn start_daemon(&mut self) -> Result<(), String>;
    /// `structuredContent` of the permission check. Never prompts.
    fn permissions(&mut self) -> Result<Value, String>;
    /// Run one tool. Returns the whole result (`content`, `structuredContent`).
    /// `Err` when the tool refuses or fails.
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, String>;
    fn sleep_ms(&mut self, ms: u64);
    /// Label shared by every call of this run.
    fn session(&self) -> String;
    /// An empty text file to type into, so the user's own documents stay untouched.
    fn make_scratch_file(&mut self) -> Result<String, String>;
    fn remove_scratch_file(&mut self, path: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub id: &'static str,
    pub outcome: Outcome,
    pub message: String,
    pub fix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub steps: Vec<Step>,
    /// A line explaining checks that do not run on this system.
    pub note: Option<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.steps.iter().all(|s| s.outcome != Outcome::Fail)
    }

    pub fn exit_code(&self) -> i32 {
        if self.ok() {
            0
        } else {
            1
        }
    }

    pub fn fixes(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|s| s.outcome == Outcome::Fail)
            .filter_map(|s| s.fix.as_deref())
            .collect()
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for step in &self.steps {
            let mark = match step.outcome {
                Outcome::Pass => "\u{2713}",
                Outcome::Fail => "\u{2717}",
            };
            out.push_str(&format!("{mark} {}\n", step.message));
        }
        if let Some(note) = &self.note {
            out.push_str(&format!("{note}\n"));
        }
        if self.ok() {
            out.push_str(&format!(
                "Cua Driver works. Ask your agent: \"{EXAMPLE_PROMPT}\"\n"
            ));
        } else {
            out.push_str("\nWhat to fix:\n");
            for (i, fix) in self.fixes().iter().enumerate() {
                out.push_str(&format!("{}. {fix}\n", i + 1));
            }
        }
        out
    }

    pub fn to_json(&self) -> Value {
        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "status": match s.outcome {
                        Outcome::Pass => "pass",
                        Outcome::Fail => "fail",
                    },
                    "message": s.message,
                    "fix": s.fix,
                })
            })
            .collect();
        json!({
            "ok": self.ok(),
            "steps": steps,
            "fixes": self.fixes(),
            "note": self.note,
            "example_prompt": if self.ok() { json!(EXAMPLE_PROMPT) } else { Value::Null },
        })
    }
}

fn pass(id: &'static str, message: &str) -> Step {
    Step {
        id,
        outcome: Outcome::Pass,
        message: message.to_owned(),
        fix: None,
    }
}

fn fail(id: &'static str, message: &str, fix: &str) -> Step {
    Step {
        id,
        outcome: Outcome::Fail,
        message: message.to_owned(),
        fix: Some(fix.to_owned()),
    }
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn structured(result: &Value) -> &Value {
    result.get("structuredContent").unwrap_or(result)
}

/// Run every step and stop at the first one that fails.
pub fn run(backend: &mut dyn Backend) -> Report {
    let mut steps: Vec<Step> = Vec::new();
    let mut note = None;

    let grant = "run `cua-driver permissions grant`";

    // 1. Make sure Cua Driver is up, then ask what it may do.
    if !backend.daemon_running() {
        if !backend.app_installed() {
            steps.push(fail(
                "permissions",
                "Cua Driver is not installed",
                "Install Cua Driver: /bin/bash -c \"$(curl -fsSL https://cua.ai/driver/install.sh)\"",
            ));
            return Report { steps, note };
        }
        if let Err(error) = backend.start_daemon() {
            steps.push(fail(
                "permissions",
                &format!("Cua Driver would not start ({error})"),
                &format!("Start Cua Driver and turn on its access: {grant}"),
            ));
            return Report { steps, note };
        }
    }
    let perms = backend.permissions();
    let (ax, sr) = match &perms {
        Ok(p) => (flag(p, "accessibility"), flag(p, "screen_recording")),
        Err(_) => (false, false),
    };
    steps.push(if ax {
        pass(
            "accessibility",
            "Cua Driver can use your mouse and keyboard",
        )
    } else {
        fail(
            "accessibility",
            "Cua Driver can't use your mouse and keyboard yet",
            &format!("Turn on Accessibility for Cua Driver: {grant}"),
        )
    });
    steps.push(if sr {
        pass("screen_recording", "Cua Driver can see your screen")
    } else {
        fail(
            "screen_recording",
            "Cua Driver can't see your screen yet",
            &format!("Turn on Screen Recording for Cua Driver: {grant}"),
        )
    });
    if !(ax && sr) {
        return Report { steps, note };
    }

    // 2. Read the screen.
    let screen = backend.call(
        "get_desktop_state",
        json!({ "max_image_dimension": 640, "session": backend.session() }),
    );
    match screen {
        Ok(result) if has_image(&result) => {
            steps.push(pass("screen_read", "Cua Driver read your screen"))
        }
        Ok(_) => {
            steps.push(fail(
                "screen_read",
                "Cua Driver got a blank picture of your screen",
                &format!("Check Screen Recording is on for Cua Driver, then run `cua-driver test` again: {grant}"),
            ));
            return Report { steps, note };
        }
        Err(error) => {
            steps.push(fail(
                "screen_read",
                &format!("Cua Driver couldn't read your screen ({})", short(&error)),
                &format!("Check Screen Recording is on for Cua Driver: {grant}"),
            ));
            return Report { steps, note };
        }
    }

    // 3. Move Cua Driver's own pointer (never the real mouse).
    match backend.call(
        "move_cursor",
        json!({ "x": 320, "y": 240, "session": backend.session() }),
    ) {
        Ok(_) => steps.push(pass("pointer", "Cua Driver moved its own pointer")),
        Err(error) => {
            steps.push(fail(
                "pointer",
                &format!("Cua Driver couldn't move its pointer ({})", short(&error)),
                "Run `cua-driver doctor` and send us what it prints.",
            ));
            return Report { steps, note };
        }
    }

    // 4. Type into a document and read it back.
    if backend.runs_text_editor_check() {
        let step = text_editor_check(backend);
        let failed = step.outcome == Outcome::Fail;
        steps.push(step);
        if failed {
            let _ = backend.call("end_session", json!({ "session": backend.session() }));
            return Report { steps, note };
        }
    } else {
        note = Some("Typing in a document is only checked on a Mac for now.".to_owned());
    }
    let _ = backend.call("end_session", json!({ "session": backend.session() }));
    Report { steps, note }
}

fn short(error: &str) -> String {
    let first = error.lines().next().unwrap_or(error).trim();
    if first.chars().count() > 120 {
        let cut: String = first.chars().take(117).collect();
        format!("{cut}...")
    } else {
        first.to_owned()
    }
}

fn has_image(result: &Value) -> bool {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("image")
                    && item
                        .get("data")
                        .and_then(Value::as_str)
                        .is_some_and(|d| d.len() > 200)
            })
        })
        .unwrap_or(false)
}

fn text_editor_check(backend: &mut dyn Backend) -> Step {
    let id = "text_editor";
    let doctor = "Run `cua-driver doctor` and send us what it prints.";
    // A fresh TextEdit instance on a scratch file, so the user's own TextEdit
    // windows and documents are never touched.
    let scratch = match backend.make_scratch_file() {
        Ok(path) => path,
        Err(error) => {
            return fail(
                id,
                &format!(
                    "Cua Driver couldn't prepare a test file ({})",
                    short(&error)
                ),
                doctor,
            )
        }
    };
    let step = text_editor_run(backend, &scratch);
    backend.remove_scratch_file(&scratch);
    step
}

fn text_editor_run(backend: &mut dyn Backend, scratch: &str) -> Step {
    let id = "text_editor";
    let doctor = "Run `cua-driver doctor` and send us what it prints.";
    let launched = match backend.call(
        "launch_app",
        json!({ "bundle_id": "com.apple.TextEdit", "creates_new_application_instance": true,
                "urls": [scratch], "session": backend.session() }),
    ) {
        Ok(v) => v,
        Err(error) => {
            return fail(
                id,
                &format!("Cua Driver couldn't open TextEdit ({})", short(&error)),
                doctor,
            )
        }
    };
    let pid = structured(&launched).get("pid").and_then(Value::as_i64);
    let Some(pid) = pid else {
        return fail(id, "Cua Driver couldn't open TextEdit", doctor);
    };

    let result = type_and_read_back(backend, pid);
    // Always tidy up, whatever happened above.
    let closed = close_without_saving(backend, pid);
    // Quit the TextEdit this test started (best effort; only works for our own launch).
    let _ = backend.call(
        "kill_app",
        json!({ "pid": pid, "session": backend.session() }),
    );
    match (result, closed) {
        (Ok(()), Ok(())) => pass(id, "Cua Driver typed in TextEdit and read it back"),
        (Ok(()), Err(_)) => fail(
            id,
            "Cua Driver typed in TextEdit, but couldn't close the test document",
            "Close the empty TextEdit window that is left over and choose Delete.",
        ),
        (Err(error), _) => fail(
            id,
            &format!("Cua Driver couldn't type in TextEdit ({})", short(&error)),
            doctor,
        ),
    }
}

fn visible_window(backend: &mut dyn Backend, pid: i64) -> Result<Option<i64>, String> {
    let listed = backend.call(
        "list_windows",
        json!({ "pid": pid, "session": backend.session() }),
    )?;
    let windows = structured(&listed)
        .get("windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(windows
        .iter()
        .filter(|w| w.get("layer").and_then(Value::as_i64).unwrap_or(0) == 0)
        .filter(|w| {
            w.get("is_on_screen")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        })
        .find_map(|w| w.get("window_id").and_then(Value::as_i64)))
}

fn find_window(backend: &mut dyn Backend, pid: i64) -> Result<i64, String> {
    for _ in 0..20 {
        if let Some(id) = visible_window(backend, pid)? {
            return Ok(id);
        }
        backend.sleep_ms(400);
    }
    Err("TextEdit showed no window".to_owned())
}

fn read_elements(
    backend: &mut dyn Backend,
    pid: i64,
    window_id: i64,
) -> Result<Vec<Value>, String> {
    let read = backend.call(
        "get_window_state",
        json!({ "pid": pid, "window_id": window_id, "include_screenshot": false,
                "full_output": true, "timeout_ms": 5000, "session": backend.session() }),
    )?;
    Ok(structured(&read)
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn token(element: &Value) -> Option<String> {
    element
        .get("element_token")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn type_and_read_back(backend: &mut dyn Backend, pid: i64) -> Result<(), String> {
    let window_id = find_window(backend, pid)?;
    let elements = read_elements(backend, pid, window_id)?;
    let area = elements
        .iter()
        .find(|e| e.get("role").and_then(Value::as_str) == Some("AXTextArea"))
        .and_then(token)
        .ok_or("no text area in the TextEdit window")?;
    backend.call(
        "type_text",
        json!({ "pid": pid, "window_id": window_id, "element_token": area, "text": PHRASE, "session": backend.session() }),
    )?;
    let read = backend.call(
        "get_window_state",
        json!({ "pid": pid, "window_id": window_id, "include_screenshot": false,
                "query": "Hello", "timeout_ms": 5000, "session": backend.session() }),
    )?;
    let tree = structured(&read)
        .get("tree_markdown")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tree.contains(PHRASE) {
        Ok(())
    } else {
        Err("the typed text was not in the window".to_owned())
    }
}

/// Press the window's close button, then choose Delete on the save sheet.
fn close_without_saving(backend: &mut dyn Backend, pid: i64) -> Result<(), String> {
    let window_id = match visible_window(backend, pid)? {
        Some(id) => id,
        None => return Ok(()), // nothing left to close
    };
    let elements = read_elements(backend, pid, window_id)?;
    // The close button is the left-most unlabeled button on the window's title bar.
    let close = elements
        .iter()
        .filter(|e| {
            e.get("role").and_then(Value::as_str) == Some("AXButton")
                && e.get("parent_index").and_then(Value::as_i64) == Some(0)
        })
        .min_by(|a, b| {
            let x = |e: &Value| e["frame"]["x"].as_f64().unwrap_or(f64::MAX);
            x(a).total_cmp(&x(b))
        })
        .and_then(token)
        .ok_or("no close button")?;
    backend.call(
        "click",
        json!({ "pid": pid, "window_id": window_id, "element_token": close, "session": backend.session() }),
    )?;
    // A saved file closes at once. If a save sheet shows up, choose Delete.
    for _ in 0..10 {
        backend.sleep_ms(300);
        if visible_window(backend, pid)?.is_none() {
            return Ok(());
        }
        let elements = read_elements(backend, pid, window_id)?;
        let sheet_delete = elements.iter().find(|e| {
            e.get("role").and_then(Value::as_str) == Some("AXButton")
                && e.get("label").and_then(Value::as_str) == Some("Delete")
        });
        if let Some(button) = sheet_delete.and_then(token) {
            backend.call(
                "click",
                json!({ "pid": pid, "window_id": window_id, "element_token": button, "session": backend.session() }),
            )?;
        }
    }
    Err("the window did not close".to_owned())
}

/// The real thing: talks to the installed Cua Driver.
pub struct DaemonBackend {
    socket: String,
}

impl DaemonBackend {
    pub fn new() -> Self {
        Self {
            socket: crate::serve::default_socket_path(),
        }
    }
}

impl Backend for DaemonBackend {
    fn runs_text_editor_check(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn app_installed(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            std::path::Path::new(&crate::bundle::app_bundle_path()).exists()
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }

    fn daemon_running(&self) -> bool {
        crate::serve::is_daemon_listening(&self.socket)
    }

    fn start_daemon(&mut self) -> Result<(), String> {
        // Same launch `permissions grant` and `mcp` use: on a Mac this opens the
        // installed app through LaunchServices, so access is tied to Cua Driver.
        crate::cli::launch_daemon_and_wait(
            &self.socket,
            30,
            false,
            &[],
            crate::history_runtime::preview_admitted_preference(),
        )
        .map_err(|e| e.to_string())
    }

    fn permissions(&mut self) -> Result<Value, String> {
        let request = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some("check_permissions".into()),
            args: Some(json!({ "prompt": false, "probe_direct_capture": false })),
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
        };
        let answered = crate::serve::send_request(&self.socket, &request)
            .ok()
            .filter(|r| r.ok)
            .and_then(|r| r.result)
            .and_then(|r| r.get("structuredContent").cloned());
        if let Some(status) = answered {
            return Ok(status);
        }
        #[cfg(target_os = "macos")]
        {
            // The app may still be inside its first-launch permission step.
            crate::cli::request_permissions_via_launchservices(false)
        }
        #[cfg(not(target_os = "macos"))]
        Err("no answer".to_owned())
    }

    fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        let mut args = args;
        cua_driver_core::tool_args::sanitize_reserved_args(&mut args);
        let request = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some(tool.to_owned()),
            args: Some(args),
            session_id: Some("cli-explicit".to_owned()),
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
        };
        let response =
            crate::serve::send_request(&self.socket, &request).map_err(|e| e.to_string())?;
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "the call failed".to_owned()));
        }
        let result = response.result.unwrap_or(Value::Null);
        let refused = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || structured(&result).get("status").and_then(Value::as_str) == Some("refused");
        if refused {
            let reason = structured(&result)
                .get("refusal")
                .and_then(|r| r.get("message"))
                .and_then(Value::as_str)
                .or_else(|| result["content"][0]["text"].as_str())
                .unwrap_or("the tool refused");
            return Err(reason.to_owned());
        }
        Ok(result)
    }

    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn session(&self) -> String {
        format!("cua-driver-test-{}", std::process::id())
    }

    fn make_scratch_file(&mut self) -> Result<String, String> {
        let path = std::env::temp_dir().join(format!("cua-driver-test-{}.txt", std::process::id()));
        std::fs::write(&path, "").map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }

    fn remove_scratch_file(&mut self, path: &str) {
        let _ = std::fs::remove_file(path);
    }
}

/// Entry point for `cua-driver test [--json]`. Returns the exit code.
pub fn run_cmd(json_output: bool) -> i32 {
    let mut backend = DaemonBackend::new();
    let report = run(&mut backend);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
        );
    } else {
        print!("{}", report.to_text());
    }
    report.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        installed: bool,
        running: bool,
        start_ok: bool,
        perms: Value,
        text_editor: bool,
        calls: Vec<String>,
        closed: bool,
        started: bool,
        typed: bool,
        screen_blank: bool,
        scratch_removed: bool,
    }

    impl Fake {
        fn good() -> Self {
            Fake {
                installed: true,
                running: true,
                start_ok: true,
                perms: json!({ "accessibility": true, "screen_recording": true }),
                text_editor: true,
                calls: vec![],
                closed: false,
                started: false,
                typed: false,
                screen_blank: false,
                scratch_removed: false,
            }
        }
    }

    fn element(index: u32, role: &str, label: Option<&str>, parent: u32, x: f64) -> Value {
        json!({ "element_token": format!("s1:{index}"), "role": role, "label": label,
                "parent_index": parent, "frame": { "x": x, "y": 85.0, "w": 16.0, "h": 16.0 } })
    }

    impl Backend for Fake {
        fn runs_text_editor_check(&self) -> bool {
            self.text_editor
        }
        fn app_installed(&self) -> bool {
            self.installed
        }
        fn daemon_running(&self) -> bool {
            self.running
        }
        fn start_daemon(&mut self) -> Result<(), String> {
            self.started = true;
            if self.start_ok {
                self.running = true;
                Ok(())
            } else {
                Err("timed out".into())
            }
        }
        fn permissions(&mut self) -> Result<Value, String> {
            Ok(self.perms.clone())
        }
        fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
            self.calls.push(tool.to_owned());
            match tool {
                "get_desktop_state" => {
                    let data = if self.screen_blank {
                        ""
                    } else {
                        &"x".repeat(500)
                    };
                    Ok(json!({ "content": [{ "type": "image", "data": data }] }))
                }
                "move_cursor" => Ok(json!({ "structuredContent": {} })),
                "launch_app" => Ok(json!({ "structuredContent": { "pid": 42 } })),
                "list_windows" => {
                    let windows = if self.closed {
                        json!([])
                    } else {
                        json!([{ "window_id": 7, "layer": 0 }])
                    };
                    Ok(json!({ "structuredContent": { "windows": windows } }))
                }
                "get_window_state" => {
                    if args.get("query").is_some() {
                        let tree = if self.typed {
                            format!("AXTextArea = \"{PHRASE}\"")
                        } else {
                            String::new()
                        };
                        return Ok(json!({ "structuredContent": { "tree_markdown": tree } }));
                    }
                    let elements = vec![
                        element(1, "AXTextArea", None, 0, 300.0),
                        element(34, "AXButton", None, 0, 221.0),
                        element(36, "AXButton", None, 0, 244.0),
                    ];
                    Ok(json!({ "structuredContent": { "elements": elements } }))
                }
                "type_text" => {
                    self.typed = args["text"] == PHRASE && args["element_token"] == "s1:1";
                    Ok(json!({ "structuredContent": { "effect": "confirmed" } }))
                }
                "click" => {
                    match args["element_token"].as_str() {
                        Some("s1:34") => self.closed = true,
                        _ => panic!("clicked the wrong thing: {args}"),
                    }
                    Ok(json!({ "structuredContent": {} }))
                }
                _ => Ok(json!({})),
            }
        }
        fn sleep_ms(&mut self, _ms: u64) {}
        fn session(&self) -> String {
            "t".into()
        }
        fn make_scratch_file(&mut self) -> Result<String, String> {
            Ok("/tmp/scratch.txt".into())
        }
        fn remove_scratch_file(&mut self, _path: &str) {
            self.scratch_removed = true;
        }
    }

    #[test]
    fn everything_works() {
        let mut fake = Fake::good();
        let report = run(&mut fake);
        assert!(report.ok());
        assert_eq!(report.exit_code(), 0);
        let text = report.to_text();
        assert_eq!(text.lines().count(), 6, "{text}");
        assert!(text.contains("Cua Driver works. Ask your agent:"));
        assert!(!text.contains("What to fix"));
        assert!(fake.closed, "the test document is closed");
        assert!(fake.scratch_removed, "the scratch file is removed");
        assert!(fake.calls.contains(&"kill_app".to_owned()));
        assert_eq!(report.to_json()["ok"], true);
        assert_eq!(report.to_json()["steps"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn missing_accessibility_stops_with_one_fix() {
        let mut fake = Fake::good();
        fake.perms = json!({ "accessibility": false, "screen_recording": true });
        let report = run(&mut fake);
        assert!(!report.ok());
        assert_eq!(report.exit_code(), 1);
        let text = report.to_text();
        assert!(text.contains("\u{2717} Cua Driver can't use your mouse and keyboard"));
        assert!(text.contains("\u{2713} Cua Driver can see your screen"));
        assert!(text.contains(
            "1. Turn on Accessibility for Cua Driver: run `cua-driver permissions grant`"
        ));
        assert!(!text.contains("Cua Driver works"));
        assert!(fake.calls.is_empty(), "no tool runs once access is missing");
    }

    #[test]
    fn missing_screen_recording_stops_with_one_fix() {
        let mut fake = Fake::good();
        fake.perms = json!({ "accessibility": true, "screen_recording": false });
        let report = run(&mut fake);
        assert_eq!(report.exit_code(), 1);
        let text = report.to_text();
        assert!(text.contains(
            "1. Turn on Screen Recording for Cua Driver: run `cua-driver permissions grant`"
        ));
        assert!(!text.contains("2."));
        assert_eq!(report.to_json()["fixes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn both_missing_lists_both_fixes_without_jargon() {
        let mut fake = Fake::good();
        fake.perms = json!({});
        let text = run(&mut fake).to_text();
        assert!(text.contains("1. Turn on Accessibility"));
        assert!(text.contains("2. Turn on Screen Recording"));
        for word in ["TCC", "daemon", " AX "] {
            assert!(!text.contains(word), "{word} in {text}");
        }
    }

    #[test]
    fn no_app_installed() {
        let mut fake = Fake::good();
        fake.running = false;
        fake.installed = false;
        let report = run(&mut fake);
        assert_eq!(report.exit_code(), 1);
        assert!(
            !fake.started,
            "does not try to start an app that is not there"
        );
        let text = report.to_text();
        assert!(text.contains("Cua Driver is not installed"));
        assert!(text.contains("cua.ai/driver/install.sh"));
    }

    #[test]
    fn starts_cua_driver_when_it_is_not_running() {
        let mut fake = Fake::good();
        fake.running = false;
        assert!(run(&mut fake).ok());
        assert!(fake.started);
    }

    #[test]
    fn a_start_that_fails_says_how_to_continue() {
        let mut fake = Fake::good();
        fake.running = false;
        fake.start_ok = false;
        let report = run(&mut fake);
        assert_eq!(report.exit_code(), 1);
        assert!(report.to_text().contains("cua-driver permissions grant"));
    }

    #[test]
    fn a_blank_screen_read_fails() {
        let mut fake = Fake::good();
        fake.screen_blank = true;
        let report = run(&mut fake);
        assert_eq!(report.exit_code(), 1);
        assert!(report.to_text().contains("blank picture"));
        assert!(!fake.calls.contains(&"move_cursor".to_owned()));
    }

    #[test]
    fn text_that_never_shows_up_fails_and_still_cleans_up() {
        struct NoText(Fake);
        impl Backend for NoText {
            fn runs_text_editor_check(&self) -> bool {
                true
            }
            fn app_installed(&self) -> bool {
                true
            }
            fn daemon_running(&self) -> bool {
                true
            }
            fn start_daemon(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn permissions(&mut self) -> Result<Value, String> {
                self.0.permissions()
            }
            fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
                if tool == "type_text" {
                    return Err("refused".into());
                }
                self.0.call(tool, args)
            }
            fn sleep_ms(&mut self, _: u64) {}
            fn session(&self) -> String {
                "t".into()
            }
            fn make_scratch_file(&mut self) -> Result<String, String> {
                self.0.make_scratch_file()
            }
            fn remove_scratch_file(&mut self, p: &str) {
                self.0.remove_scratch_file(p)
            }
        }
        let mut backend = NoText(Fake::good());
        let report = run(&mut backend);
        assert_eq!(report.exit_code(), 1);
        assert!(report.to_text().contains("couldn't type in TextEdit"));
        assert!(
            backend.0.closed,
            "the document is closed even when typing failed"
        );
    }

    #[test]
    fn linux_skips_the_text_editor_and_says_so() {
        let mut fake = Fake::good();
        fake.text_editor = false;
        let report = run(&mut fake);
        assert!(report.ok());
        assert!(report.to_text().contains("only checked on a Mac"));
        assert!(!fake.calls.contains(&"launch_app".to_owned()));
        assert_eq!(report.to_json()["steps"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn json_marks_failures_with_fixes() {
        let mut fake = Fake::good();
        fake.perms = json!({ "accessibility": false, "screen_recording": true });
        let json = run(&mut fake).to_json();
        assert_eq!(json["ok"], false);
        assert_eq!(json["steps"][0]["id"], "accessibility");
        assert_eq!(json["steps"][0]["status"], "fail");
        assert_eq!(json["steps"][1]["status"], "pass");
        assert!(json["fixes"][0].as_str().unwrap().contains("Accessibility"));
    }
}
