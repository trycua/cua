use super::*;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

const WAIT: Duration = Duration::from_secs(30);
const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n";

fn fixture_state(
    directory: &Path,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(bytes) = std::fs::read(directory.join("state.json")) {
            let state: serde_json::Value =
                serde_json::from_slice(&bytes).expect("fixture JSON state");
            if predicate(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "fixture state deadline: {state}");
        } else {
            assert!(Instant::now() < deadline, "fixture did not publish state");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fifo_reader(path: &Path) -> File {
    assert!(Command::new("/usr/bin/mkfifo")
        .arg(path)
        .status()
        .unwrap()
        .success());
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .unwrap()
}

struct PendingCapture<R> {
    reader: Option<File>,
    task: Option<JoinHandle<R>>,
    bytes: Vec<u8>,
}

impl PendingCapture<ToolResponse> {
    fn start(directory: &Path, pid: u32, window: u64, response_path: PathBuf) -> Self {
        let path = directory.join("pending.png.pipe");
        let reader = fifo_reader(&path);
        let home = std::env::var("HOME").unwrap();
        let socket = std::env::var("CUA_E2E_MACOS_DAEMON_SOCKET")
            .unwrap_or_else(|_| format!("{home}/Library/Caches/cua-driver/cua-driver.sock"));
        let mut driver = McpDriver::spawn_daemon_proxy_unrecorded(&socket)
            .expect("second proxy must connect to the same installed daemon");
        let task = std::thread::spawn(move || {
            let response = driver.call(
                "get_window_state",
                serde_json::json!({
                    "pid": pid,
                    "window_id": window,
                    "include_screenshot": true,
                    "screenshot_out_file": path
                }),
            );
            std::fs::write(
                response_path,
                serde_json::to_vec_pretty(&response.raw).unwrap(),
            )
            .unwrap();
            response
        });
        Self {
            reader: Some(reader),
            task: Some(task),
            bytes: Vec::new(),
        }
    }
}

impl<R> PendingCapture<R> {
    fn wait_for_png_writer(&mut self) {
        let deadline = Instant::now() + WAIT;
        while self.bytes.len() < PNG_HEADER.len() {
            let mut buffer = [0u8; 8];
            match self
                .reader
                .as_mut()
                .unwrap()
                .read(&mut buffer[..PNG_HEADER.len() - self.bytes.len()])
            {
                Ok(count) => self.bytes.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("FIFO PNG header: {error}"),
            }
            assert!(
                !self.task.as_ref().unwrap().is_finished(),
                "capture completed without a blocked PNG writer"
            );
            assert!(
                Instant::now() < deadline,
                "built-in capture did not reach its PNG output writer"
            );
            if self.bytes.len() < PNG_HEADER.len() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(self.bytes, PNG_HEADER);
        self.assert_writer_pending();
    }

    fn assert_writer_pending(&self) {
        let mut descriptor = libc::pollfd {
            fd: self.reader.as_ref().unwrap().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert!(unsafe { libc::poll(&mut descriptor, 1, 0) } >= 0);
        assert_eq!(
            descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL),
            0,
            "capture writer closed before the pending-publication assertion"
        );
        assert!(
            !self.task.as_ref().unwrap().is_finished(),
            "capture already published"
        );
    }

    fn drain(&mut self) -> bool {
        let deadline = Instant::now() + WAIT;
        let mut buffer = [0u8; 65536];
        loop {
            match self.reader.as_mut().unwrap().read(&mut buffer) {
                Ok(count) if count > 0 => self.bytes.extend_from_slice(&buffer[..count]),
                Ok(_) => {
                    if self.task.as_ref().unwrap().is_finished() {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return false,
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    }

    fn finish(mut self, evidence: &Path) -> R {
        assert!(self.drain(), "capture did not finish after FIFO release");
        std::fs::write(evidence, &self.bytes).unwrap();
        assert!(
            self.bytes.len() > 256 * 1024,
            "fixture image was too small for reliable FIFO backpressure"
        );
        self.task
            .take()
            .unwrap()
            .join()
            .expect("capture proxy thread")
    }
}

impl<R> Drop for PendingCapture<R> {
    fn drop(&mut self) {
        if self.task.is_some() {
            let completed = self.drain();
            self.reader.take();
            if completed {
                let _ = self.task.take().unwrap().join();
            }
        }
    }
}

fn controlled_writer(directory: &Path) -> (PendingCapture<()>, Arc<std::sync::atomic::AtomicBool>) {
    let path = directory.join("control.pipe");
    let reader = fifo_reader(&path);
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = finished.clone();
    let task = std::thread::spawn(move || {
        let mut bytes = vec![0u8; 1024 * 1024];
        bytes[..PNG_HEADER.len()].copy_from_slice(PNG_HEADER);
        std::fs::write(path, bytes).unwrap();
        signal.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    (
        PendingCapture {
            reader: Some(reader),
            task: Some(task),
            bytes: Vec::new(),
        },
        finished,
    )
}

#[test]
fn fifo_backpressure_holds_writer_until_explicit_release() {
    let directory = tempfile::tempdir().unwrap();
    let (mut capture, finished) = controlled_writer(directory.path());
    capture.wait_for_png_writer();
    assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
    let output = directory.path().join("control.bin");
    capture.finish(&output);
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(std::fs::metadata(output).unwrap().len(), 1024 * 1024);
}

#[test]
fn fifo_drop_releases_blocked_writer() {
    let directory = tempfile::tempdir().unwrap();
    let (mut capture, finished) = controlled_writer(directory.path());
    capture.wait_for_png_writer();
    drop(capture);
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
#[ignore]
fn harness_appkit_pending_snapshot_cannot_retarget_token() {
    let case = native_foreground_case(
        "appkit",
        "snapshot_publication",
        Targeting::Ax,
        DriverRoute::MacosAxAction,
    );
    let label = case.cell_id.clone();
    execute_case(case, |evidence| {
        let mut driver = McpDriver::spawn_macos_daemon_proxy_named(&label)
            .expect("native publication test requires a running, TCC-authorized installed daemon");
        *evidence = recording_evidence(driver.recording_dir());
        let output = driver
            .recording_dir()
            .expect("set CUA_E2E_RECORDINGS_ROOT for native publication evidence")
            .to_path_buf();
        let directory = tempfile::tempdir().unwrap();
        let child = Command::new(harness_exe())
            .env("CUA_APPKIT_SNAPSHOT_DIR", directory.path())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("AppKit snapshot fixture");
        let harness = Harness {
            pid: child.id(),
            _app: child,
        };
        let ready = fixture_state(directory.path(), |state| {
            state["window_id"].as_u64().is_some_and(|id| id > 0)
        });
        let window = ready["window_id"].as_u64().unwrap();
        assert_eq!(ready["pid"].as_u64(), Some(harness.pid as u64));
        driver.start_behavior_recording();
        let initial_png = directory.path().join("initial.png");
        let first = driver.call(
            "get_window_state",
            serde_json::json!({
                "pid": harness.pid, "window_id": window, "include_screenshot": true,
                "screenshot_out_file": initial_png
            }),
        );
        std::fs::write(
            output.join("snapshot-initial-response.json"),
            serde_json::to_vec_pretty(&first.raw).unwrap(),
        )
        .unwrap();
        assert!(
            !first.is_error(),
            "initial native snapshot: {}",
            first.text()
        );
        assert!(
            std::fs::metadata(&initial_png).unwrap().len() > 256 * 1024,
            "initial fixture PNG must exceed FIFO capacity comfortably"
        );
        let original_index = element_index_by_id(first.tree_text(), "snapshot-original").unwrap();
        let old_token = element_token_by_id(&first, "snapshot-original");
        std::fs::write(directory.path().join("command"), "replace").unwrap();
        fixture_state(directory.path(), |state| state["generation"] == 1);
        let mut capture = PendingCapture::start(
            directory.path(),
            harness.pid,
            window,
            output.join("snapshot-capture-response.json"),
        );
        capture.wait_for_png_writer();
        let attempted = driver.call(
            "click",
            serde_json::json!({
                "pid": harness.pid, "window_id": window, "element_token": old_token,
                "delivery_mode": "foreground"
            }),
        );
        std::fs::write(directory.path().join("command"), "checkpoint").unwrap();
        let after_old = fixture_state(directory.path(), |state| state["checkpoint"] == true);
        capture.assert_writer_pending();
        std::fs::write(
            output.join("snapshot-old-action.json"),
            serde_json::to_vec_pretty(&attempted.raw).unwrap(),
        )
        .unwrap();
        std::fs::write(
            output.join("snapshot-after-old.json"),
            serde_json::to_vec_pretty(&after_old).unwrap(),
        )
        .unwrap();
        let second = capture.finish(&output.join("snapshot-pending.png"));
        assert!(
            !second.is_error(),
            "replacement native snapshot: {}",
            second.text()
        );
        assert_ne!(first.snapshot_id(), second.snapshot_id());
        assert_eq!(
            element_index_by_id(second.tree_text(), "snapshot-replacement"),
            Some(original_index),
            "fixture must replace the exact index addressed by the old token"
        );
        let fresh = element_token_by_id(&second, "snapshot-replacement");
        let recovered = driver.call(
            "click",
            serde_json::json!({
                "pid": harness.pid, "window_id": window, "element_token": fresh,
                "delivery_mode": "foreground"
            }),
        );
        assert!(
            !recovered.is_error(),
            "fresh token recovery: {}",
            recovered.text()
        );
        let before_count = after_old["replacement_clicks"].as_u64().unwrap();
        let after_fresh = fixture_state(directory.path(), |state| {
            state["replacement_clicks"].as_u64() == Some(before_count + 1)
        });
        std::fs::write(
            output.join("snapshot-after-fresh.json"),
            serde_json::to_vec_pretty(&after_fresh).unwrap(),
        )
        .unwrap();
        assert_eq!(
            before_count,
            0,
            "old token activated replacement control during built-in capture; old response: {}",
            attempted.text()
        );
        Observation::delivered(vec![OracleKind::FixtureState], Evidence::default())
    });
}
