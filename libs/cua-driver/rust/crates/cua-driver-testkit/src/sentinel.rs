//! Full-desktop foreground sentinel used by background E2E cells.

use std::collections::HashSet;
use std::fs;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::e2e::OracleKind;
use crate::{harness_app, ChildReaper, Driver};

/// A foreground Electron window that journals focus and leaked input while it
/// fully occludes the background target.
pub struct ForegroundSentinel {
    journal_path: std::path::PathBuf,
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
        let before_windows = window_ids(driver);

        let mut command = Command::new(&electron.path);
        command
            .args(&electron.args)
            .env("CUA_E2E_SENTINEL", "1")
            .env("CUA_E2E_SENTINEL_JOURNAL", &journal_path)
            .env("CUA_E2E_USER_DATA_DIR", user_data.path())
            .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut reaper = ChildReaper::new();
        reaper
            .spawn(&mut command)
            .expect("launch foreground sentinel");

        let window_deadline = Instant::now() + Duration::from_secs(15);
        let sentinel_pid = loop {
            let windows = driver.call("list_windows", serde_json::json!({}));
            if let Some(pid) = windows.structured()["windows"]
                .as_array()
                .and_then(|windows| {
                    windows.iter().find_map(|window| {
                        let id = window["window_id"].as_u64()?;
                        let title = window["title"].as_str().unwrap_or("");
                        (!before_windows.contains(&id) && title.contains("CuaTestHarness Sentinel"))
                            .then(|| window["pid"].as_u64().unwrap_or(0) as u32)
                    })
                })
            {
                break sentinel_pid_nonzero(pid);
            }
            assert!(
                Instant::now() < window_deadline,
                "foreground sentinel window did not appear"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        reaper.track_pid(sentinel_pid);

        let focus_deadline = Instant::now() + Duration::from_secs(10);
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
        fs::write(&journal_path, "").expect("reset focused sentinel journal");

        Self {
            journal_path,
            _reaper: reaper,
            _user_data: user_data,
        }
    }

    pub fn observe(&self) -> (Vec<OracleKind>, Vec<String>) {
        std::thread::sleep(Duration::from_millis(200));
        let journal = fs::read_to_string(&self.journal_path).unwrap_or_default();
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
}

fn sentinel_pid_nonzero(pid: u32) -> u32 {
    assert_ne!(pid, 0, "foreground sentinel window has no process id");
    pid
}

fn window_ids(driver: &mut impl Driver) -> HashSet<u64> {
    driver
        .call("list_windows", serde_json::json!({}))
        .structured()["windows"]
        .as_array()
        .map(|windows| {
            windows
                .iter()
                .filter_map(|window| window["window_id"].as_u64())
                .collect()
        })
        .unwrap_or_default()
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
