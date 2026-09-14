use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub struct KeyboardFixture {
    reaper: Option<crate::ChildReaper>,
    pid: u32,
    events: Receiver<Value>,
    pub window_id: u64,
    _directory: tempfile::TempDir,
}

impl KeyboardFixture {
    pub fn spawn(close_on_key: bool) -> Self {
        Self::spawn_configured(close_on_key, false)
    }

    pub fn spawn_with_companion() -> Self {
        Self::spawn_configured(false, true)
    }

    fn spawn_configured(close_on_key: bool, companion: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut command = fixture_command(directory.path());
        command.env("CUA_KEYBOARD_COMPANION", if companion { "1" } else { "0" });
        command.env(
            "CUA_KEYBOARD_CLOSE_ON_KEY",
            if close_on_key { "1" } else { "0" },
        );
        let mut child = crate::spawn_in_job(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit()),
        )
        .expect("launch native keyboard oracle");
        let output = child.stdout.take().unwrap();
        let pid = child.id();
        let mut reaper = crate::ChildReaper::new();
        reaper.push(child);
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    if sender.send(event).is_err() {
                        break;
                    }
                }
            }
        });
        let mut fixture = Self {
            reaper: Some(reaper),
            pid,
            events,
            window_id: 0,
            _directory: directory,
        };
        let ready = fixture.event("ready");
        fixture.window_id = ready["window"].as_u64().expect("native window identity");
        fixture
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn key_event(&self, kind: &str, key: u64) -> Value {
        self.matching_event(kind, Some(key))
    }

    pub fn event(&self, kind: &str) -> Value {
        self.matching_event(kind, None)
    }

    fn matching_event(&self, kind: &str, key: Option<u64>) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = self.events.recv_timeout(remaining).unwrap_or_else(|error| {
                panic!("native keyboard oracle did not report {kind}: {error}")
            });
            if event["kind"] == kind && key.is_none_or(|key| event["key"] == key) {
                return event;
            }
        }
    }

    pub fn assert_single_release(&self, key: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("native key release");
            assert!(
                !(event["kind"] == "down" && event["key"] == key),
                "duplicate primary key: {event}"
            );
            if event["kind"] == "up" && event["key"] == key {
                break;
            }
        }
        self.assert_events_avoid(|event| event["kind"] == "down" && event["key"] == key);
    }

    pub fn assert_no_key_down(&self) {
        self.assert_events_avoid(|event| event["kind"] == "down");
    }

    pub fn assert_quiet(&self) {
        self.assert_events_avoid(|_| true);
    }

    fn assert_events_avoid(&self, forbidden: impl Fn(&Value) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_millis(150);
        loop {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(event) => assert!(!forbidden(&event), "unexpected native event: {event}"),
                Err(mpsc::RecvTimeoutError::Timeout) => return,
                Err(error) => panic!("native observer disconnected: {error}"),
            }
        }
    }

    pub fn terminate(&mut self) {
        drop(self.reaper.take());
    }
}

#[cfg(target_os = "macos")]
fn fixture_command(directory: &std::path::Path) -> Command {
    let source = directory.join("KeyboardOracle.swift");
    let executable = directory.join("KeyboardOracle");
    std::fs::write(&source, include_str!("keyboard_fixture.swift")).unwrap();
    assert!(Command::new("/usr/bin/swiftc")
        .args(["-framework", "AppKit"])
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap()
        .success());
    Command::new(executable)
}

#[cfg(target_os = "linux")]
fn fixture_command(directory: &std::path::Path) -> Command {
    let source = directory.join("keyboard_oracle.py");
    std::fs::write(&source, include_str!("keyboard_fixture.py")).unwrap();
    let mut command = Command::new("python3");
    command.arg("-u").arg(source);
    command
}

#[cfg(target_os = "windows")]
fn fixture_command(directory: &std::path::Path) -> Command {
    let source = directory.join("keyboard_oracle.ps1");
    std::fs::write(&source, include_str!("keyboard_fixture.ps1")).unwrap();
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(source);
    command
}
