//! External behavioral contract for independently destroyable Wayland seats.
//!
//! The control socket and fixture journals are separate oracles. The test does
//! not accept a driver response as evidence that an event reached a fixture.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::{harness_app, FixtureJournal};

const HELLO: &str = "cua-inject v1";

struct Fixture {
    child: Child,
    pid: u32,
    journal: FixtureJournal,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn line(reader: &mut impl BufRead) -> String {
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .expect("read compositor response");
    assert!(!response.is_empty(), "compositor closed the control socket");
    response.trim().to_owned()
}

fn exchange(commands: &[String]) -> Vec<String> {
    let socket = std::env::var("CUA_INJECT_SOCKET")
        .expect("transient-seat E2E requires the cua-compositor socket");
    let stream = UnixStream::connect(socket).expect("connect cua-compositor socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set compositor read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone compositor socket"));
    let mut writer = stream;
    writeln!(writer, "{HELLO}").expect("write protocol hello");
    writer.flush().expect("flush protocol hello");
    assert_eq!(line(&mut reader), HELLO, "compositor protocol handshake");
    commands
        .iter()
        .map(|command| {
            writeln!(writer, "{command}").expect("write compositor command");
            writer.flush().expect("flush compositor command");
            line(&mut reader)
        })
        .collect()
}

fn launch_fixture(label: &str) -> Fixture {
    let executable = harness_app("harness-electron", "CuaTestHarness.Electron");
    assert!(
        executable.exists(),
        "required Electron fixture is missing: {executable:?}"
    );
    let journal = FixtureJournal::start();
    let child = Command::new(executable)
        .args([
            "--no-sandbox",
            "--disable-gpu",
            "--force-renderer-accessibility",
        ])
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .env("CUA_E2E_TRANSIENT_SEAT_LABEL", label)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch Electron fixture");
    Fixture {
        pid: child.id(),
        child,
        journal,
    }
}

fn target(fixture: &Fixture) -> String {
    format!("root:{}", fixture.pid)
}

fn wait_for_target(fixture: &Fixture) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let query = format!("g {}", fixture.pid);
    while Instant::now() < deadline {
        let response = exchange(&[query.clone()]).remove(0);
        if response.starts_with("geometry ") {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "fixture root PID {} never mapped into cua-compositor",
        fixture.pid
    );
}

fn hex(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn wait_for_fixture(journal: &FixtureJournal, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if journal.contains(marker) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "fixture journal did not observe {marker:?}; latest={}",
        journal.snapshot()
    );
}

#[test]
#[ignore = "requires the cua-compositor nested Wayland E2E environment"]
fn transient_seats_are_isolated_and_destroyable() {
    // Electron binds its Wayland globals at startup, so the transient seats
    // must exist before either fixture initializes its registry.
    let fixture_a = launch_fixture("agent-a");
    let fixture_b = launch_fixture("agent-b");
    wait_for_target(&fixture_a);
    wait_for_target(&fixture_b);
    let target_a = target(&fixture_a);
    let target_b = target(&fixture_b);
    let default_focus_before = exchange(&["q 0".to_owned()]).remove(0);

    assert_eq!(
        exchange(&["seat create agent-a".to_owned()]),
        vec!["ok"],
        "missing transient-seat lifecycle capability"
    );
    assert_eq!(exchange(&["seat create agent-b".to_owned()]), vec!["ok"]);

    // The shared Electron fixture is a three-column grid. This point lands in
    // the third-column keyboard control instead of the older stacked layout's
    // now-unrelated lower-left position.
    assert_eq!(
        exchange(&[
            format!("sm agent-a {target_a} 0 700 95"),
            format!("sb agent-a {target_a} 0 272 1"),
            format!("sb agent-a {target_a} 0 272 0"),
            format!("sm agent-b {target_b} 0 700 95"),
            format!("sb agent-b {target_b} 0 272 1"),
            format!("sb agent-b {target_b} 0 272 0"),
        ]),
        vec!["ok"; 6],
        "seat-scoped pointer input must be accepted"
    );
    // Chromium processes the Wayland pointer click asynchronously. Let it
    // establish DOM focus before routing the typed strings.
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        exchange(&[
            format!("st agent-a {target_a} {}", hex("seat-a")),
            format!("st agent-b {target_b} {}", hex("seat-b")),
        ]),
        vec!["ok"; 2],
        "seat-scoped keyboard input must be accepted"
    );
    wait_for_fixture(&fixture_a.journal, "seat-a");
    wait_for_fixture(&fixture_b.journal, "seat-b");
    assert_eq!(
        exchange(&["q 0".to_owned()]).remove(0),
        default_focus_before
    );

    let drag_a = target_a.clone();
    let drag_b = target_b.clone();
    let agent_a = thread::spawn(move || {
        exchange(&[
            format!("sm agent-a {drag_a} 0 20 20"),
            format!("sb agent-a {drag_a} 0 272 1"),
            format!("sm agent-a {drag_a} 0 220 120"),
            format!("sb agent-a {drag_a} 0 272 0"),
        ])
    });
    let agent_b = thread::spawn(move || {
        exchange(&[
            format!("sm agent-b {drag_b} 0 20 20"),
            format!("sb agent-b {drag_b} 0 272 1"),
            format!("sm agent-b {drag_b} 0 220 120"),
            format!("sb agent-b {drag_b} 0 272 0"),
        ])
    });
    assert_eq!(agent_a.join().expect("agent A drag"), vec!["ok"; 4]);
    assert_eq!(agent_b.join().expect("agent B drag"), vec!["ok"; 4]);

    assert_eq!(exchange(&["seat destroy agent-a".to_owned()]), vec!["ok"]);
    assert_eq!(
        exchange(&[format!("st agent-b {target_b} {}", hex("-still-b"))]),
        vec!["ok"],
        "destroying agent A must leave agent B functional"
    );
    wait_for_fixture(&fixture_b.journal, "seat-b-still-b");
    assert_eq!(
        exchange(&["q 0".to_owned()]).remove(0),
        default_focus_before
    );
}
