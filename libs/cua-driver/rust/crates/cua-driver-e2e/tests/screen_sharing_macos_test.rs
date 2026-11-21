// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Foreground typing into an exact Apple Screen Sharing window (#3201).
//!
//! The test serves two local VNC desktops, connects Screen Sharing to both so
//! one pid owns two `AXSharedScreen` windows, minimizes one, fronts Finder,
//! and types into the minimized window with a pixel-addressed foreground
//! `type_text`. The VNC servers are the oracle: the target desktop must
//! receive exactly the typed keys, the sibling desktop none, and Finder must
//! get the front back.
//!
//! Run with:
//! `cargo test -p cua-driver-e2e --test screen_sharing_macos_test -- --ignored --nocapture --test-threads=1`

#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Observation, OracleKind,
    Scope, Targeting,
};
use cua_driver_testkit::{Driver, McpDriver};

const SCREEN_SHARING: &str = "Screen Sharing";
const FRAME_WIDTH: u16 = 800;
const FRAME_HEIGHT: u16 = 600;

/// A minimal RFB server: a gray framebuffer that accepts any VNC password and
/// records the keysym of every key-down the client forwards.
struct VncDesktop {
    name: String,
    port: u16,
    keys: Arc<Mutex<Vec<u32>>>,
}

impl VncDesktop {
    fn serve(name: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind VNC desktop");
        let port = listener.local_addr().expect("VNC desktop address").port();
        let keys = Arc::new(Mutex::new(Vec::new()));
        let (thread_name, thread_keys) = (name.clone(), keys.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (name, keys) = (thread_name.clone(), thread_keys.clone());
                std::thread::spawn(move || {
                    let _ = serve_rfb(stream, &name, &keys);
                });
            }
        });
        Self { name, port, keys }
    }

    /// Printable keys received so far; modifier and function keysyms
    /// (0xff00 and up) are left out.
    fn typed(&self) -> String {
        self.keys
            .lock()
            .expect("VNC key log")
            .iter()
            .filter(|keysym| **keysym < 0xff00)
            .filter_map(|keysym| char::from_u32(*keysym))
            .collect()
    }
}

fn read_exact(stream: &mut TcpStream, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn serve_rfb(mut stream: TcpStream, name: &str, keys: &Mutex<Vec<u32>>) -> std::io::Result<()> {
    stream.write_all(b"RFB 003.008\n")?;
    let version = read_exact(&mut stream, 12)?;
    // Apple's client answers 3.3, where the server picks the security type.
    if version == b"RFB 003.003\n" {
        stream.write_all(&2u32.to_be_bytes())?;
    } else {
        stream.write_all(&[1, 2])?;
        read_exact(&mut stream, 1)?;
    }
    stream.write_all(&[0; 16])?;
    read_exact(&mut stream, 16)?;
    stream.write_all(&0u32.to_be_bytes())?;
    read_exact(&mut stream, 1)?;

    let mut init = Vec::new();
    init.extend(FRAME_WIDTH.to_be_bytes());
    init.extend(FRAME_HEIGHT.to_be_bytes());
    // 32 bpp, depth 24, little endian, true colour, 8 bits per channel.
    init.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    init.extend((name.len() as u32).to_be_bytes());
    init.extend(name.as_bytes());
    stream.write_all(&init)?;

    let mut bytes_per_pixel = 4usize;
    let mut sent_frame = false;
    loop {
        match read_exact(&mut stream, 1)?[0] {
            0 => bytes_per_pixel = usize::from(read_exact(&mut stream, 19)?[3] / 8),
            2 => {
                let header = read_exact(&mut stream, 3)?;
                let count = u16::from_be_bytes([header[1], header[2]]);
                read_exact(&mut stream, 4 * usize::from(count))?;
            }
            3 => {
                let incremental = read_exact(&mut stream, 9)?[0] != 0;
                if incremental && sent_frame {
                    std::thread::sleep(Duration::from_millis(200));
                    stream.write_all(&[0, 0, 0, 0])?;
                    continue;
                }
                let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0];
                update.extend(FRAME_WIDTH.to_be_bytes());
                update.extend(FRAME_HEIGHT.to_be_bytes());
                update.extend(0i32.to_be_bytes());
                update.resize(
                    update.len()
                        + usize::from(FRAME_WIDTH) * usize::from(FRAME_HEIGHT) * bytes_per_pixel,
                    0x80,
                );
                stream.write_all(&update)?;
                sent_frame = true;
            }
            4 => {
                let event = read_exact(&mut stream, 7)?;
                if event[0] != 0 {
                    let keysym = u32::from_be_bytes([event[3], event[4], event[5], event[6]]);
                    keys.lock().expect("VNC key log").push(keysym);
                }
            }
            5 => {
                read_exact(&mut stream, 5)?;
            }
            6 => {
                let header = read_exact(&mut stream, 7)?;
                let len = u32::from_be_bytes([header[3], header[4], header[5], header[6]]);
                read_exact(&mut stream, len as usize)?;
            }
            other => {
                return Err(std::io::Error::other(format!(
                    "unsupported RFB client message {other}"
                )))
            }
        }
    }
}

fn lsappinfo(args: &[&str]) -> String {
    let output = Command::new("lsappinfo")
        .args(args)
        .output()
        .expect("run lsappinfo");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn front_pid() -> Option<i64> {
    let asn = lsappinfo(&["front"]);
    lsappinfo(&["info", "-only", "pid", &asn])
        .rsplit('=')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn front_app() -> String {
    let asn = lsappinfo(&["front"]);
    lsappinfo(&["info", "-only", "name", &asn])
}

fn pid_of_app(name: &str) -> Option<i64> {
    lsappinfo(&["info", "-only", "pid", name])
        .rsplit('=')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Bring `app` to the front and keep it there for a settle window. A new
/// Screen Sharing connection can take the front back a moment after `open`
/// handed it to `app`, so one sample is a race.
fn bring_app_to_front(app: &str) -> i64 {
    const FRONT_SETTLE: Duration = Duration::from_millis(1500);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stable_since: Option<Instant> = None;
    loop {
        if let Some(pid) = pid_of_app(app).filter(|pid| front_pid() == Some(*pid)) {
            let since = *stable_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= FRONT_SETTLE {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        stable_since = None;
        assert!(
            Instant::now() < deadline,
            "{app} did not stay in front; front is {}",
            front_app()
        );
        Command::new("open")
            .args(["-a", app])
            .status()
            .expect("run open");
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Quits Screen Sharing at the end when the test was the one that started it.
struct QuitScreenSharing(bool);

impl Drop for QuitScreenSharing {
    fn drop(&mut self) {
        if self.0 {
            let _ = Command::new("pkill").args(["-x", SCREEN_SHARING]).status();
        }
    }
}

/// Open a Screen Sharing connection to `desktop` and return the pid and the
/// id of the window that shows it, found by the desktop name in its title.
fn connect(driver: &mut McpDriver, desktop: &VncDesktop) -> (i64, u64) {
    let opened = Command::new("open")
        .arg(format!("vnc://:cua@127.0.0.1:{}", desktop.port))
        .status()
        .expect("open vnc URL");
    assert!(opened.success(), "could not open the vnc:// URL");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({}));
        let window = windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| {
                window["app_name"] == SCREEN_SHARING
                    && window["title"] == desktop.name.as_str()
                    && window["is_on_screen"] == true
            })
            .and_then(|window| Some((window["pid"].as_i64()?, window["window_id"].as_u64()?)));
        if let Some(found) = window {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "Screen Sharing showed no window for {}",
            desktop.name
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn window_state(driver: &mut McpDriver, pid: i64, window_id: u64) -> serde_json::Value {
    let state = driver.call(
        "get_window_state",
        serde_json::json!({ "pid": pid, "window_id": window_id, "tree_format": "elements" }),
    );
    assert!(
        !state.is_error(),
        "get_window_state failed: {}",
        state.text()
    );
    state.structured().clone()
}

fn minimize(driver: &mut McpDriver, pid: i64, window_id: u64) {
    let state = window_state(driver, pid, window_id);
    let token = state["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|element| element["subrole"] == "AXMinimizeButton")
        .and_then(|element| element["element_token"].as_str())
        .expect("Screen Sharing window has a minimize button")
        .to_owned();
    let clicked = driver.call(
        "click",
        serde_json::json!({ "pid": pid, "window_id": window_id, "element_token": token }),
    );
    assert!(
        !clicked.is_error(),
        "minimize click failed: {}",
        clicked.text()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({ "pid": pid }));
        let on_screen = windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| window["window_id"].as_u64() == Some(window_id))
            .map(|window| window["is_on_screen"] == true);
        if on_screen == Some(false) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Screen Sharing window {window_id} did not minimize"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after the Unix epoch")
        .as_nanos();
    format!("{}", nanos % 1_000_000_000)
}

#[test]
#[ignore = "needs a logged-in macOS desktop with Screen Sharing; switches the frontmost app"]
fn foreground_type_text_reaches_a_minimized_screen_sharing_window() {
    let cell_id = "macos-screen-sharing-type-text-px-foreground";
    let case = CaseSpec::delivered(
        cell_id,
        "screen-sharing",
        "appkit",
        "type_text",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Window,
        DriverRoute::MacosCgEventHid,
        vec![
            OracleKind::FixtureState,
            OracleKind::Focus,
            OracleKind::NoLeakedInput,
        ],
    );
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(cell_id)
            .expect("start installed macOS daemon proxy");
        *evidence = recording_evidence(driver.recording_dir());
        let suffix = unique_suffix();
        let target = VncDesktop::serve(format!("cua-e2e-target-{suffix}"));
        let sibling = VncDesktop::serve(format!("cua-e2e-sibling-{suffix}"));
        let _quit = QuitScreenSharing(pid_of_app(SCREEN_SHARING).is_none());

        let (pid, target_window) = connect(&mut driver, &target);
        let (sibling_pid, _) = connect(&mut driver, &sibling);
        assert_eq!(
            pid, sibling_pid,
            "both connections belong to one Screen Sharing"
        );
        minimize(&mut driver, pid, target_window);
        let finder = bring_app_to_front("Finder");

        let state = window_state(&mut driver, pid, target_window);
        let text = format!("cua{suffix}");
        driver.start_behavior_recording();
        let typed = driver.call(
            "type_text",
            serde_json::json!({
                "pid": pid, "window_id": target_window,
                "x": f64::from(FRAME_WIDTH) / 2.0, "y": f64::from(FRAME_HEIGHT) / 2.0,
                "text": text, "delivery_mode": "foreground"
            }),
        );
        assert!(
            !typed.is_error(),
            "foreground type_text into the minimized Screen Sharing window failed: {}\nsnapshot: {}",
            typed.text(),
            state["tree_markdown"]
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while target.typed() != text && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            target.typed(),
            text,
            "the target desktop did not receive exactly the typed text"
        );
        assert_eq!(
            sibling.typed(),
            "",
            "keys leaked into the sibling Screen Sharing window"
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        while front_pid() != Some(finder) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(
            front_pid(),
            Some(finder),
            "Finder did not get the front back after the foreground type_text; front is {}",
            front_app()
        );
        Observation::delivered(
            vec![
                OracleKind::FixtureState,
                OracleKind::Focus,
                OracleKind::NoLeakedInput,
            ],
            Default::default(),
        )
    });
}
