//! Pipe-only subprocess fixtures. Never initialize a driver, AppKit, or a desktop.
use super::*;
use std::sync::mpsc;
use std::time::Instant;

const BUDGET: Duration = Duration::from_millis(100);
const LIMIT: Duration = Duration::from_millis(400);

// Re-execute only this test, not a real private worker. The parent consumes the
// libtest preamble and READY before handing the pipes to the production client.
#[test]
fn fixture() {
    let Ok(mode) = std::env::var("CUA_SHUTDOWN_TEST_FIXTURE") else {
        return;
    };
    println!("READY");
    std::io::stdout().flush().unwrap();
    if mode == "unread" {
        std::thread::sleep(Duration::from_secs(3));
        std::process::exit(0);
    }
    let mut input = std::io::stdin().lock();
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).unwrap() == 0 {
            std::process::exit(0);
        }
        let request: ChannelRequest = serde_json::from_str(&line).unwrap();
        if let Ok(path) = std::env::var("CUA_SHUTDOWN_TEST_REQUESTS") {
            use std::fs::OpenOptions;
            writeln!(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .unwrap(),
                "{}",
                request.operation
            )
            .unwrap();
        }
        if mode == "partial" {
            print!("{{\"protocol_version\":");
            std::io::stdout().flush().unwrap();
        } else if mode == "ack" || mode == "graceful" || mode == "delayed_ack" {
            if mode == "delayed_ack" {
                std::thread::sleep(Duration::from_millis(450));
            }
            let response = ChannelResponse::ok(request.request_id, request.generation, Value::Null);
            println!("{}", serde_json::to_string(&response).unwrap());
            std::io::stdout().flush().unwrap();
            if mode == "graceful" {
                std::process::exit(0);
            }
        }
        // Finite even against the old implementation: red tests must not hang.
        std::thread::sleep(Duration::from_millis(900));
        std::process::exit(0);
    }
}

fn fixture_client(mode: &str, requests: &std::path::Path) -> Arc<PrivateWorkerClient> {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "worker::shutdown_tests::fixture", "--nocapture"])
        .env("CUA_SHUTDOWN_TEST_FIXTURE", mode)
        .env("CUA_SHUTDOWN_TEST_REQUESTS", requests)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    // Bound readiness with an owned reader thread; failed readiness kills and
    // reaps this exact child before joining the reader.
    let (tx, rx) = mpsc::sync_channel(1);
    let ready = std::thread::spawn(move || loop {
        let mut line = String::new();
        if stdout.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.trim() == "READY" {
            tx.send(stdout).unwrap();
            return;
        }
    });
    let stdout = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(stdout) => stdout,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            ready.join().unwrap();
            panic!("fixture readiness failed: {error}");
        }
    };
    ready.join().unwrap();
    let child = Arc::new(Mutex::new(child));
    Arc::new(PrivateWorkerClient {
        generation: "shutdown-fixture".into(),
        process: Mutex::new(WorkerProcess {
            child: Arc::clone(&child),
            stdin: Some(stdin),
            stdout: Some(stdout),
            next_request_id: 1,
            stopped: false,
        }),
        child,
        shutdown_started: AtomicBool::new(false),
        cleanup_started: AtomicBool::new(false),
        shutdown_timeout: BUDGET,
    })
}

fn runtime() -> tokio::runtime::Runtime {
    runtime_with_time(true)
}

fn runtime_with_time(enable_time: bool) -> tokio::runtime::Runtime {
    let mut builder = tokio::runtime::Builder::new_current_thread();
    builder.max_blocking_threads(1);
    if enable_time {
        builder.enable_time();
    }
    builder.build().unwrap()
}

fn assert_interrupted(result: Result<(), DriverError>, completion: ActionCompletion) {
    assert!(
        matches!(result, Err(DriverError::ActionInterrupted { completion: actual, .. }) if actual == completion),
        "expected {completion:?}, got {result:?}"
    );
}

fn assert_reaped(client: &PrivateWorkerClient) -> std::process::ExitStatus {
    assert_reaped_by(client, Instant::now() + Duration::from_secs(2))
}

fn assert_reaped_by(client: &PrivateWorkerClient, deadline: Instant) -> std::process::ExitStatus {
    // On Unix, try_wait itself can reap a zombie and mask a missing production
    // wait. First observe disappearance without reaping, keeping the client
    // alive so its Drop cannot repair cleanup either. Only then read the cached
    // exit status. Windows has no corresponding zombie/reaping requirement.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let pid = loop {
            if let Ok(child) = client.child.try_lock() {
                break child.id();
            }
            assert!(Instant::now() < deadline, "child owner stayed locked");
            std::thread::sleep(Duration::from_millis(5));
        };
        loop {
            let output = Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .unwrap();
            if output.stdout.is_empty()
                && output.stderr.is_empty()
                && output.status.code() == Some(1)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "owned child was not reaped: pid={pid}, state={:?}, stderr={:?}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    loop {
        if let Ok(mut child) = client.child.try_lock() {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
        }
        assert!(Instant::now() < deadline, "owned child was not reaped");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn shutdown_deadline_bounds_missing_reply() {
    let dir = tempfile::tempdir().unwrap();
    let client = fixture_client("silent", &dir.path().join("requests"));
    let start = Instant::now();
    let result = runtime().block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_reaped(&client);
    assert!(elapsed < LIMIT, "shutdown took {elapsed:?}");
    assert_interrupted(result, ActionCompletion::Unknown);
}

#[test]
fn shutdown_deadline_bounds_partial_reply() {
    let dir = tempfile::tempdir().unwrap();
    let client = fixture_client("partial", &dir.path().join("requests"));
    let start = Instant::now();
    let result = runtime().block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_reaped(&client);
    assert!(elapsed < LIMIT, "partial reply took {elapsed:?}");
    assert_interrupted(result, ActionCompletion::Unknown);
}

#[test]
fn shutdown_deadline_does_not_report_ack_as_exit() {
    let dir = tempfile::tempdir().unwrap();
    let client = fixture_client("ack", &dir.path().join("requests"));
    let start = Instant::now();
    let result = runtime().block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert!(
        !assert_reaped(&client).success(),
        "fixture exited without force"
    );
    assert!(elapsed < LIMIT, "ack without exit took {elapsed:?}");
    assert_interrupted(result, ActionCompletion::Unknown);
}

#[test]
fn shutdown_deadline_is_not_restarted_after_acknowledgment() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = fixture_client("delayed_ack", &dir.path().join("requests"));
    Arc::get_mut(&mut client).unwrap().shutdown_timeout = Duration::from_millis(600);
    let rt = runtime();
    let start = Instant::now();
    let result = rt.block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_reaped(&client);
    assert!(
        elapsed < Duration::from_millis(900),
        "exit received a fresh budget: {elapsed:?}"
    );
    assert_interrupted(result, ActionCompletion::Unknown);
}

#[test]
fn shutdown_deadline_allows_graceful_exit() {
    shutdown_deadline_allows_graceful_exit_on(runtime());
}

#[test]
fn timerless_shutdown_deadline_allows_graceful_exit() {
    shutdown_deadline_allows_graceful_exit_on(runtime_with_time(false));
}

fn shutdown_deadline_allows_graceful_exit_on(rt: tokio::runtime::Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let client = fixture_client("graceful", &dir.path().join("requests"));
    rt.block_on(client.shutdown()).unwrap();
    assert!(assert_reaped(&client).success());
}

#[test]
fn shutdown_deadline_includes_blocking_pool_queue_and_never_sends_late() {
    shutdown_deadline_includes_blocking_pool_queue_and_never_sends_late_on(runtime());
}

#[test]
fn timerless_shutdown_deadline_includes_blocking_pool_queue_and_never_sends_late() {
    shutdown_deadline_includes_blocking_pool_queue_and_never_sends_late_on(runtime_with_time(
        false,
    ));
}

fn shutdown_deadline_includes_blocking_pool_queue_and_never_sends_late_on(
    rt: tokio::runtime::Runtime,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("requests");
    let client = fixture_client("graceful", &path);
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let blocker = rt.spawn_blocking(move || {
        entered_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(700));
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    let result = rt.block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_reaped_by(&client, start + LIMIT);
    assert!(!client.is_available());
    rt.block_on(blocker).unwrap();
    // Drain queued shutdown work before checking its observable side effect.
    rt.block_on(rt.spawn_blocking(|| ())).unwrap();
    assert_reaped(&client);
    assert!(elapsed < LIMIT, "queue time was excluded: {elapsed:?}");
    assert_interrupted(result, ActionCompletion::NotStarted);
    assert!(!path.exists(), "expired queued shutdown was transmitted");
}

#[test]
fn shutdown_deadline_includes_process_lock_contention() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("requests");
    let client = fixture_client("graceful", &path);
    let (tx, rx) = mpsc::sync_channel(1);
    let other = client.clone();
    let holder = std::thread::spawn(move || {
        let _lock = other.process.lock().unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(700));
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let rt = runtime();
    let start = Instant::now();
    let result = rt.block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_reaped_by(&client, start + LIMIT);
    assert!(!client.is_available());
    holder.join().unwrap();
    assert_reaped(&client);
    assert!(elapsed < LIMIT, "lock time was excluded: {elapsed:?}");
    assert_interrupted(result, ActionCompletion::NotStarted);
    assert!(
        !path.exists(),
        "expired shutdown was transmitted after lock release"
    );
}

#[test]
fn shutdown_deadline_interrupts_unread_stdin_without_killing_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let client = fixture_client("unread", &dir.path().join("requests"));
    let sibling = fixture_client("graceful", &dir.path().join("sibling"));
    let other = client.clone();
    let request = std::thread::spawn(move || {
        other.request_sync(
            "call",
            None,
            Some(Value::String("x".repeat(4 * 1024 * 1024))),
            None,
        )
    });
    // Wait until the in-flight request holds the transport lock while its large
    // write fills the anonymous pipe. This fixture never reads stdin.
    let deadline = Instant::now() + Duration::from_secs(2);
    while client.process.try_lock().is_ok() {
        assert!(
            Instant::now() < deadline,
            "request never acquired transport lock"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let rt = runtime();
    let start = Instant::now();
    let result = rt.block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_interrupted(
        request.join().unwrap().map(|_| ()),
        ActionCompletion::Unknown,
    );
    assert_reaped(&client);
    assert!(
        sibling.is_available(),
        "shutdown killed an unrelated owned sibling"
    );
    rt.block_on(sibling.shutdown()).unwrap();
    assert!(
        elapsed < LIMIT,
        "unread stdin excluded from budget: {elapsed:?}"
    );
    assert_interrupted(result, ActionCompletion::NotStarted);
}

#[test]
fn shutdown_deadline_bounds_its_own_blocked_write() {
    shutdown_deadline_bounds_its_own_blocked_write_on(runtime());
}

#[test]
fn timerless_shutdown_deadline_bounds_its_own_blocked_write() {
    shutdown_deadline_bounds_its_own_blocked_write_on(runtime_with_time(false));
}

fn shutdown_deadline_bounds_its_own_blocked_write_on(rt: tokio::runtime::Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let mut client = fixture_client("unread", &dir.path().join("requests"));
    // Exercise the shutdown write itself without changing the public protocol:
    // the fixture intentionally accepts an oversized generation for this test.
    Arc::get_mut(&mut client).unwrap().generation = "x".repeat(256 * 1024);
    let start = Instant::now();
    let result = rt.block_on(client.shutdown());
    let elapsed = start.elapsed();
    assert_reaped(&client);
    assert!(
        elapsed < LIMIT,
        "shutdown write exceeded budget: {elapsed:?}"
    );
    assert_interrupted(result, ActionCompletion::Unknown);
}

#[test]
fn shutdown_deadline_cancellation_releases_watchdog() {
    let ShutdownDeadline {
        _cancel: cancel,
        mut expired,
    } = ShutdownDeadline::new(Instant::now() + Duration::from_secs(60)).unwrap();
    drop(cancel);
    let deadline = Instant::now() + LIMIT;
    loop {
        match expired.try_recv() {
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => break,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Ok(()) => panic!("cancelled watchdog reported expiry"),
        }
        assert!(
            Instant::now() < deadline,
            "cancelled watchdog kept sleeping"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn timerless_shutdown_keeps_executor_responsive() {
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    let dir = tempfile::tempdir().unwrap();
    let mut client = fixture_client("silent", &dir.path().join("requests"));
    Arc::get_mut(&mut client).unwrap().shutdown_timeout = Duration::from_millis(600);
    let rt = runtime_with_time(false);
    let result = rt.block_on(async {
        let mut shutdown = Box::pin(client.shutdown());
        let start = Instant::now();
        poll_fn(|cx| {
            assert!(shutdown.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        // This code shares the only executor thread with shutdown and must run
        // before the 600ms deadline, not after a synchronous timer wait.
        assert!(start.elapsed() < LIMIT, "shutdown blocked the executor");
        shutdown.await
    });
    assert_interrupted(result, ActionCompletion::Unknown);
    assert_reaped(&client);
}

#[test]
fn shutdown_deadline_concurrent_calls_send_at_most_one_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("requests");
    let client = fixture_client("ack", &path);
    let rt = runtime();
    let start = Instant::now();
    let (first, second) = rt.block_on(async { tokio::join!(client.shutdown(), client.shutdown()) });
    let elapsed = start.elapsed();
    assert_reaped(&client);
    assert!(elapsed < LIMIT, "concurrent shutdown took {elapsed:?}");
    assert_interrupted(first, ActionCompletion::Unknown);
    assert!(matches!(second, Err(DriverError::Shutdown)));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "shutdown\n");
}
