// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A long-lived macOS daemon without the cursor overlay must report the app
//! that is in front now, not the one that was in front at its first read
//! (#5045). The testkit daemon is `serve --no-overlay` started as a child
//! process, which runs no AppKit main run loop. The oracle is LaunchServices
//! through `lsappinfo`, which does not depend on this process's run loop.
//!
//! Run with:
//! `cargo test -p cua-driver-e2e --test frontmost_app_macos_test -- --ignored --nocapture --test-threads=1`

#![cfg(target_os = "macos")]

use std::process::Command;
use std::time::{Duration, Instant};

use cua_driver_testkit::{Driver, McpDriver};

fn lsappinfo(args: &[&str]) -> String {
    let output = Command::new("lsappinfo")
        .args(args)
        .output()
        .expect("run lsappinfo");
    assert!(output.status.success(), "lsappinfo {args:?} failed");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The pid in `lsappinfo info -only pid` output: `"pid"=468` on older macOS,
/// a full record containing `pid = 468 ...` on macOS 27.
fn parse_pid(info: &str) -> Option<i64> {
    let (_, rest) = info.split_once("pid")?;
    let digits: String = rest
        .trim_start_matches(|c: char| c == '"' || c == '=' || c.is_whitespace())
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// LaunchServices' frontmost application pid.
fn front_pid() -> Option<i64> {
    let asn = lsappinfo(&["front"]);
    parse_pid(&lsappinfo(&["info", "-only", "pid", &asn]))
}

fn pid_of_app(name: &str) -> Option<i64> {
    parse_pid(&lsappinfo(&["info", "-only", "pid", name]))
}

/// Bring `app` to the front through LaunchServices and return its pid once
/// it holds the front.
fn bring_app_to_front(app: &str) -> i64 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            Command::new("open")
                .args(["-a", app])
                .status()
                .expect("run open")
                .success(),
            "open -a {app} failed"
        );
        std::thread::sleep(Duration::from_millis(500));
        if let Some(pid) = pid_of_app(app).filter(|pid| front_pid() == Some(*pid)) {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "{app} did not take the front; front pid is {:?}",
            front_pid()
        );
    }
}

/// The pid `list_apps` marks `active`, read while the oracle front pid holds
/// still around the call. Returns `(reported, oracle)`.
fn active_pid(driver: &mut McpDriver) -> (Option<i64>, i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let before = front_pid();
        let response = driver.call("list_apps", serde_json::json!({}));
        assert!(
            !response.is_error(),
            "list_apps failed: {}",
            response.text()
        );
        if let Some(oracle) = before.filter(|pid| front_pid() == Some(*pid)) {
            let reported = response.structured()["apps"]
                .as_array()
                .expect("list_apps apps array")
                .iter()
                .find(|app| app["active"] == true)
                .and_then(|app| app["pid"].as_i64());
            return (reported, oracle);
        }
        assert!(
            Instant::now() < deadline,
            "the front app kept changing around list_apps"
        );
    }
}

struct QuitCalculator;

impl Drop for QuitCalculator {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-x", "Calculator"]).status();
    }
}

#[test]
#[ignore = "needs a logged-in macOS desktop; switches the frontmost app"]
fn no_overlay_daemon_reports_the_current_frontmost_app() {
    bring_app_to_front("Finder");
    let mut driver = McpDriver::spawn().expect("start a no-overlay cua-driver daemon");

    let (reported, finder) = active_pid(&mut driver);
    assert_eq!(reported, Some(finder), "first read: Finder is in front");

    let _quit = QuitCalculator;
    let calculator = bring_app_to_front("Calculator");
    let (reported, oracle) = active_pid(&mut driver);
    assert_eq!(oracle, calculator);
    assert_eq!(
        reported,
        Some(calculator),
        "list_apps kept the app that was in front at the daemon's first read"
    );

    let finder = bring_app_to_front("Finder");
    let (reported, oracle) = active_pid(&mut driver);
    assert_eq!(oracle, finder);
    assert_eq!(reported, Some(finder), "switch back to Finder");
}

/// The LaunchServices ASN of `pid` or one of its child processes.
fn registered_asn(pid: u32) -> Option<String> {
    let children = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .expect("run pgrep");
    std::iter::once(pid.to_string())
        .chain(
            String::from_utf8_lossy(&children.stdout)
                .lines()
                .map(str::to_owned),
        )
        .map(|pid| lsappinfo(&["find", &format!("pid={}", pid.trim())]))
        .find(|asn| !asn.is_empty())
}

/// Starting a daemon with the cursor overlay must leave the user's app in
/// front. With the Accessory activation policy, AppKit made the daemon the
/// active app as the overlay finished launching, taking keyboard focus from
/// the user's window.
#[test]
#[ignore = "needs a logged-in macOS desktop with an app in front"]
fn overlay_daemon_start_leaves_the_front_app_in_front() {
    let front = front_pid().expect("an app in front");
    let driver = McpDriver::spawn_named_with_overlay("overlay-front")
        .expect("start a cua-driver daemon with the cursor overlay");
    let daemon = driver.daemon_pid().expect("daemon pid");

    // The overlay registers with LaunchServices as it starts AppKit, and the
    // old policy took the front within about 50 ms of that. Outside embedded
    // mode `serve` relaunches itself, so the overlay may be in a child process.
    let deadline = Instant::now() + Duration::from_secs(10);
    let asn = loop {
        if let Some(asn) = registered_asn(daemon) {
            break asn;
        }
        assert!(
            Instant::now() < deadline,
            "the overlay daemon never registered with LaunchServices"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        assert_eq!(
            front_pid(),
            Some(front),
            "the overlay daemon took the front"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let info = lsappinfo(&["info", &asn]);
    assert!(
        info.contains("type=\"BackgroundOnly\""),
        "the overlay daemon can still become active: {info}"
    );
}
