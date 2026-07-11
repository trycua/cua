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
    _reaper: ChildReaper,
    _user_data: tempfile::TempDir,
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
            loop {
                let journal = fs::read_to_string(&journal_path).unwrap_or_default();
                if journal.contains(r#""kind":"ready""#) && journal.contains(r#""kind":"focus""#) {
                    break;
                }
                assert!(
                    Instant::now() < focus_deadline,
                    "foreground sentinel did not become ready and focused: {journal}"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            activate_native_foreground(driver, target);
            wait_for_native_focus_stable(target);
        }
        fs::write(&journal_path, "").expect("reset focused sentinel journal");

        Self {
            journal_path,
            target,
            _reaper: reaper,
            _user_data: user_data,
        }
    }

    pub fn observe(&self) -> (Vec<OracleKind>, Vec<String>) {
        std::thread::sleep(Duration::from_millis(200));
        let journal = match fs::read_to_string(&self.journal_path) {
            Ok(journal) => journal,
            Err(error) => {
                return (
                    Vec::new(),
                    vec![format!(
                        "foreground sentinel journal could not be read: {error}"
                    )],
                )
            }
        };
        let mut passed = Vec::new();
        let mut violations = Vec::new();
        if journal.contains(r#""kind":"blur""#) {
            violations.push("foreground sentinel lost focus".to_owned());
        } else {
            passed.push(OracleKind::Focus);
        }
        let leaked = ["keydown", "pointerdown", "wheel", "contextmenu"]
            .into_iter()
            .filter(|kind| journal.contains(&format!(r#""kind":"{kind}""#)))
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

    pub fn target(&self) -> TargetWindow {
        self.target
    }

    /// Confirm the target is fully behind the ready foreground sentinel before
    /// the behavioral video boundary is crossed.
    pub fn assert_background_posture(&self, target: TargetWindow) -> Result<(), String> {
        let observer = DesktopObserver::new(NativeObserver::new(), target);
        let before = observer.snapshot().map_err(|error| error.to_string())?;
        if before.target_z == crate::observer::TargetZ::BackgroundOccluded {
            Ok(())
        } else {
            Err(format!(
                "background target was not fully occluded before recording: {:?}",
                before.target_z
            ))
        }
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

fn is_wayland_session() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn activate_native_foreground(driver: &mut impl Driver, target: TargetWindow) {
    #[cfg(target_os = "linux")]
    if is_wayland_session() {
        let output = Command::new("swaymsg")
            .arg(format!(r#"[pid="{}"] focus"#, target.pid))
            .output()
            .expect("could not run swaymsg to focus foreground sentinel");
        assert!(
            output.status.success(),
            "could not focus foreground sentinel through Sway: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
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
    #[cfg(target_os = "windows")]
    physically_focus_windows_sentinel(target);
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

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn activate_native_foreground(_driver: &mut impl Driver, _target: TargetWindow) {}

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
