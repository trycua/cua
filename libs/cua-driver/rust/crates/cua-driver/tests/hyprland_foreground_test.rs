//! Supplemental native Hyprland foreground safety regression.
//!
//! Requires the exact-candidate Driver/plugin, GTK3/PyGObject, hyprctl, and a
//! disposable Hyprland desktop. Set CUA_DRIVER_RS_ENABLE_WAYLAND=1 and
//! CUA_E2E_UNRESTRICTED_GUI=1; optionally set CUA_TEST_DRIVER_BIN and
//! CUA_TEST_WORKSPACE_ROOT using the normal testkit overrides. Run with
//! --ignored --nocapture --test-threads=1. This does not replace the desktop matrix.
//! The grab case additionally requires cc, pkg-config, wayland-scanner,
//! libwayland-client development files, and CUA_TEST_VIRTUAL_POINTER_XML pointing
//! to the source wlr-virtual-pointer-unstable-v1.xml protocol.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cua_driver_testkit::{spawn_in_job, workspace_root, Driver, McpDriver, ToolResponse};
use serde_json::{json, Value};

struct Fixture {
    pid: u32,
    window_id: u64,
    address: String,
    journal: PathBuf,
}

fn hyprctl(args: &[&str]) -> String {
    let output = Command::new("hyprctl")
        .args(args)
        .output()
        .expect("hyprctl must be installed in the native desktop");
    assert!(output.status.success(), "hyprctl {args:?}: {output:?}");
    String::from_utf8(output.stdout).expect("hyprctl UTF-8 output")
}

fn active_address() -> String {
    let active: Value = serde_json::from_str(&hyprctl(&["-j", "activewindow"]))
        .expect("Hyprland activewindow JSON");
    active["address"].as_str().unwrap_or("").to_owned()
}

fn focus_window(address: &str) {
    let selector = serde_json::to_string(&format!("address:{address}")).unwrap();
    let action = format!("hl.dsp.focus({{ window = {selector} }})");
    assert_eq!(hyprctl(&["dispatch", &action]).trim(), "ok");
    assert_eq!(active_address(), address);
}

fn events(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).expect("fixture journal must exist");
    // A concurrently written final line is not an event until its newline lands.
    text.split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| serde_json::from_str(line).expect("valid fixture journal event"))
        .collect()
}

fn launch(driver: &mut McpDriver, dir: &Path, actor: &str) -> Fixture {
    let journal = dir.join(format!("{actor}.jsonl"));
    let fixture = workspace_root().join("../tests/fixtures/apps/linux/isolated-input/main.py");
    assert!(fixture.is_file(), "missing native fixture: {fixture:?}");
    let child = spawn_in_job(
        Command::new("python3")
            .arg(fixture)
            .args(["--actor", actor, "--journal"])
            .arg(&journal)
            .env("GDK_BACKEND", "wayland")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .expect("launch repository raw-event GTK3 fixture");
    let pid = child.id();
    driver.reaper().push(child);
    wait_fixture(driver, pid, journal, &format!("Cua Isolated Input {actor}"))
}

fn wait_fixture(driver: &mut McpDriver, pid: u32, journal: PathBuf, title: &str) -> Fixture {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let windows = driver.call("list_windows", json!({"pid": pid}));
        if let Some(window_id) = windows.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["pid"].as_u64() == Some(u64::from(pid))
                        && window["title"].as_str() == Some(title)
                })
            })
            .and_then(|window| window["window_id"].as_u64())
        {
            let clients: Value =
                serde_json::from_str(&hyprctl(&["-j", "clients"])).expect("Hyprland clients JSON");
            let client = clients
                .as_array()
                .unwrap()
                .iter()
                .find(|client| {
                    client["pid"].as_u64() == Some(u64::from(pid))
                        && client["title"].as_str() == Some(title)
                })
                .expect("fixture must be independently visible in Hyprland IPC");
            assert_eq!(client["xwayland"], false, "fixture must be native Wayland");
            let address = client["address"].as_str().unwrap();
            assert_eq!(
                u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap(),
                window_id,
                "Driver window identity must match the independent compositor address"
            );
            return Fixture {
                pid,
                window_id,
                address: address.to_owned(),
                journal,
            };
        }
        assert!(
            Instant::now() < deadline,
            "fixture failed to map: {}",
            windows.text()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn launch_siblings(driver: &mut McpDriver, dir: &Path) -> (Fixture, Fixture) {
    let script = workspace_root().join("../hyprland-plugin/tests/foreground_siblings.py");
    assert!(script.is_file(), "missing sibling fixture: {script:?}");
    let child = spawn_in_job(
        Command::new("python3")
            .arg(script)
            .arg("--journal-dir")
            .arg(dir)
            .env("GDK_BACKEND", "wayland")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .expect("launch same-client native GTK3 siblings");
    let pid = child.id();
    driver.reaper().push(child);
    let target = wait_fixture(
        driver,
        pid,
        dir.join("Target.jsonl"),
        "Cua Foreground Target",
    );
    let sibling = wait_fixture(
        driver,
        pid,
        dir.join("Sibling.jsonl"),
        "Cua Foreground Sibling",
    );
    assert_ne!(target.window_id, sibling.window_id);
    assert_ne!(target.address, sibling.address);
    for fixture in [&target, &sibling] {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !events(&fixture.journal).iter().any(|event| {
            event["kind"] == "ready"
                && event["pid"] == pid
                && event["same_display"] == true
                && event["native_wayland"] == true
        }) {
            assert!(
                Instant::now() < deadline,
                "same-client native fixture readiness must be proven"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    focus_window(&sibling.address);
    (target, sibling)
}

fn snapshot(driver: &mut McpDriver, fixture: &Fixture) -> ToolResponse {
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let screenshot = fixture
        .journal
        .with_file_name(format!("snapshot-{sequence}-{}.png", fixture.pid));
    let state = driver.call(
        "get_window_state",
        json!({"pid": fixture.pid, "window_id": fixture.window_id,
               "screenshot_out_file": screenshot}),
    );
    assert!(!state.is_error(), "snapshot failed: {}", state.text());
    assert!(
        state.structured().get("screenshot_error").is_none(),
        "{}",
        state.structured()
    );
    for dimension in ["screenshot_width", "screenshot_height"] {
        assert!(state.structured()[dimension].as_u64().unwrap_or(0) > 0);
    }
    state
}

#[test]
#[ignore = "requires disposable native Hyprland, candidate plugin/Driver, GTK3 and unrestricted GUI"]
fn foreground_drag_focus_loss_does_not_release_into_successor() {
    drag_focus_loss(false);
}

#[test]
#[ignore = "requires disposable native Hyprland, candidate plugin/Driver, GTK3 and unrestricted GUI"]
fn foreground_drag_focus_loss_does_not_release_into_same_client_sibling() {
    drag_focus_loss(true);
}

fn native_preflight() {
    assert!(cfg!(target_os = "linux"), "native Linux test only");
    assert!(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some());
    assert_eq!(
        std::env::var("CUA_DRIVER_RS_ENABLE_WAYLAND").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("CUA_E2E_UNRESTRICTED_GUI").as_deref(),
        Ok("1")
    );
}

fn drag_focus_loss(same_client: bool) {
    native_preflight();
    let evidence_root = std::env::var_os("CUA_E2E_RECORDINGS_ROOT");
    let journals = if let Some(root) = &evidence_root {
        tempfile::Builder::new()
            .prefix("hyprland-foreground-focus-loss-")
            .tempdir_in(root)
    } else {
        tempfile::tempdir()
    }
    .expect("isolated fixture journals");
    let journals_path = journals.path().to_path_buf();
    // Retain raw journals and snapshots in the harness archive, including panics.
    let _cleanup = if evidence_root.is_some() {
        let _ = journals.keep();
        None
    } else {
        Some(journals)
    };
    let mut driver = McpDriver::spawn_named("hyprland-foreground-focus-loss")
        .expect("candidate Driver must be available");
    let (target, successor) = if same_client {
        launch_siblings(&mut driver, &journals_path)
    } else {
        (
            launch(&mut driver, &journals_path, "Background"),
            launch(&mut driver, &journals_path, "Foreground"),
        )
    };
    snapshot(&mut driver, &successor);
    let before = snapshot(&mut driver, &target);
    assert_eq!(active_address(), successor.address);
    let target_start = events(&target.journal).len();
    let successor_start = events(&successor.journal).len();
    let width = before.structured()["screenshot_width"].as_f64().unwrap();
    let height = before.structured()["screenshot_height"].as_f64().unwrap();
    driver.start_behavior_recording();

    // Interrupt only after the application independently confirms the press.
    // The MCP call remains in flight while this thread changes primary focus.
    let interrupt = std::thread::spawn({
        let journal = target.journal.clone();
        let target_address = target.address.clone();
        let successor_address = successor.address.clone();
        move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if events(&journal)[target_start..]
                    .iter()
                    .any(|event| event["kind"] == "button-press" && event["button"] == 1)
                {
                    assert_eq!(active_address(), target_address);
                    focus_window(&successor_address);
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "drag never reached the target journal"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    let result = driver.call(
        "drag",
        json!({
            "pid": target.pid, "window_id": target.window_id,
            "from_x": width * 0.25, "from_y": height * 0.5,
            "to_x": width * 0.75, "to_y": height * 0.5,
            "duration_ms": 2000, "steps": 100, "delivery_mode": "foreground"
        }),
    );
    std::fs::write(
        journals_path.join("drag-result.json"),
        serde_json::to_vec_pretty(result.structured()).unwrap(),
    )
    .expect("retain drag result");
    interrupt.join().expect("independent focus intervention");
    snapshot(&mut driver, &target);
    snapshot(&mut driver, &successor);
    assert_eq!(
        result.action_effect(),
        Some("partial"),
        "{}",
        result.structured()
    );
    assert!(
        matches!(
            result.action_delivery_mode(),
            Some("foreground" | "unknown")
        ),
        "{}",
        result.structured()
    );
    assert_eq!(result.structured()["delivery"]["delivered_count"], 1);
    assert_eq!(active_address(), successor.address);

    // Observe through the original drag deadline, including late queued release.
    // The independent focus dispatcher may warp the cursor, so motion alone
    // cannot be attributed to Driver. Raw press/release counters also do not
    // establish GTK's internal cancellation state after pointer focus leaves.
    let observation_deadline = Instant::now() + Duration::from_millis(2300);
    while Instant::now() < observation_deadline {
        let successor_events = events(&successor.journal);
        assert!(
            successor_events[successor_start..].iter().all(|event| {
                !matches!(
                    event["kind"].as_str(),
                    Some("button-press" | "button-release" | "key-press" | "key-release")
                )
            }),
            "successor received drag input: {:?}",
            &successor_events[successor_start..]
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let target_events = events(&target.journal);
    assert!(
        target_events[target_start..]
            .iter()
            .any(|event| event["kind"] == "state"),
        "target journal must remain live"
    );
    assert_eq!(
        target_events[target_start..]
            .iter()
            .filter(|event| { event["kind"] == "button-press" && event["button"] == 1 })
            .count(),
        1,
        "drag must not be replayed"
    );
    let successor_events = events(&successor.journal);
    assert!(
        successor_events[successor_start..]
            .iter()
            .any(|event| event["kind"] == "state"),
        "successor journal must remain live through observation"
    );
    snapshot(&mut driver, &successor);
    assert_eq!(active_address(), successor.address);
}

fn build_primary_grab(dir: &Path) -> PathBuf {
    let xml = PathBuf::from(
        std::env::var_os("CUA_TEST_VIRTUAL_POINTER_XML")
            .expect("set CUA_TEST_VIRTUAL_POINTER_XML to the source wlr virtual-pointer XML"),
    );
    assert!(xml.is_file(), "missing protocol source: {xml:?}");
    let header = dir.join("wlr-virtual-pointer-unstable-v1-client-protocol.h");
    let protocol = dir.join("virtual-pointer-protocol.c");
    for (mode, output) in [("client-header", &header), ("private-code", &protocol)] {
        let result = Command::new("wayland-scanner")
            .arg(mode)
            .arg(&xml)
            .arg(output)
            .output()
            .expect("wayland-scanner required");
        assert!(result.status.success(), "protocol generation: {result:?}");
    }
    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "wayland-client"])
        .output()
        .expect("pkg-config required");
    assert!(
        flags.status.success(),
        "libwayland-client development files required: {flags:?}"
    );
    let binary = dir.join("primary-grab");
    let result = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg("-I")
        .arg(dir)
        .arg(workspace_root().join("../hyprland-plugin/tests/primary_grab.c"))
        .arg(protocol)
        .args(String::from_utf8(flags.stdout).unwrap().split_whitespace())
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("C compiler required");
    assert!(
        result.status.success(),
        "compile independent virtual pointer: {result:?}"
    );
    binary
}

fn last_state(journal: &[Value]) -> &Value {
    journal
        .iter()
        .rev()
        .find(|event| event["kind"] == "state")
        .expect("fixture must have a live state heartbeat")
}

#[test]
#[ignore = "requires disposable native Hyprland, GTK3, Wayland development tools and protocol XML"]
fn foreground_action_refused_while_independent_virtual_pointer_holds_primary_grab() {
    native_preflight();
    let root = std::env::var_os("CUA_E2E_RECORDINGS_ROOT");
    let dir = if let Some(root) = &root {
        tempfile::Builder::new()
            .prefix("hyprland-foreground-grab-")
            .tempdir_in(root)
    } else {
        tempfile::tempdir()
    }
    .expect("test-owned helper build and evidence directory");
    let path = dir.path().to_path_buf();
    let _cleanup = if root.is_some() {
        let _ = dir.keep();
        None
    } else {
        Some(dir)
    };
    let binary = build_primary_grab(&path);
    let mut driver = McpDriver::spawn_named("hyprland-foreground-primary-grab")
        .expect("candidate Driver must be available");
    let (target, foreground) = launch_siblings(&mut driver, &path);
    snapshot(&mut driver, &target);
    let before = snapshot(&mut driver, &foreground);
    let desktop = driver.call("get_desktop_state", json!({}));
    assert!(!desktop.is_error(), "{}", desktop.text());
    let monitors: Value = serde_json::from_str(&hyprctl(&["-j", "monitors"])).unwrap();
    let monitors = monitors.as_array().unwrap();
    assert_eq!(
        monitors.len(),
        1,
        "virtual-pointer coordinate proof requires one output"
    );
    assert_eq!(monitors[0]["x"], 0);
    assert_eq!(monitors[0]["y"], 0);
    assert_eq!(monitors[0]["scale"].as_f64(), Some(1.0));
    assert_eq!(monitors[0]["transform"], 0);
    let bounds = &before.structured()["window_bounds"];
    let window_width = bounds["width"].as_f64().unwrap();
    let window_height = bounds["height"].as_f64().unwrap();
    assert!(window_width >= 200.0 && window_height >= 200.0);
    let x = (bounds["x"].as_f64().unwrap() + window_width / 2.0).round() as i64;
    let y = (bounds["y"].as_f64().unwrap() + window_height / 2.0).round() as i64;
    let width = desktop.structured()["screen_width"].as_u64().unwrap();
    let height = desktop.structured()["screen_height"].as_u64().unwrap();
    assert!(x >= 0 && y >= 0 && (x as u64) < width && (y as u64) < height);
    driver.start_behavior_recording();
    // Independent virtual pointer on the primary seat, not physical hardware.
    // Closing controlled stdin releases immediately; the helper also has a timeout.
    let mut grab = spawn_in_job(
        Command::new(binary)
            .args([
                x.to_string(),
                y.to_string(),
                width.to_string(),
                height.to_string(),
                "10000".into(),
                "controlled".into(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .expect("launch bounded independent primary-seat adversary");
    let control = grab.stdin.take().unwrap();
    driver.reaper().push(grab);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let journal = events(&foreground.journal);
        if journal
            .iter()
            .any(|event| event["kind"] == "state" && event["held"] == true)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "primary press must reach foreground journal"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(active_address(), foreground.address);
    let target_before = events(&target.journal);
    let foreground_before = events(&foreground.journal);
    assert_eq!(last_state(&foreground_before)["held"], true);
    snapshot(&mut driver, &target);
    let result = driver.call(
        "click",
        json!({"pid": target.pid, "window_id": target.window_id,
        "x": 100, "y": 100, "delivery_mode": "foreground"}),
    );
    std::fs::write(
        path.join("grab-refusal.json"),
        serde_json::to_vec_pretty(result.structured()).unwrap(),
    )
    .expect("retain foreground refusal");
    snapshot(&mut driver, &target);
    snapshot(&mut driver, &foreground);
    assert!(
        result.is_error(),
        "held primary grab must refuse: {}",
        result.structured()
    );
    assert_eq!(
        result.action_effect(),
        Some("refused"),
        "{}",
        result.structured()
    );
    assert_eq!(result.structured()["code"], "foreground_unavailable");
    assert_eq!(result.structured()["reason"], "primary_target_busy");
    assert_eq!(result.structured()["detail"], "foreground_physical_buttons");
    assert!(result.structured().get("delivery").is_none());
    assert_eq!(active_address(), foreground.address);
    std::thread::sleep(Duration::from_millis(300));
    let target_after = events(&target.journal);
    let foreground_after = events(&foreground.journal);
    for (before, after) in [
        (&target_before, &target_after),
        (&foreground_before, &foreground_after),
    ] {
        assert!(
            after[before.len()..]
                .iter()
                .any(|event| event["kind"] == "state"),
            "both journals must remain live during refusal"
        );
        assert!(
            after[before.len()..].iter().all(|event| !matches!(
                event["kind"].as_str(),
                Some("button-press" | "button-release" | "key-press" | "key-release")
            )),
            "refused action must emit no input: {:?}",
            &after[before.len()..]
        );
        for field in ["clicks", "keys", "held", "motion"] {
            assert_eq!(
                last_state(before)[field],
                last_state(after)[field],
                "changed {field}"
            );
        }
    }
    assert_eq!(last_state(&foreground_after)["held"], true);
    assert_eq!(active_address(), foreground.address);
    drop(control);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let journal = events(&foreground.journal);
        if last_state(&journal)["held"] == false {
            assert_eq!(
                journal[foreground_after.len()..]
                    .iter()
                    .filter(|event| event["kind"] == "button-release" && event["button"] == 1)
                    .count(),
                1
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "independent helper must release its grab"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Observe after the adversary releases too: a refusal must not queue a retry.
    std::thread::sleep(Duration::from_millis(300));
    let target_final = events(&target.journal);
    assert!(
        target_final[target_before.len()..]
            .iter()
            .all(|event| !matches!(
                event["kind"].as_str(),
                Some("button-press" | "button-release" | "key-press" | "key-release")
            )),
        "refused click must not replay after primary release"
    );
    for field in ["clicks", "keys", "held", "motion"] {
        assert_eq!(
            last_state(&target_before)[field],
            last_state(&target_final)[field]
        );
    }
    snapshot(&mut driver, &target);
    snapshot(&mut driver, &foreground);
    assert_eq!(active_address(), foreground.address);
}
