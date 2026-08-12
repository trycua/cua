//! Hosted native-Wayland lifecycle and CPU certification for the layer-shell
//! cursor overlay. Run inside the repository's isolated Sway session.

#![cfg(target_os = "linux")]

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::RawDriver;

const OVERLAY_THREAD: &str = "cua-overlay-wl";

fn call(driver: &mut RawDriver, id: u64, name: &str, arguments: serde_json::Value) {
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    }));
    let response = driver.recv();
    assert_eq!(response["id"], id, "unexpected response: {response:?}");
    assert!(
        !response["result"]["isError"].as_bool().unwrap_or(false),
        "{name} failed: {response:?}"
    );
}

fn initialize(driver: &mut RawDriver) {
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }));
    let response = driver.recv();
    assert_eq!(response["id"], 1, "initialize failed: {response:?}");
}

fn overlay_tid(pid: u32) -> Option<u32> {
    let tasks = fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    tasks.filter_map(Result::ok).find_map(|task| {
        let comm = fs::read_to_string(task.path().join("comm")).ok()?;
        (comm.trim() == OVERLAY_THREAD).then(|| {
            task.file_name()
                .to_string_lossy()
                .parse::<u32>()
                .expect("numeric Linux task id")
        })
    })
}

fn wait_for_overlay_tid(pid: u32) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(tid) = overlay_tid(pid) {
            return tid;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("{OVERLAY_THREAD} did not start in daemon {pid}");
}

fn cpu_ticks(pid: u32, tid: u32) -> u64 {
    let stat =
        fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")).expect("read overlay task stat");
    let suffix = stat.rsplit_once(") ").expect("parse task comm").1;
    let fields: Vec<&str> = suffix.split_whitespace().collect();
    let utime = fields[11].parse::<u64>().expect("parse task utime");
    let stime = fields[12].parse::<u64>().expect("parse task stime");
    utime + stime
}

fn assert_idle_tick_bound(pid: u32, tid: u32, window: Duration, bound: u64) {
    let before = cpu_ticks(pid, tid);
    thread::sleep(window);
    let after = cpu_ticks(pid, tid);
    let delta = after.saturating_sub(before);
    eprintln!(
        "wayland overlay idle evidence: pid={pid} tid={tid} window_ms={} tick_delta={delta} bound={bound}",
        window.as_millis()
    );
    assert!(
        delta <= bound,
        "idle overlay used {delta} ticks (bound {bound})"
    );
}

#[test]
#[ignore]
fn no_overlay_flag_never_starts_wayland_overlay_thread() {
    let Some(mut driver) = RawDriver::spawn_with_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ]) else {
        panic!("source-built driver is required");
    };
    let pid = driver.daemon_pid().expect("daemon-backed driver pid");
    initialize(&mut driver);
    call(
        &mut driver,
        2,
        "move_cursor",
        serde_json::json!({"x": 300.0, "y": 240.0}),
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        overlay_tid(pid),
        None,
        "--no-overlay daemon unexpectedly started {OVERLAY_THREAD}"
    );
}

#[test]
#[ignore]
fn wayland_overlay_quiesces_and_recovers_after_capture_and_cursor_activity() {
    let Some(mut driver) = RawDriver::spawn_with_overlay_and_env(&[
        ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
        ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
    ]) else {
        panic!("source-built driver is required");
    };
    let pid = driver.daemon_pid().expect("daemon-backed driver pid");
    initialize(&mut driver);

    // Exercise the reported trigger class: native compositor capture followed
    // by daemon-owned cursor motion. Call the Linux capture backend directly:
    // `screenshot` is intentionally denied at the MCP authorization boundary
    // until that tool has a reviewed risk classification.
    let capture = platform_linux::wayland::screenshot_display_dispatch()
        .expect("capture native Wayland display before overlay activity");
    assert!(!capture.is_empty(), "native Wayland capture was empty");
    call(
        &mut driver,
        3,
        "set_agent_cursor_motion",
        serde_json::json!({"glide_duration_ms": 700, "idle_hide_ms": 0}),
    );
    call(
        &mut driver,
        4,
        "move_cursor",
        serde_json::json!({"x": 500.0, "y": 360.0}),
    );
    let tid = wait_for_overlay_tid(pid);
    thread::sleep(Duration::from_secs(2));
    assert_idle_tick_bound(pid, tid, Duration::from_secs(2), 1);

    call(
        &mut driver,
        5,
        "set_agent_cursor_enabled",
        serde_json::json!({"enabled": false}),
    );
    thread::sleep(Duration::from_millis(250));
    assert_idle_tick_bound(pid, tid, Duration::from_secs(1), 1);

    call(
        &mut driver,
        6,
        "set_agent_cursor_enabled",
        serde_json::json!({"enabled": true}),
    );
    let recovery_before = cpu_ticks(pid, tid);
    call(
        &mut driver,
        7,
        "move_cursor",
        serde_json::json!({"x": 900.0, "y": 600.0}),
    );
    thread::sleep(Duration::from_millis(500));
    let recovery_delta = cpu_ticks(pid, tid).saturating_sub(recovery_before);
    eprintln!("wayland overlay recovery evidence: tick_delta={recovery_delta}");
    assert!(
        recovery_delta > 0,
        "re-enabled overlay did not resume rendering"
    );

    thread::sleep(Duration::from_secs(2));
    assert_idle_tick_bound(pid, tid, Duration::from_secs(2), 1);
}
