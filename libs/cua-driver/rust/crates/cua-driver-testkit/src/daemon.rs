//! Per-test daemon lifecycle for CLI and MCP transport fixtures.

use std::path::Path;
use std::process::{Command, Stdio};
#[cfg(not(unix))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::reaper::{spawn_in_job, ChildReaper};

#[cfg(not(unix))]
static DAEMON_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Keeps the temporary socket directory alive for the daemon lifetime.
pub(crate) struct TestDaemon {
    pub(crate) socket: String,
    pub(crate) pid: u32,
    #[cfg(unix)]
    _socket_dir: tempfile::TempDir,
}

impl TestDaemon {
    pub(crate) fn spawn(
        binary: &Path,
        reaper: &mut ChildReaper,
        env: &[(&str, &str)],
    ) -> Option<Self> {
        Self::spawn_configured(binary, reaper, env, false)
    }

    pub(crate) fn spawn_with_overlay(
        binary: &Path,
        reaper: &mut ChildReaper,
        env: &[(&str, &str)],
    ) -> Option<Self> {
        Self::spawn_configured(binary, reaper, env, true)
    }

    fn spawn_configured(
        binary: &Path,
        reaper: &mut ChildReaper,
        env: &[(&str, &str)],
        overlay_enabled: bool,
    ) -> Option<Self> {
        #[cfg(not(unix))]
        let sequence = DAEMON_SEQUENCE.fetch_add(1, Ordering::Relaxed);

        #[cfg(unix)]
        let (socket, socket_dir) = {
            // Unix-domain socket paths are short on macOS, so keep the test
            // directory directly under /tmp with a compact filename.
            let dir = tempfile::Builder::new()
                .prefix("cua-")
                .tempdir_in("/tmp")
                .inspect_err(|error| {
                    eprintln!("[testkit] create daemon socket directory failed: {error}")
                })
                .ok()?;
            (dir.path().join("d.sock").display().to_string(), dir)
        };

        #[cfg(target_os = "windows")]
        let socket = format!(
            r"\\.\pipe\cua-driver-test-{}-{sequence}",
            std::process::id()
        );

        #[cfg(not(any(unix, target_os = "windows")))]
        let socket = format!("cua-driver-test-{}-{sequence}", std::process::id());

        let stderr = if std::env::var_os("CUA_TEST_DRIVER_STDERR").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };
        let mut command = Command::new(binary);
        command
            .args(["serve", "--socket", &socket, "--no-permissions-gate"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false");
        if !overlay_enabled {
            command.arg("--no-overlay");
        }
        for (key, value) in env {
            command.env(key, value);
        }
        // Spawn directly so the `Child` stays reachable for `try_wait` below;
        // the reaper adopts it on every exit path. Without the handle, a daemon
        // that dies during startup is indistinguishable from one that is merely
        // slow, which is what made #2480 undiagnosable from CI logs alone.
        let mut child = spawn_in_job(&mut command)
            .inspect_err(|error| eprintln!("[testkit] daemon spawn failed: {error}"))
            .ok()?;
        let pid = child.id();

        let deadline = Instant::now() + READINESS_WINDOW;
        // Keep the furthest point any probe reached. A late attempt can regress
        // (the pipe is torn down as the daemon exits), and the furthest outcome
        // is the one that explains where startup actually stopped.
        let mut furthest = ProbeOutcome::PipeUnavailable;
        let mut exit_status = None;
        while Instant::now() < deadline {
            // A daemon that has already exited will never become ready; report
            // its status now instead of burning the rest of the window.
            if let Ok(Some(status)) = child.try_wait() {
                exit_status = Some(status);
                break;
            }
            let outcome = daemon_is_listening(binary, &socket);
            furthest = furthest.max(outcome);
            if outcome == ProbeOutcome::Ready {
                reaper.push(child);
                return Some(Self {
                    socket,
                    pid,
                    #[cfg(unix)]
                    _socket_dir: socket_dir,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        match exit_status {
            Some(status) => eprintln!(
                "[testkit] daemon exited before becoming ready on {socket}: {status}. \
                 Furthest readiness probe: {}. Set CUA_TEST_DRIVER_STDERR=1 to inherit \
                 daemon stderr.",
                furthest.describe()
            ),
            None => eprintln!(
                "[testkit] daemon did not become ready on {socket} within {}s (process still \
                 running). Furthest readiness probe: {}. Set CUA_TEST_DRIVER_STDERR=1 to \
                 inherit daemon stderr.",
                READINESS_WINDOW.as_secs(),
                furthest.describe()
            ),
        }
        reaper.push(child);
        None
    }
}

/// Total time the daemon has to answer a readiness probe.
const READINESS_WINDOW: Duration = Duration::from_secs(10);

/// Per-attempt bound on a single readiness probe.
///
/// A Windows named-pipe read has no timeout, so a server that accepts the
/// connection but never answers blocks forever. The outer deadline is only
/// consulted *between* attempts, so without an independent per-attempt bound
/// one stalled connection consumes the entire readiness window and the failure
/// surfaces as a bare "did not become ready" (#2480).
#[cfg(target_os = "windows")]
const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);

/// How far a single readiness probe progressed.
///
/// Ordered by progress so `max` across the window keeps the most informative
/// outcome: knowing the pipe was reachable but never answered points at daemon
/// startup, whereas never opening the pipe points at spawn or naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProbeOutcome {
    /// The pipe/socket could not be opened at all.
    PipeUnavailable,
    /// Opened, but the request could not be written.
    WriteFailed,
    /// Request written, but no response line arrived before the attempt bound.
    NoResponse,
    /// A response arrived but was not valid JSON.
    MalformedResponse,
    /// Valid JSON, but not the `ok: true` the protocol promises.
    NotOk,
    /// Fully serviced — the daemon is ready.
    Ready,
}

impl ProbeOutcome {
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::PipeUnavailable => {
                "pipe never became connectable (daemon had not created it, or the name differs)"
            }
            Self::WriteFailed => "pipe opened but the request could not be written",
            Self::NoResponse => {
                "request written but the daemon sent no response line before the per-attempt \
                 timeout (accepted-but-stalled connection)"
            }
            Self::MalformedResponse => "daemon replied but the response was not valid JSON",
            Self::NotOk => "daemon replied with valid JSON but without ok:true",
            Self::Ready => "ready",
        }
    }
}

#[cfg(unix)]
fn daemon_is_listening(_binary: &Path, socket: &str) -> ProbeOutcome {
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(_) => ProbeOutcome::Ready,
        Err(_) => ProbeOutcome::PipeUnavailable,
    }
}

/// Run one named-pipe probe under an independent timeout.
///
/// The probe runs on a worker thread because the blocking read it performs has
/// no native timeout. If the attempt bound expires the thread is abandoned: it
/// is unblocked when the daemon is reaped at teardown (the kill-on-close job
/// object closes the pipe), and the harness is short-lived, so leaking a parked
/// thread is preferable to hanging the whole readiness window.
#[cfg(target_os = "windows")]
fn daemon_is_listening(_binary: &Path, socket: &str) -> ProbeOutcome {
    probe_pipe_with_timeout(socket, PROBE_ATTEMPT_TIMEOUT)
}

#[cfg(target_os = "windows")]
fn probe_pipe_with_timeout(socket: &str, timeout: Duration) -> ProbeOutcome {
    use std::sync::mpsc;

    let socket = socket.to_string();
    let (tx, rx) = mpsc::channel();
    if std::thread::Builder::new()
        .name("cua-testkit-pipe-probe".into())
        .spawn(move || {
            let _ = tx.send(probe_pipe_once(&socket));
        })
        .is_err()
    {
        return ProbeOutcome::PipeUnavailable;
    }
    rx.recv_timeout(timeout).unwrap_or(ProbeOutcome::NoResponse)
}

#[cfg(target_os = "windows")]
fn probe_pipe_once(socket: &str) -> ProbeOutcome {
    use std::io::{BufRead, BufReader, Write};

    // Exercise the real named-pipe protocol instead of spawning `status`.
    // A finite CLI command is wrapped by the telemetry completion observer,
    // which makes it an unnecessarily heavy and timing-sensitive readiness
    // probe on hosted Windows runners. Completing `list` also proves that the
    // server has progressed past pipe creation and can service the connection.
    let Ok(pipe) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(socket)
    else {
        return ProbeOutcome::PipeUnavailable;
    };
    let Ok(mut writer) = pipe.try_clone() else {
        return ProbeOutcome::WriteFailed;
    };
    if writer
        .write_all(b"{\"method\":\"list\"}\n")
        .and_then(|()| writer.flush())
        .is_err()
    {
        return ProbeOutcome::WriteFailed;
    }

    let mut response = String::new();
    if BufReader::new(pipe).read_line(&mut response).is_err() {
        return ProbeOutcome::NoResponse;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&response) else {
        return ProbeOutcome::MalformedResponse;
    };
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        ProbeOutcome::Ready
    } else {
        ProbeOutcome::NotOk
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn daemon_is_listening(binary: &Path, socket: &str) -> ProbeOutcome {
    let ready = Command::new(binary)
        .args(["status", "--socket", socket])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if ready {
        ProbeOutcome::Ready
    } else {
        ProbeOutcome::PipeUnavailable
    }
}

#[cfg(test)]
mod readiness_probe_tests {
    use super::*;

    /// The furthest-progress ordering is what makes the expiry message useful,
    /// so pin it rather than leaving it to derive order by accident.
    #[test]
    fn outcomes_are_ordered_by_progress() {
        let ordered = [
            ProbeOutcome::PipeUnavailable,
            ProbeOutcome::WriteFailed,
            ProbeOutcome::NoResponse,
            ProbeOutcome::MalformedResponse,
            ProbeOutcome::NotOk,
            ProbeOutcome::Ready,
        ];
        for pair in ordered.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{:?} must rank below {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// The reported outcome must survive a late regression: as a daemon exits,
    /// its pipe stops opening, and reporting `PipeUnavailable` would point at
    /// spawn/naming when the real evidence was an accepted-but-stalled pipe.
    #[test]
    fn furthest_outcome_survives_a_late_regression() {
        let observed = [
            ProbeOutcome::PipeUnavailable,
            ProbeOutcome::NoResponse,
            ProbeOutcome::PipeUnavailable,
        ];
        let furthest = observed
            .iter()
            .copied()
            .fold(ProbeOutcome::PipeUnavailable, ProbeOutcome::max);
        assert_eq!(furthest, ProbeOutcome::NoResponse);
    }

    #[test]
    fn ready_outranks_every_failure() {
        for outcome in [
            ProbeOutcome::PipeUnavailable,
            ProbeOutcome::WriteFailed,
            ProbeOutcome::NoResponse,
            ProbeOutcome::MalformedResponse,
            ProbeOutcome::NotOk,
        ] {
            assert!(ProbeOutcome::Ready > outcome);
        }
    }

    /// Every outcome must explain itself; the whole point of #2480 is that the
    /// operator can tell a stall from a genuine startup failure.
    #[test]
    fn every_outcome_has_a_distinct_description() {
        let all = [
            ProbeOutcome::PipeUnavailable,
            ProbeOutcome::WriteFailed,
            ProbeOutcome::NoResponse,
            ProbeOutcome::MalformedResponse,
            ProbeOutcome::NotOk,
            ProbeOutcome::Ready,
        ];
        let mut seen = Vec::new();
        for outcome in all {
            let text = outcome.describe();
            assert!(!text.is_empty(), "{outcome:?} needs a description");
            assert!(!seen.contains(&text), "duplicate description: {text}");
            seen.push(text);
        }
    }

    /// The per-attempt bound must leave room for several attempts inside the
    /// window; if one attempt could span it, the flake in #2480 returns.
    #[cfg(target_os = "windows")]
    #[test]
    fn attempt_bound_permits_multiple_attempts_per_window() {
        assert!(
            PROBE_ATTEMPT_TIMEOUT * 4 <= READINESS_WINDOW,
            "one stalled attempt must not consume the readiness window"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn stalled_pipe_attempt_is_bounded_and_does_not_block_the_next_probe() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use std::sync::mpsc;
        use windows::core::HSTRING;
        use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, HANDLE};
        use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        fn socket_name(label: &str) -> String {
            let sequence = DAEMON_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            format!(
                r"\\.\pipe\cua-driver-test-{label}-{}-{sequence}",
                std::process::id()
            )
        }

        fn spawn_server(
            socket: String,
            response: Option<&'static [u8]>,
            release: Option<mpsc::Receiver<()>>,
        ) -> (
            mpsc::Receiver<()>,
            mpsc::Receiver<()>,
            std::thread::JoinHandle<()>,
        ) {
            let (ready_tx, ready_rx) = mpsc::channel();
            let (request_tx, request_rx) = mpsc::channel();
            let handle = std::thread::spawn(move || {
                let name = HSTRING::from(socket);
                let raw = unsafe {
                    CreateNamedPipeW(
                        &name,
                        PIPE_ACCESS_DUPLEX,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                        1,
                        4096,
                        4096,
                        0,
                        None,
                    )
                };
                assert!(!raw.is_invalid(), "CreateNamedPipeW failed");
                let mut pipe = unsafe { std::fs::File::from_raw_handle(raw.0) };
                ready_tx.send(()).unwrap();

                let raw = HANDLE(pipe.as_raw_handle());
                if let Err(error) = unsafe { ConnectNamedPipe(raw, None) } {
                    assert_eq!(
                        error.code(),
                        ERROR_PIPE_CONNECTED.to_hresult(),
                        "ConnectNamedPipe failed: {error}"
                    );
                }

                let mut request = String::new();
                BufReader::new(pipe.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                assert_eq!(request, "{\"method\":\"list\"}\n");
                request_tx.send(()).unwrap();

                if let Some(response) = response {
                    pipe.write_all(response).unwrap();
                    pipe.flush().unwrap();
                } else {
                    release
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .expect("stalled server was not released");
                }
            });
            (ready_rx, request_rx, handle)
        }

        let stalled_socket = socket_name("stalled");
        let (release_tx, release_rx) = mpsc::channel();
        let (stalled_ready, stalled_request, stalled_server) =
            spawn_server(stalled_socket.clone(), None, Some(release_rx));
        stalled_ready.recv_timeout(Duration::from_secs(2)).unwrap();

        let attempt_timeout = Duration::from_millis(250);
        let started = Instant::now();
        assert_eq!(
            probe_pipe_with_timeout(&stalled_socket, attempt_timeout),
            ProbeOutcome::NoResponse
        );
        assert!(
            started.elapsed() < attempt_timeout * 4,
            "stalled probe exceeded its attempt bound by too much: {:?}",
            started.elapsed()
        );
        stalled_request
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        // Leave the first probe worker blocked while proving a later probe can
        // still complete normally in the same harness process.
        let responsive_socket = socket_name("responsive");
        let (responsive_ready, responsive_request, responsive_server) =
            spawn_server(responsive_socket.clone(), Some(b"{\"ok\":true}\n"), None);
        responsive_ready
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            probe_pipe_with_timeout(&responsive_socket, Duration::from_secs(2)),
            ProbeOutcome::Ready
        );
        responsive_request
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        release_tx.send(()).unwrap();
        stalled_server.join().unwrap();
        responsive_server.join().unwrap();
    }
}
