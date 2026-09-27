// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Real stdio proxies and a real daemon, with selective control-channel loss.
//! Set CUA_PROXY_RECOVERY_IDLE_SECONDS=1200 for a long-idle diagnostic replay.
//! This injects a known transport failure; it does not reproduce an unknown trigger.

#![cfg(any(unix, target_os = "windows"))]

use cua_driver_testkit::spawn_in_job;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc as async_mpsc, oneshot};

const BOUND: Duration = Duration::from_secs(10);

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
type Connection = Box<dyn Stream>;

async fn copy_until_disconnect<A: Stream, B: Stream>(incoming: &mut A, upstream: &mut B) {
    let (mut client_read, mut client_write) = tokio::io::split(incoming);
    let (mut daemon_read, mut daemon_write) = tokio::io::split(upstream);
    // Named pipes cannot half-close: AsyncWrite::shutdown only flushes. Drop
    // both handles on either EOF so the relay preserves peer-disconnect semantics.
    tokio::select! {
        _ = tokio::io::copy(&mut client_read, &mut daemon_write) => {},
        _ = tokio::io::copy(&mut daemon_read, &mut client_write) => {},
    }
}

async fn connect(endpoint: &str) -> std::io::Result<Connection> {
    #[cfg(unix)]
    return Ok(Box::new(tokio::net::UnixStream::connect(endpoint).await?));
    #[cfg(target_os = "windows")]
    {
        let deadline = Instant::now() + BOUND;
        loop {
            match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
                Ok(pipe) => return Ok(Box::new(pipe)),
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    }
}

// Forward the handshake and all responses unchanged. Only a session_begin
// connection can be cut; per-call connections and the daemon stay alive.
async fn forward(
    incoming: Connection,
    daemon: String,
    controls: async_mpsc::UnboundedSender<oneshot::Sender<()>>,
) {
    let mut incoming = BufReader::new(incoming);
    let mut first = String::new();
    if !matches!(tokio::time::timeout(BOUND, incoming.read_line(&mut first)).await, Ok(Ok(n)) if n > 0)
    {
        return;
    }
    let request: Value = serde_json::from_str(&first).expect("relay request JSON");
    let mut upstream = connect(&daemon)
        .await
        .expect("connect relay to real daemon");
    upstream.write_all(first.as_bytes()).await.unwrap();
    if request["method"] == "session_begin" {
        let (cut, cut_rx) = oneshot::channel();
        controls.send(cut).unwrap();
        tokio::select! {
            _ = cut_rx => {},
            _ = copy_until_disconnect(&mut incoming, &mut upstream) => {},
        }
    } else {
        copy_until_disconnect(&mut incoming, &mut upstream).await;
    }
}

async fn relay(
    endpoint: &str,
    daemon: String,
    controls: async_mpsc::UnboundedSender<oneshot::Sender<()>>,
) -> tokio::task::JoinHandle<()> {
    #[cfg(unix)]
    {
        let listener = tokio::net::UnixListener::bind(endpoint).unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(forward(Box::new(stream), daemon.clone(), controls.clone()));
            }
        })
    }
    #[cfg(target_os = "windows")]
    {
        use tokio::net::windows::named_pipe::ServerOptions;
        let mut listener = ServerOptions::new()
            .first_pipe_instance(true)
            .create(endpoint)
            .unwrap();
        let endpoint = endpoint.to_owned();
        tokio::spawn(async move {
            loop {
                listener.connect().await.unwrap();
                let next = ServerOptions::new().create(&endpoint).unwrap();
                let stream = std::mem::replace(&mut listener, next);
                tokio::spawn(forward(Box::new(stream), daemon.clone(), controls.clone()));
            }
        })
    }
}

struct Process(Child, #[allow(dead_code)] tempfile::TempDir);

impl Process {
    fn spawn(args: &[&str]) -> Self {
        let driver_home = tempfile::tempdir().expect("isolated driver state");
        let idle_seconds = recovery_idle_seconds();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
            // This test isolates transport lifetime from intentional session expiry.
            .env(
                "CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS",
                (idle_seconds + 60).max(300).to_string(),
            )
            .env("HOME", driver_home.path())
            .env("USERPROFILE", driver_home.path())
            .env("LOCALAPPDATA", driver_home.path())
            .env("XDG_STATE_HOME", driver_home.path())
            .env("CUA_DRIVER_HOME", driver_home.path())
            .env("CUA_DRIVER_LOCAL_HOME", driver_home.path())
            .env("CUA_DRIVER_RS_HOME", driver_home.path());
        Self(
            spawn_in_job(&mut command).expect("spawn source-built driver"),
            driver_home,
        )
    }

    fn exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + BOUND;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "driver process did not exit within {BOUND:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn recovery_idle_seconds() -> u64 {
    let seconds = std::env::var("CUA_PROXY_RECOVERY_IDLE_SECONDS")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("idle seconds must be an integer")
        })
        .unwrap_or(1);
    assert!(
        (1..=3600).contains(&seconds),
        "idle seconds must be 1..=3600"
    );
    seconds
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Client {
    process: Process,
    stdin: Option<ChildStdin>,
    responses: Receiver<Value>,
    id: u64,
}

impl Client {
    fn spawn(endpoint: &str) -> Self {
        let mut process = Process::spawn(&["mcp", "--socket", endpoint]);
        let stdin = process.0.stdin.take();
        let stdout = process.0.stdout.take().unwrap();
        let (tx, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let response =
                    serde_json::from_str(&line.expect("read MCP stdout")).expect("MCP JSON");
                if tx.send(response).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            process,
            stdin,
            responses,
            id: 0,
        };
        let response = client.request(
            "initialize",
            json!({
                "protocolVersion":"2024-11-05", "capabilities":{},
                "clientInfo":{"name":"proxy-recovery-test","version":"1"}
            }),
        );
        assert!(response["result"]["serverInfo"].is_object(), "{response}");
        writeln!(
            client.stdin.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        writeln!(
            self.stdin.as_mut().unwrap(),
            "{}",
            json!({
                "jsonrpc":"2.0", "id":self.id, "method":method, "params":params
            })
        )
        .unwrap();
        let deadline = Instant::now() + BOUND;
        loop {
            let response = self
                .responses
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("bounded MCP response");
            if response["id"] == self.id {
                assert!(response.get("error").is_none(), "{response}");
                return response;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) {
        let inventory = self.request("tools/list", json!({}));
        let schema = inventory["result"]["tools"]
            .as_array()
            .expect("advertised tools")
            .iter()
            .find(|tool| tool["name"] == name)
            .and_then(|tool| tool.get("outputSchema"));
        if name == "get_session_state" {
            assert!(schema.is_some(), "session state must advertise a schema");
        }
        let response = self.request("tools/call", json!({"name":name,"arguments":arguments}));
        assert_ne!(response["result"]["isError"], true, "{response}");
        assert!(response["result"]["content"].is_array(), "{response}");
        if let Some(schema) = schema {
            let structured = response["result"]
                .get("structuredContent")
                .expect("schema-bearing success must contain structured output");
            assert!(jsonschema::is_valid(schema, structured), "{response}");
        }
    }

    fn eof(&self) {
        assert!(
            matches!(
                self.responses.recv_timeout(BOUND),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ),
            "MCP stdout must close promptly without additional responses"
        );
    }
}

async fn daemon_request(endpoint: &str, request: Value) -> Value {
    tokio::time::timeout(BOUND, async {
        let mut stream = connect(endpoint).await.unwrap();
        stream
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await.unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["ok"], true, "{response}");
        response["result"].clone()
    })
    .await
    .expect("bounded daemon request")
}

async fn session_absent(endpoint: &str, label: &str) {
    let deadline = Instant::now() + BOUND;
    loop {
        let result = daemon_request(endpoint, json!({"method":"sessions_list"})).await;
        let sessions = result["sessions"]
            .as_array()
            .expect("operator sessions array");
        if !sessions.iter().any(|session| session["session"] == label) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session {label} was not cleaned up: {result}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn session_active(endpoint: &str, label: &str) {
    let result = daemon_request(endpoint, json!({"method":"sessions_list"})).await;
    assert!(
        result["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["session"] == label && session["state"] == "active"),
        "expected active session {label}: {result}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_proxies_recover_from_control_loss_without_waiting_for_stdin() {
    #[cfg(unix)]
    let directory = tempfile::Builder::new()
        .prefix("cua-recovery-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(unix)]
    let (daemon_endpoint, proxy_endpoint) = (
        directory.path().join("d.sock").display().to_string(),
        directory.path().join("p.sock").display().to_string(),
    );
    #[cfg(target_os = "windows")]
    let (daemon_endpoint, proxy_endpoint) = (
        format!(r"\\.\pipe\cua-recovery-{}-daemon", std::process::id()),
        format!(r"\\.\pipe\cua-recovery-{}-proxy", std::process::id()),
    );
    let mut daemon = Process::spawn(&[
        "serve",
        "--socket",
        &daemon_endpoint,
        "--no-overlay",
        "--no-permissions-gate",
        "--dangerously-bypass-approvals",
    ]);
    let deadline = Instant::now() + BOUND;
    loop {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon exited during startup"
        );
        if connect(&daemon_endpoint).await.is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "daemon readiness timeout");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (control_tx, mut controls) = async_mpsc::unbounded_channel();
    let relay_task = relay(&proxy_endpoint, daemon_endpoint.clone(), control_tx).await;
    let mut idle = Client::spawn(&proxy_endpoint);
    let idle_control = tokio::time::timeout(BOUND, controls.recv())
        .await
        .unwrap()
        .unwrap();
    idle.call("start_session", json!({"session":"idle-client"}));
    let mut peer = Client::spawn(&proxy_endpoint);
    let _peer_control = tokio::time::timeout(BOUND, controls.recv())
        .await
        .unwrap()
        .unwrap();
    peer.call("start_session", json!({"session":"peer-client"}));
    session_active(&daemon_endpoint, "peer-client").await;
    session_active(&daemon_endpoint, "idle-client").await;

    let idle_seconds = recovery_idle_seconds();
    tokio::time::sleep(Duration::from_secs(idle_seconds)).await;
    drop(peer.stdin.take());
    assert!(
        peer.process.exit().success(),
        "peer stdin EOF should exit successfully"
    );
    peer.eof();
    session_absent(&daemon_endpoint, "peer-client").await;
    session_active(&daemon_endpoint, "idle-client").await;
    idle.call("get_config", json!({"session":"idle-client"}));
    idle.call("get_session_state", json!({"session":"idle-client"}));

    let mut survivor = Client::spawn(&proxy_endpoint);
    let _survivor_control = tokio::time::timeout(BOUND, controls.recv())
        .await
        .unwrap()
        .unwrap();
    survivor.call("start_session", json!({"session":"survivor"}));
    idle_control
        .send(())
        .expect("idle control must still be connected");
    assert!(idle.stdin.is_some(), "retain stdin during control loss");
    assert!(
        !idle.process.exit().success(),
        "lost control must fail the proxy"
    );
    idle.eof();
    session_absent(&daemon_endpoint, "idle-client").await;
    session_active(&daemon_endpoint, "survivor").await;
    survivor.call("get_config", json!({"session":"survivor"}));
    assert!(
        daemon.0.try_wait().unwrap().is_none(),
        "daemon must survive selective loss"
    );

    let mut fresh = Client::spawn(&proxy_endpoint);
    let _fresh_control = tokio::time::timeout(BOUND, controls.recv())
        .await
        .unwrap()
        .unwrap();
    fresh.call("get_config", json!({}));
    fresh.call("start_session", json!({"session":"fresh-client"}));
    fresh.call("get_session_state", json!({"session":"fresh-client"}));
    survivor.call("get_config", json!({"session":"survivor"}));
    relay_task.abort();
}
