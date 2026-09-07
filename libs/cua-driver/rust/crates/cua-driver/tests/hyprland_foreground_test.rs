//! Supplemental native Hyprland foreground safety regression.
//!
//! Requires the exact-candidate Driver/plugin, GTK3/PyGObject, hyprctl, and a
//! disposable Hyprland desktop. Set CUA_DRIVER_RS_ENABLE_WAYLAND=1 and
//! CUA_E2E_UNRESTRICTED_GUI=1; optionally set CUA_TEST_DRIVER_BIN and
//! CUA_TEST_WORKSPACE_ROOT using the normal testkit overrides. Run with
//! --ignored --nocapture --test-threads=1. This does not replace the desktop matrix.

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
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let windows = driver.call("list_windows", json!({"pid": pid}));
        if let Some(window_id) = windows.structured()["windows"]
            .as_array()
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window["pid"].as_u64() == Some(u64::from(pid))
                        && window["title"].as_str()
                            == Some(format!("Cua Isolated Input {actor}").as_str())
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
                .find(|client| client["pid"].as_u64() == Some(u64::from(pid)))
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
    let target = launch(&mut driver, &journals_path, "Background");
    let successor = launch(&mut driver, &journals_path, "Foreground");
    snapshot(&mut driver, &successor);
    let before = snapshot(&mut driver, &target);
    assert_eq!(active_address(), successor.address);
    let target_start = events(&target.journal).len();
    let successor_start = events(&successor.journal).len();
    let width = before.structured()["screenshot_width"].as_f64().unwrap();
    let height = before.structured()["screenshot_height"].as_f64().unwrap();

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
                    let selector = format!("address:{successor_address}");
                    let reply = hyprctl(&["dispatch", "focuswindow", &selector]);
                    assert_eq!(reply.trim(), "ok", "focus intervention failed");
                    assert_eq!(active_address(), successor_address);
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
    let observation_deadline = Instant::now() + Duration::from_millis(2300);
    while Instant::now() < observation_deadline {
        let successor_events = events(&successor.journal);
        assert!(
            successor_events[successor_start..].iter().all(|event| {
                !matches!(
                    event["kind"].as_str(),
                    Some("button-press" | "button-release")
                )
            }),
            "successor received drag input: {:?}",
            &successor_events[successor_start..]
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let target_events = events(&target.journal);
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
