//! MCP transport: one long-lived `cua-driver` server over stdio JSON-RPC.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::daemon::TestDaemon;
use crate::driver::{BehaviorRecording, Driver};
use crate::paths::driver_binary;
use crate::reaper::{spawn_in_job, ChildReaper};
use crate::response::ToolResponse;
use crate::CALL_TIMEOUT;

/// A spawned cua-driver MCP server. State (e.g. `set_config`) persists for the
/// lifetime of this one connection — which is why config-scope tests drive a
/// single `McpDriver`. The server (and any apps spawned through [`reaper`]) are
/// reaped when this value drops.
///
/// [`reaper`]: McpDriver::reaper
pub struct McpDriver {
    reaper: ChildReaper,
    _daemon: Option<TestDaemon>,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u32,
    recording_dir: Option<PathBuf>,
    recording_started: bool,
}

static RECORDING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

impl McpDriver {
    /// Spawn the driver, start the stdout reader thread, and `initialize`.
    /// Returns `None` (with a skip message) if the binary isn't built — the
    /// caller's test should early-return so an un-built binary skips, not fails.
    pub fn spawn() -> Option<Self> {
        Self::spawn_internal(&[], &[], None)
    }

    /// Spawn the driver with a stable recording label for artifact naming.
    pub fn spawn_named(recording_label: &str) -> Option<Self> {
        Self::spawn_internal(&[], &[], Some(recording_label))
    }

    /// Spawn a named driver with environment variables scoped to this child.
    pub fn spawn_named_with_env(recording_label: &str, env: &[(&str, &str)]) -> Option<Self> {
        Self::spawn_internal(env, &[], Some(recording_label))
    }

    /// Spawn the driver with extra environment variables set on the child.
    pub fn spawn_with_env(env: &[(&str, &str)]) -> Option<Self> {
        Self::spawn_internal(env, &[], None)
    }

    fn spawn_internal(
        env: &[(&str, &str)],
        args: &[&str],
        recording_label: Option<&str>,
    ) -> Option<Self> {
        let bin = driver_binary();
        if !bin.exists() {
            eprintln!("[testkit] driver binary not built at {bin:?} — skipping");
            return None;
        }

        let mut reaper = ChildReaper::new();
        let daemon = if args.is_empty() {
            // Environment that changes tool behavior belongs on the daemon,
            // because the stdio process is now only a transport proxy.
            Some(TestDaemon::spawn(&bin, &mut reaper, env)?)
        } else {
            None
        };
        let mut cmd = Command::new(&bin);
        let stderr = if std::env::var_os("CUA_TEST_DRIVER_STDERR").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            // Product telemetry is default-on, but behavior harnesses should
            // remain deterministic and must not send CI traffic to PostHog.
            // A test that exercises telemetry can explicitly override this
            // through `spawn_with_env` / `spawn_named_with_env` below.
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false");
        if let Some(daemon) = &daemon {
            cmd.args(["mcp", "--socket", &daemon.socket]);
        } else {
            cmd.args(args);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut driver = spawn_in_job(&mut cmd)
            .inspect_err(|e| eprintln!("[testkit] driver spawn failed: {e}"))
            .ok()?;
        let stdin = driver.stdin.take().unwrap();
        let stdout = driver.stdout.take().unwrap();
        reaper.push(driver);

        let (tx, rx) = channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.trim().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut d = McpDriver {
            reaper,
            _daemon: daemon,
            stdin,
            rx,
            next_id: 2,
            recording_dir: None,
            recording_started: false,
        };
        d.initialize();
        d.prepare_e2e_recording(recording_label);
        Some(d)
    }

    /// macOS GUI harness tests need the installed, TCC-authorized daemon path.
    ///
    /// A raw `target/debug/cua-driver` MCP process initializes AppKit for the
    /// cursor overlay and can lose Screen Recording-attributed window titles even
    /// when the shell's one-shot `cua-driver call` path can see them. The installed
    /// daemon is the product path agents use, so ignored GUI tests proxy through an
    /// already-running daemon and skip with a clear note when it is absent.
    #[cfg(target_os = "macos")]
    pub fn spawn_macos_daemon_proxy() -> Option<Self> {
        Self::spawn_macos_daemon_proxy_internal(None)
    }

    /// macOS daemon-proxy variant with a stable recording artifact label.
    #[cfg(target_os = "macos")]
    pub fn spawn_macos_daemon_proxy_named(recording_label: &str) -> Option<Self> {
        Self::spawn_macos_daemon_proxy_internal(Some(recording_label))
    }

    #[cfg(target_os = "macos")]
    fn spawn_macos_daemon_proxy_internal(recording_label: Option<&str>) -> Option<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let socket = std::env::var("CUA_E2E_MACOS_DAEMON_SOCKET")
            .unwrap_or_else(|_| format!("{home}/Library/Caches/cua-driver/cua-driver.sock"));
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            eprintln!(
                "[testkit] CuaDriver daemon not listening at {socket} — \
                 run `./scripts/install-local.sh` and `open -n -g -a CuaDriver --args serve`; skipping"
            );
            return None;
        }
        Self::spawn_internal(&[], &["mcp", "--socket", &socket], recording_label)
    }

    fn initialize(&mut self) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
        }));
        let _ = self.rx.recv_timeout(CALL_TIMEOUT);
    }

    fn send(&mut self, req: Value) {
        let _ = writeln!(self.stdin, "{}", serde_json::to_string(&req).unwrap());
        let _ = self.stdin.flush();
    }

    fn prepare_e2e_recording(&mut self, explicit_label: Option<&str>) {
        let Some(root) = std::env::var_os("CUA_E2E_RECORDINGS_ROOT") else {
            return;
        };
        let sequence = RECORDING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("unnamed-test")
            .to_owned();
        let label = recording_label(explicit_label.unwrap_or(&thread_name));
        let output_dir =
            PathBuf::from(root).join(format!("{label}-pid{}-{sequence:03}", std::process::id()));
        std::fs::create_dir_all(&output_dir).unwrap_or_else(|error| {
            panic!("could not prepare E2E recording directory for {thread_name}: {error}")
        });
        let runner_console = crate::windows_setup::minimize_hosted_runner_console();
        let runner_console_status = runner_console.as_deref().unwrap_or("error");
        let manifest = serde_json::json!({
            "schema": "cua-e2e-trajectory/v1",
            "label": label,
            "rust_test_thread": thread_name,
            "process_id": std::process::id(),
            "sequence": sequence,
            "behavior_video": {
                "status": "pending",
                "baseline_settle_ms": 300
            },
            "hosted_runner_console": {
                "status": runner_console_status
            }
        });
        std::fs::write(
            output_dir.join("trajectory.json"),
            serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
        )
        .expect("write prepared E2E trajectory manifest");
        eprintln!(
            "[testkit] prepared E2E evidence directory {}",
            output_dir.display()
        );
        self.recording_dir = Some(output_dir);
        if let Err(error) = runner_console {
            panic!("hosted-runner console cleanup failed before fixture setup: {error}");
        }
    }

    /// Begin the behavioral clip after fixture readiness and foreground or
    /// background posture have been established. The settle interval gives
    /// the video backend time to encode a visible baseline before dispatch.
    pub fn start_behavior_recording(&mut self) {
        let Some(output_dir) = self.recording_dir.clone() else {
            return;
        };
        if self.recording_started {
            return;
        }
        let response = self.call(
            "start_recording",
            serde_json::json!({
                "output_dir": output_dir.to_string_lossy(),
                "record_video": true
            }),
        );
        let video_active = response.structured()["video_active"]
            .as_bool()
            .unwrap_or(false);
        if response.is_error() || !video_active {
            update_behavior_video_status(&output_dir, "error");
            panic!(
                "E2E behavioral video did not start after setup: {}; structured={}",
                response.text(),
                response.structured()
            );
        }
        self.recording_started = true;
        update_behavior_video_status(&output_dir, "started");
        std::thread::sleep(Duration::from_millis(300));
        mark_behavior_video_baseline_ready(&output_dir);
        eprintln!(
            "[testkit] behavioral E2E video started at {}",
            output_dir.display()
        );
    }

    fn stop_e2e_recording(&mut self) {
        let Some(output_dir) = self.recording_dir.take() else {
            return;
        };
        if !self.recording_started {
            let message = "E2E behavioral video boundary was never reached; fixture setup or posture failed before capture";
            eprintln!("[testkit] {message}");
            let _ = std::fs::write(output_dir.join("recording-error.txt"), message);
            return;
        }
        let response = self.call("stop_recording", serde_json::json!({}));
        let video_path = output_dir.join("recording.mp4");
        let valid_video = !response.is_error()
            && response.structured()["last_video_path"].as_str().is_some()
            && std::fs::metadata(&video_path)
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false);
        if valid_video {
            update_behavior_video_status(&output_dir, "finalized");
            eprintln!("[testkit] finalized E2E video at {}", video_path.display());
            return;
        }

        let message = format!(
            "E2E video failed to finalize at {}: {}",
            video_path.display(),
            response.text()
        );
        eprintln!("[testkit] {message}");
        update_behavior_video_status(&output_dir, "error");
        let _ = std::fs::create_dir_all(&output_dir);
        let _ = std::fs::write(output_dir.join("recording-error.txt"), message);
    }

    /// Mutable access to the child reaper, e.g. to launch a target app whose
    /// lifetime should be tied to this driver.
    pub fn reaper(&mut self) -> &mut ChildReaper {
        &mut self.reaper
    }

    /// Directory for this driver's active per-cell recording, when enabled.
    pub fn recording_dir(&self) -> Option<&std::path::Path> {
        self.recording_dir.as_deref()
    }

    /// Poll `list_windows` until a window of `pid` whose title contains
    /// `title_substr` appears (up to ~12s). Returns `(window_id, title)`.
    /// Replaces the per-file `find_harness_window` helper.
    pub fn find_window(&mut self, pid: i64, title_substr: &str) -> Option<(u64, String)> {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let r = self.call("list_windows", serde_json::json!({ "pid": pid }));
            if let Some(wins) = r.structured()["windows"].as_array() {
                for w in wins {
                    if w["pid"].as_i64() != Some(pid) {
                        continue;
                    }
                    let title = w["title"].as_str().unwrap_or("");
                    if title.contains(title_substr) {
                        return Some((w["window_id"].as_u64()?, title.to_string()));
                    }
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Raw JSON-RPC response envelope, for the rare assertion needing it.
    pub fn call_raw(&mut self, tool: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        }));
        match self.rx.recv_timeout(CALL_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|_| serde_json::json!({ "error": format!("bad json: {line}") })),
            Err(_) => serde_json::json!({
                "error": format!("TIMEOUT (>{}s) on {tool}", CALL_TIMEOUT.as_secs())
            }),
        }
    }
}

impl Drop for McpDriver {
    fn drop(&mut self) {
        self.stop_e2e_recording();
    }
}

fn recording_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() {
        "unnamed-test".to_owned()
    } else {
        label.chars().take(96).collect()
    }
}

impl Driver for McpDriver {
    fn call(&mut self, tool: &str, args: Value) -> ToolResponse {
        ToolResponse::from_mcp(self.call_raw(tool, args))
    }
}

impl BehaviorRecording for McpDriver {
    fn start_behavior_recording(&mut self) {
        McpDriver::start_behavior_recording(self);
    }
}

fn update_behavior_video_status(output_dir: &std::path::Path, status: &str) {
    let path = output_dir.join("trajectory.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut manifest) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    manifest["behavior_video"]["status"] = Value::String(status.to_owned());
    let timestamp_field = match status {
        "started" => Some("started_at_unix_ms"),
        "finalized" => Some("finalized_at_unix_ms"),
        "error" => Some("error_at_unix_ms"),
        _ => None,
    };
    if let Some(field) = timestamp_field {
        manifest["behavior_video"][field] = Value::from(unix_ms());
    }
    let _ = std::fs::write(
        path,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    );
}

fn mark_behavior_video_baseline_ready(output_dir: &std::path::Path) {
    let path = output_dir.join("trajectory.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut manifest) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    manifest["behavior_video"]["baseline_ready_at_unix_ms"] = Value::from(unix_ms());
    let _ = std::fs::write(
        path,
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    );
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::recording_label;

    #[test]
    fn recording_label_is_artifact_safe() {
        assert_eq!(
            recording_label("module::test name/windows"),
            "module--test-name-windows"
        );
        assert_eq!(recording_label("///"), "unnamed-test");
    }
}
