// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Stdio MCP adapters for direct SDK-owned and service-owned runtimes.
//!
//! The client side always sees a normal stdio server. Depending on platform
//! and explicit launch options, this adapter either owns the SDK runtime
//! directly or forwards to a service that owns it.
//!
//! On macOS the CLI can ensure a daemon is running under `LaunchServices`
//! (which gives it the right TCC attribution). Embedded hosts may also start a
//! private service explicitly. The MCP client never sees that ownership
//! boundary — it receives the same JSON-RPC envelope.
//!
//! Why this lives in `cua-driver` and not `mcp-server`:
//!   `cua_driver_core::server` defines the shared JSON-RPC protocol. The
//!   proxy speaks that protocol on the client side, while the server side is the daemon's
//!   line-delimited JSON UDS protocol, owned by `crate::serve`.
//!   Putting the proxy here avoids `mcp-server → cua-driver` reverse
//!   coupling.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cua_driver_core::mcp_wire::{self, ProtocolSession};
use cua_driver_core::policy::{authorize_tool_call, is_tool_listable, validate_configured_policy};
use cua_driver_core::protocol::{initialize_result, Request, Response};
use cua_driver_core::server::{
    observe_proxy_session_started, observe_proxy_tool_completed, tool_observation_timer,
    StdioExecutionPath,
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::serve::{
    is_daemon_listening, send_request, DaemonRequest, DaemonResponse, ToolObservationOrigin,
};

pub(crate) const DIRECT_WORKER_ENV: &str = "CUA_DRIVER_DIRECT_MCP_WORKER";
pub(crate) const SERVICE_PROXY_WORKER_ENV: &str = "CUA_DRIVER_SERVICE_MCP_WORKER";
const DIRECT_WORKER_INIT_ID: &str = "cua-idle-supervisor-init";
const DIRECT_WORKER_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const DIRECT_WORKER_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Run stdio MCP directly over an SDK-owned runtime.
///
/// The stdio endpoint remains alive for its MCP host, but the desktop runtime
/// is created only for actual `tools/call` requests and is released after the
/// idle timeout. A later call rebuilds it through `driver_factory`, so an
/// idle host connection does not pin the platform runtime or prevent wake-on-use.
pub async fn run_direct<F>(
    mut driver_factory: F,
    tools_list: serde_json::Value,
) -> anyhow::Result<()>
where
    F: FnMut() -> anyhow::Result<Arc<cua_driver_sdk::CuaDriver>>,
{
    // Direct stdio is an action endpoint just like `serve`; enforce the same
    // admin lock, bounded-manifest approval/expiry, and legacy-approval
    // consistency before the first request can be read.
    cua_driver_core::authorization::validate_startup_authorization()?;
    validate_configured_policy()?;
    if crate::mcp_envelope::configured()? {
        // Typed envelopes hold a transport-owned receiver state that is not
        // reconstructed by the legacy JSON-RPC loop below. Keep that opt-in
        // transport on its existing explicit lifetime contract.
        let driver = driver_factory()?;
        let sdk = crate::sdk_adapter::SdkAdapter::load(driver).await?;
        let result = crate::mcp_envelope::run(
            sdk.clone(),
            BufReader::new(tokio::io::stdin()),
            tokio::io::stdout(),
        )
        .await;
        sdk.shutdown().await.map_err(anyhow::Error::msg)?;
        return result;
    }

    let idle_timeout = crate::serve::configured_idle_shutdown_timeout(
        false,
        false,
        cfg!(target_os = "macos"),
        std::env::var("CUA_DRIVER_IDLE_SHUTDOWN_SECONDS")
            .ok()
            .as_deref(),
    );
    if let Some(raw) = std::env::var("CUA_DRIVER_IDLE_SHUTDOWN_SECONDS").ok() {
        if raw.parse::<u64>().is_err() {
            eprintln!(
                "ignoring invalid CUA_DRIVER_IDLE_SHUTDOWN_SECONDS; expected whole seconds (0 disables)"
            );
        }
    }

    // MCP metadata and tool inventory do not require the platform runtime.
    // Hosts may keep this process open for a long time without using CUA.
    let tools_list = Arc::new(tools_list);
    let mut sdk: Option<Arc<crate::sdk_adapter::SdkAdapter>> = None;
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut session_observed = false;
    let mut protocol_session = ProtocolSession::default();
    let transport_session = format!("mcp-{}", uuid::Uuid::new_v4());
    let mut last_tool_activity = std::time::Instant::now();
    let mut next_release_attempt = None;

    loop {
        let idle_deadline = sdk.as_ref().and_then(|_| {
            idle_timeout
                .map(|timeout| next_release_attempt.unwrap_or_else(|| last_tool_activity + timeout))
        });
        let line = tokio::select! {
            result = reader.next_line() => result?,
            _ = async {
                tokio::time::sleep_until(tokio::time::Instant::from_std(
                    idle_deadline.expect("guarded idle deadline"),
                )).await;
            }, if idle_deadline.is_some() => {
                if let Some(runtime) = sdk.take() {
                    runtime.end_transport_sessions(&transport_session);
                    match runtime.shutdown().await {
                        Ok(()) => {
                            next_release_attempt = None;
                            eprintln!(
                                "Cua Driver MCP runtime idle for {} seconds; released. The stdio endpoint remains ready to restart it on the next tool call.",
                                idle_timeout.expect("idle deadline requires timeout").as_secs()
                            );
                        }
                        Err(error) => {
                            tracing::warn!("idle Cua Driver MCP runtime shutdown failed; retaining it for retry: {error}");
                            sdk = Some(runtime);
                            next_release_attempt = Some(
                                std::time::Instant::now() + std::time::Duration::from_secs(60),
                            );
                        }
                    }
                }
                continue;
            }
        };
        let Some(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(trimmed) {
            Err(error) => {
                error!("JSON parse error: {error}");
                Response::parse_error()
            }
            Ok(request) if request.is_notification() => continue,
            Ok(mut request) => {
                let era = match protocol_session.validate(&request).and_then(|era| {
                    if request.method == "tools/call" {
                        mcp_wire::validate_tool_call(
                            &request,
                            request.id.clone().unwrap_or_default(),
                            era,
                            &tools_list,
                        )?;
                    }
                    Ok(era)
                }) {
                    Ok(era) => era,
                    Err(response) => {
                        write_stdio_response(&mut writer, response).await?;
                        continue;
                    }
                };
                apply_direct_session_identity(&mut request, &transport_session);
                let initialize_metadata = (!session_observed)
                    .then(|| request.initialize_metadata())
                    .flatten();
                let id = request.id.clone().unwrap_or(serde_json::Value::Null);
                let method = request.method.clone();
                let response = if method == "initialize" {
                    Response::ok(id.clone(), initialize_result())
                } else if let Some(response) =
                    mcp_wire::handle_metadata_request(&request, id.clone())
                {
                    response
                } else if method == "tools/list" {
                    Response::ok(id.clone(), (*tools_list).clone())
                } else if method == "tools/call" {
                    last_tool_activity = std::time::Instant::now();
                    next_release_attempt = None;
                    let runtime = match ensure_direct_sdk(&mut sdk, &mut driver_factory).await {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            let response = Response::error(id.clone(), -32603, error.to_string());
                            writer
                                .write_all(serde_json::to_string(&response)?.as_bytes())
                                .await?;
                            writer.write_all(b"\n").await?;
                            writer.flush().await?;
                            continue;
                        }
                    };
                    apply_direct_session_identity(&mut request, &transport_session);
                    let session_context = request.tool_call().ok().and_then(|call| {
                        runtime.begin_tool_call(
                            &call.name,
                            &call.args,
                            cua_driver_core::session::SessionTransport::McpStdio,
                            cua_driver_core::session::SessionClientKind::Mcp,
                        )
                    });
                    let timer = tool_observation_timer(
                        &request,
                        |name| runtime.is_known_tool(name),
                        StdioExecutionPath::DirectDaemon,
                    );
                    let response = cua_driver_core::server::handle_request_with_transport_session(
                        request,
                        id.clone(),
                        runtime.as_ref(),
                        &transport_session,
                    )
                    .await;
                    if let Some(timer) = timer {
                        let outcome = timer.finish(&response);
                        if let Some(context) = session_context {
                            context.complete(&outcome);
                        }
                        observe_proxy_tool_completed(outcome);
                    }
                    last_tool_activity = std::time::Instant::now();
                    response
                } else {
                    Response::method_not_found(id.clone(), &method)
                };
                if let Some(metadata) = initialize_metadata {
                    observe_proxy_session_started(metadata);
                    session_observed = true;
                }
                mcp_wire::finish_response(era, &method, response)
            }
        };
        let serialized = serde_json::to_string(&response).unwrap_or_else(|error| {
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {error}"}}}}"#
            )
        });
        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    if let Some(runtime) = sdk {
        runtime.end_transport_sessions(&transport_session);
        runtime.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

async fn ensure_direct_sdk<F>(
    sdk: &mut Option<Arc<crate::sdk_adapter::SdkAdapter>>,
    driver_factory: &mut F,
) -> anyhow::Result<Arc<crate::sdk_adapter::SdkAdapter>>
where
    F: FnMut() -> anyhow::Result<Arc<cua_driver_sdk::CuaDriver>>,
{
    if sdk.is_none() {
        let driver = driver_factory()?;
        *sdk = Some(crate::sdk_adapter::SdkAdapter::load(driver).await?);
    }
    Ok(sdk.as_ref().expect("runtime was loaded").clone())
}

/// Keep the host-owned MCP stdio endpoint lightweight while supervising the
/// native process that owns Cua Driver's desktop runtime. OpenCode does not
/// reliably reconnect after an MCP child exits, so the endpoint must stay
/// available to launch a fresh worker when the next `tools/call` arrives.
pub async fn run_mcp_idle_supervisor(
    executable: std::path::PathBuf,
    arguments: Vec<std::ffi::OsString>,
    tools_list: serde_json::Value,
    worker_env: &'static str,
    disable_worker_idle_timeout: bool,
    envelopes: bool,
) -> anyhow::Result<()> {
    cua_driver_core::authorization::validate_startup_authorization()?;
    validate_configured_policy()?;

    let idle_timeout = crate::serve::configured_idle_shutdown_timeout(
        false,
        false,
        cfg!(target_os = "macos"),
        std::env::var("CUA_DRIVER_IDLE_SHUTDOWN_SECONDS")
            .ok()
            .as_deref(),
    );
    if let Some(raw) = std::env::var("CUA_DRIVER_IDLE_SHUTDOWN_SECONDS").ok() {
        if raw.parse::<u64>().is_err() {
            eprintln!(
                "ignoring invalid CUA_DRIVER_IDLE_SHUTDOWN_SECONDS; expected whole seconds (0 disables)"
            );
        }
    }

    let tools_list = Arc::new(tools_list);
    let mut worker: Option<DirectWorker> = None;
    let mut last_tool_activity = std::time::Instant::now();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut deferred_lines = VecDeque::new();
    let mut protocol_session = ProtocolSession::default();
    let mut session_observed = false;
    let mut worker_reaper = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_millis(100),
        std::time::Duration::from_millis(100),
    );
    worker_reaper.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let idle_deadline = worker
            .as_ref()
            .and_then(|_| idle_timeout.map(|timeout| last_tool_activity + timeout));
        let line = if let Some(line) = deferred_lines.pop_front() {
            Some(line)
        } else {
            tokio::select! {
                result = reader.next_line() => result?,
                _ = async {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(
                        idle_deadline.expect("guarded idle deadline"),
                    )).await;
                }, if idle_deadline.is_some() => {
                    if let Some(worker) = worker.take() {
                        worker.shutdown().await;
                        eprintln!(
                            "Cua Driver runtime process idle for {} seconds since the last real use; stopped. The MCP endpoint will start a new process when the runtime is next needed.",
                            idle_timeout.expect("idle deadline requires timeout").as_secs()
                        );
                    }
                    continue;
                },
                _ = worker_reaper.tick(), if worker.is_some() => {
                    if let Some(current) = worker.as_mut() {
                        if let Some(status) = current.child.try_wait()? {
                            eprintln!(
                                "Cua Driver runtime worker exited ({status}); the MCP endpoint will start a replacement when the runtime is next needed."
                            );
                            worker.take();
                        }
                    }
                    continue;
                }
            }
        };
        let Some(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Request>(trimmed) {
            Ok(request) if request.is_notification() => continue,
            Ok(request) => request,
            Err(error) => {
                let response = Response::parse_error();
                tracing::warn!("invalid MCP request at idle supervisor: {error}");
                write_stdio_response(&mut writer, response).await?;
                continue;
            }
        };
        let era = match protocol_session.validate(&request) {
            Ok(era) => era,
            Err(response) => {
                write_stdio_response(&mut writer, response).await?;
                continue;
            }
        };
        if let Err(response) = mcp_wire::validate_tool_call(
            &request,
            request.id.clone().unwrap_or_default(),
            era,
            &tools_list,
        ) {
            write_stdio_response(&mut writer, response).await?;
            continue;
        }

        let id = request.id.clone().unwrap_or(serde_json::Value::Null);
        let method = request.method.clone();
        let initialize_metadata = (!session_observed)
            .then(|| request.initialize_metadata())
            .flatten();

        let starts_worker =
            method == "tools/call" || (envelopes && method.starts_with("cua/driver/v1/"));
        let worker_exited = match worker.as_mut() {
            Some(current) => current.child.try_wait()?.is_some(),
            None => false,
        };
        if worker_exited {
            worker.take();
        }
        if starts_worker {
            let request_line = trimmed.to_owned();
            let call_result: anyhow::Result<String> = async {
                if worker.is_none() {
                    let mut command = tokio::process::Command::new(&executable);
                    command.args(&arguments).env(worker_env, "1");
                    if disable_worker_idle_timeout {
                        // This mode's parent owns the process timeout. The
                        // child stays until the parent closes stdin.
                        command.env("CUA_DRIVER_IDLE_SHUTDOWN_SECONDS", "0");
                    }
                    worker = Some(
                        tokio::time::timeout(
                            DIRECT_WORKER_START_TIMEOUT,
                            DirectWorker::spawn(command),
                        )
                        .await
                        .map_err(|_| anyhow::anyhow!("Cua Driver worker startup timed out"))??,
                    );
                    eprintln!("Cua Driver runtime process started for MCP tools/call.");
                }
                let response = tokio::time::timeout(DIRECT_WORKER_CALL_TIMEOUT, async {
                    let direct_worker = worker.as_mut().expect("worker was started");
                    exchange_worker_request_with_controls(
                        direct_worker.stdin.as_mut().expect("worker stdin is open"),
                        &mut direct_worker.stdout,
                        &mut direct_worker.output_buffer,
                        &request_line,
                        &mut reader,
                        &mut writer,
                        &mut deferred_lines,
                        envelopes,
                    )
                    .await
                })
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Cua Driver tool call timed out after {} seconds; outcome may be unknown",
                        DIRECT_WORKER_CALL_TIMEOUT.as_secs()
                    )
                })??;
                Ok(response)
            }
            .await;
            if worker_method_counts_as_use(&method) {
                last_tool_activity = std::time::Instant::now();
            }

            let response_line = match call_result {
                Ok(response_line) => response_line,
                Err(error) => {
                    if let Some(worker) = worker.take() {
                        worker.shutdown().await;
                    }
                    let response = Response::error(
                        id,
                        -32603,
                        format!(
                            "Cua Driver worker disconnected during this call; outcome may be unknown. Observe the target state before retrying. {error}"
                        ),
                    );
                    write_stdio_response(
                        &mut writer,
                        mcp_wire::finish_response(era, &method, response),
                    )
                    .await?;
                    if let Some(metadata) = initialize_metadata {
                        observe_proxy_session_started(metadata);
                        session_observed = true;
                    }
                    continue;
                }
            };
            if let Some(metadata) = initialize_metadata {
                observe_proxy_session_started(metadata);
                session_observed = true;
            }
            writer.write_all(response_line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            continue;
        }

        let response = if method == "initialize" {
            let mut result = initialize_result();
            if envelopes {
                result["capabilities"]["experimental"][crate::mcp_envelope::CAPABILITY] =
                    serde_json::json!({"version": 1});
            }
            Response::ok(id, result)
        } else if method == "tools/list" {
            Response::ok(id, (*tools_list).clone())
        } else if let Some(response) = mcp_wire::handle_metadata_request(&request, id.clone()) {
            response
        } else {
            Response::method_not_found(id, &method)
        };
        if let Some(metadata) = initialize_metadata {
            observe_proxy_session_started(metadata);
            session_observed = true;
        }
        write_stdio_response(
            &mut writer,
            mcp_wire::finish_response(era, &method, response),
        )
        .await?;
    }

    if let Some(worker) = worker.take() {
        worker.shutdown().await;
    }
    Ok(())
}

fn worker_method_counts_as_use(method: &str) -> bool {
    matches!(
        method,
        "tools/call" | "cua/driver/v1/open" | "cua/driver/v1/exchange"
    )
}

async fn write_stdio_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: Response,
) -> anyhow::Result<()> {
    writer
        .write_all(serde_json::to_string(&response)?.as_bytes())
        .await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

struct DirectWorker {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    output_buffer: Vec<u8>,
}

impl DirectWorker {
    async fn spawn(mut command: tokio::process::Command) -> anyhow::Result<Self> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Cua Driver worker stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Cua Driver worker stdout was not piped"))?;
        let mut worker = Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            output_buffer: Vec::new(),
        };

        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": DIRECT_WORKER_INIT_ID,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cua-idle-supervisor", "version": "1"}
            }
        })
        .to_string();
        let response =
            tokio::time::timeout(DIRECT_WORKER_START_TIMEOUT, worker.exchange(&initialize))
                .await
                .map_err(|_| anyhow::anyhow!("Cua Driver worker initialize timed out"))??;
        let response: serde_json::Value = serde_json::from_str(&response)?;
        anyhow::ensure!(
            response.get("id").and_then(serde_json::Value::as_str) == Some(DIRECT_WORKER_INIT_ID)
                && response.get("result").is_some(),
            "Cua Driver worker did not initialize: {response}"
        );
        worker
            .write_notification("notifications/initialized")
            .await?;
        Ok(worker)
    }

    async fn exchange(&mut self, request: &str) -> anyhow::Result<String> {
        let request_value: serde_json::Value = serde_json::from_str(request)?;
        let expected_id = request_value
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MCP request has no id"))?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Cua Driver worker stdin is closed"))?;
        stdin.write_all(request.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        loop {
            let Some(response_line) =
                read_worker_line(&mut self.stdout, &mut self.output_buffer).await?
            else {
                anyhow::bail!("Cua Driver worker closed stdout before replying");
            };
            let response: serde_json::Value = serde_json::from_str(response_line.trim())?;
            if response.get("id") == Some(&expected_id) {
                return Ok(response_line.trim_end().to_owned());
            }
        }
    }

    async fn write_notification(&mut self, method: &str) -> anyhow::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Cua Driver worker stdin is closed"))?;
        stdin
            .write_all(
                serde_json::json!({"jsonrpc":"2.0","method":method})
                    .to_string()
                    .as_bytes(),
            )
            .await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn shutdown(mut self) {
        // Closing stdin gives the worker a normal MCP EOF so it can end its
        // sessions and shut down the SDK before the process is force-killed.
        drop(self.stdin.take());
        if tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            tracing::warn!("Cua Driver worker did not exit after stdin EOF; terminating it");
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
    }
}

async fn read_worker_line<R>(
    stdout: &mut R,
    output_buffer: &mut Vec<u8>,
) -> anyhow::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let read = stdout.read_until(b'\n', output_buffer).await?;
    if read == 0 {
        return Ok(None);
    }
    let bytes = std::mem::take(output_buffer);
    Ok(Some(String::from_utf8(bytes)?))
}

async fn write_worker_line<W>(stdin: &mut W, line: &str) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    stdin.write_all(line.trim_end().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

async fn write_stdio_line<W>(writer: &mut W, line: &str) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(line.trim_end().as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn valid_worker_request_id(id: &serde_json::Value) -> bool {
    id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()
}

/// Keep typed envelope cancellation and close requests responsive while the
/// worker is waiting for a native action. Other client requests stay queued
/// until that action completes; worker responses are correlated by JSON-RPC id.
async fn exchange_worker_request_with_controls<R, W, S, O>(
    stdin: &mut S,
    stdout: &mut O,
    output_buffer: &mut Vec<u8>,
    request_line: &str,
    reader: &mut tokio::io::Lines<R>,
    writer: &mut W,
    deferred_lines: &mut VecDeque<String>,
    envelopes: bool,
) -> anyhow::Result<String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    S: AsyncWrite + Unpin,
    O: AsyncBufRead + Unpin,
{
    let request: serde_json::Value = serde_json::from_str(request_line)?;
    let expected_id = request
        .get("id")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("MCP request has no id"))?;
    let expected_key = expected_id.to_string();
    write_worker_line(stdin, request_line).await?;

    let mut control_ids = HashMap::<String, serde_json::Value>::new();
    let mut request_response = None;
    loop {
        if control_ids.is_empty() {
            if let Some(response) = request_response.take() {
                return Ok(response);
            }
        }

        tokio::select! {
            worker_line = read_worker_line(stdout, output_buffer) => {
                let Some(worker_line) = worker_line? else {
                    for (_, id) in control_ids.drain() {
                        let error = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32603, "message": "worker cerró antes de responder al control"}
                        });
                        write_stdio_line(writer, &error.to_string()).await?;
                    }
                    anyhow::bail!("Cua Driver worker closed stdout before replying");
                };
                let message: serde_json::Value = match serde_json::from_str(worker_line.trim()) {
                    Ok(message) => message,
                    Err(_) => continue,
                };
                if message.get("method").and_then(serde_json::Value::as_str).is_some() {
                    write_stdio_line(writer, &worker_line).await?;
                    continue;
                }
                let Some(id) = message.get("id") else { continue };
                let key = id.to_string();
                if control_ids.remove(&key).is_some() {
                    write_stdio_line(writer, &worker_line).await?;
                } else if key == expected_key && request_response.is_none() {
                    request_response = Some(worker_line.trim_end().to_owned());
                }
            }
            client_line_result = reader.next_line() => {
                let Some(client_line) = client_line_result? else {
                    anyhow::bail!("MCP client closed stdin while a worker request was active");
                };
                let message: serde_json::Value = match serde_json::from_str(&client_line) {
                    Ok(message) => message,
                    Err(_) => {
                        deferred_lines.push_back(client_line);
                        continue;
                    }
                };
                let id = message.get("id").filter(|id| valid_worker_request_id(id));
                let key = id.map(serde_json::Value::to_string);
                let method = message.get("method").and_then(serde_json::Value::as_str);
                let valid_jsonrpc = message.get("jsonrpc").and_then(serde_json::Value::as_str) == Some("2.0");
                let has_result = message.as_object().is_some_and(|object| object.contains_key("result"));
                let has_error = message.as_object().is_some_and(|object| object.contains_key("error"));
                let is_response = valid_jsonrpc && method.is_none() && key.is_some() && (has_result ^ has_error);
                let is_cancellation = valid_jsonrpc
                    && message.get("method").and_then(serde_json::Value::as_str) == Some("notifications/cancelled")
                    && message.get("id").is_none();
                let is_envelope_control = envelopes
                    && valid_jsonrpc
                    && matches!(method, Some("cua/driver/v1/cancel" | "cua/driver/v1/close"))
                    && key.is_some();

                if is_envelope_control {
                    let control_key = key.expect("control requires id");
                    if control_key == expected_key || control_ids.contains_key(&control_key) {
                        deferred_lines.push_back(client_line);
                        continue;
                    }
                    control_ids.insert(control_key, id.expect("control requires id").clone());
                    write_worker_line(stdin, &client_line).await?;
                } else if is_cancellation || is_response {
                    write_worker_line(stdin, &client_line).await?;
                } else {
                    deferred_lines.push_back(client_line);
                }
            }
        }
    }
}

pub(crate) fn apply_direct_session_identity(request: &mut Request, transport_session: &str) {
    let Some(arguments) = request
        .params
        .as_mut()
        .and_then(|params| params.get_mut("arguments"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let effective = arguments
        .get("session")
        .and_then(serde_json::Value::as_str)
        .filter(|session| !session.is_empty())
        .unwrap_or(transport_session)
        .to_owned();
    arguments.insert("_session_id".into(), serde_json::Value::String(effective));
    arguments.insert(
        "_transport_session_id".into(),
        serde_json::Value::String(transport_session.to_owned()),
    );
}

/// Run the MCP stdio proxy. Reads JSON-RPC lines from stdin, forwards
/// the body of each `tools/list` / `tools/call` to the daemon at
/// `socket_path`, and writes the daemon's response back as a proper
/// JSON-RPC envelope.
///
/// Implements the core protocol's EOF, parse-error, notification, and
/// response rules while forwarding method dispatch to the daemon.
///
/// Fails fast if the daemon isn't reachable, so MCP clients see a
/// clear startup error instead of a "successful" handshake that
/// advertises zero tools and then errors on every call. Matches
/// Swift `makeProxy`'s `fetchProxyToolList` pre-check.
pub async fn run_proxy(socket_path: String) -> anyhow::Result<()> {
    validate_configured_policy()?;
    if !is_daemon_listening(&socket_path) {
        anyhow::bail!(
            "cua-driver-rs daemon not reachable on {socket_path}. Start it \
             with `cua-driver permissions grant` and retry."
        );
    }
    // A selected service may outlive the CLI package that launched this
    // proxy. Refuse an incompatible contract before creating the control
    // binding or forwarding any action.
    let compatibility_client = cua_driver_sdk::CuaDriver::connect(Some(socket_path.clone()))?;
    compatibility_client.metadata().await?;
    if crate::mcp_envelope::configured()? {
        return crate::mcp_envelope::proxy(&socket_path).await;
    }

    // Mint this MCP session's identity once at proxy startup. One proxy process
    // == one MCP session; the daemon outlives it. We stamp this id on every
    // forwarded request so the daemon can OWN and CLEAN UP this session's
    // state (recording, config overrides) and tear it down on disconnect via
    // a `session_end` signal. Dep-free `pid + start-nanos` is sufficient for
    // daemon-local uniqueness over this proxy's lifetime (no `uuid` crate dep
    // for one mint).
    let session_id = mint_session_id();
    debug!(session_id = %session_id, "proxy session minted");

    // Open ONE long-lived "control" connection to the daemon and hold it open
    // for this proxy's entire lifetime (separate from the per-call connections
    // that `send_request` opens and closes per tool call). It sends a single
    // `session_begin` line and then parks reading — it never writes again and
    // never closes until this process dies.
    //
    // This is the reaper: when the proxy exits (graceful stdin EOF) OR is
    // SIGKILLed/crashes, the kernel closes this socket; the daemon's
    // per-connection reader hits EOF and fires `session_end` for `session_id`,
    // tearing down every piece of state this session owns (overlay cursor,
    // config overrides, recording). Liveness is connection-based, so an
    // alive-but-idle session — one issuing zero tool calls — is never reaped:
    // its control connection stays parked open.
    //
    // The daemon must acknowledge `session_begin` before the proxy accepts tool
    // calls. Besides lifecycle cleanup, that registered control channel is the
    // trust boundary used by destructive `browser_prepare` calls.
    let (control_ready_tx, control_ready_rx) = tokio::sync::oneshot::channel();
    let control = run_control_connection(socket_path.clone(), session_id.clone(), control_ready_tx);
    tokio::pin!(control);
    supervise_control_connection(&mut control, async {
        tokio::time::timeout(std::time::Duration::from_secs(4), control_ready_rx)
            .await
            .map_err(|_| anyhow::anyhow!("daemon did not acknowledge the MCP control session"))?
            .map_err(|_| anyhow::anyhow!("daemon control session closed before acknowledgement"))
    })
    .await?;

    // Cache the tool list once at startup. The daemon's registry is
    // static for the lifetime of the daemon, so polling on every
    // `tools/list` would waste a round-trip per call. Swift does the
    // same caching in `fetchProxyToolList`.
    let (cached_tools_list, daemon_observes_tool_calls) =
        fetch_tools_list_from_daemon(&socket_path, &session_id)?;
    let cached_tools_list = Arc::new(cached_tools_list);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    supervise_control_connection(
        &mut control,
        run_proxy_io(
            BufReader::new(stdin),
            tokio::io::BufWriter::new(stdout),
            &socket_path,
            &cached_tools_list,
            &session_id,
            daemon_observes_tool_calls,
        ),
    )
    .await
}

/// Keep the MCP transport alive only while its daemon-owned identity is live.
/// Dropping the scoped control future also closes its socket on stdin EOF,
/// startup failure, or cancellation; no detached task can retain the session.
async fn supervise_control_connection<T>(
    control: impl std::future::Future<Output = ()>,
    work: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::select! {
        biased;
        _ = control => anyhow::bail!("daemon control session closed; reconnect the MCP client"),
        result = work => result,
    }
}

/// Run the service-owned stdio loop over caller-provided I/O.
///
/// A clean reader EOF must return `Ok(())` promptly. The caller then drops the
/// persistent control connection, allowing the daemon to reap the MCP session
/// and its recording, preview, and overlay state (issue #2002).
async fn run_proxy_io<R, W>(
    mut reader: R,
    mut writer: W,
    socket_path: &str,
    cached_tools_list: &Arc<serde_json::Value>,
    session_id: &str,
    daemon_observes_tool_calls: bool,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = String::new();
    let mut session_observed = false;
    let mut protocol_session = ProtocolSession::default();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF — MCP client disconnected (stdin closed).
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        debug!(raw = trimmed, "→ proxy request");

        let response = match serde_json::from_str::<Request>(trimmed) {
            Err(e) => {
                error!("JSON parse error: {e}");
                Response::parse_error()
            }
            Ok(req) if req.is_notification() => {
                // Notifications are intentionally dropped by the stdio adapter.
                continue;
            }
            Ok(req) => {
                let admission = protocol_session.validate(&req).and_then(|era| {
                    mcp_wire::validate_tool_call(
                        &req,
                        req.id.clone().unwrap_or_default(),
                        era,
                        cached_tools_list,
                    )?;
                    Ok(era)
                });
                if let Err(response) = admission {
                    writer
                        .write_all(serde_json::to_string(&response)?.as_bytes())
                        .await?;
                    writer.write_all(b"\n").await?;
                    writer.flush().await?;
                    continue;
                }
                let initialize_metadata = (!session_observed)
                    .then(|| req.initialize_metadata())
                    .flatten();
                let session_context = (!daemon_observes_tool_calls)
                    .then(|| {
                        req.tool_call().ok().and_then(|call| {
                            let known_tool = proxy_knows_tool(cached_tools_list, &call.name);
                            cua_driver_core::session::begin_tool_call(
                                &call.name,
                                &call.args,
                                known_tool,
                                cua_driver_core::session::SessionTransport::McpStdio,
                                cua_driver_core::session::SessionClientKind::Mcp,
                            )
                        })
                    })
                    .flatten();
                let tool_timer = (!daemon_observes_tool_calls)
                    .then(|| {
                        tool_observation_timer(
                            &req,
                            |name| proxy_knows_tool(cached_tools_list, name),
                            StdioExecutionPath::DaemonProxy,
                        )
                    })
                    .flatten();
                let id = req.id.clone().unwrap_or(serde_json::Value::Null);
                let response = handle_proxy_request(
                    req,
                    id,
                    socket_path,
                    cached_tools_list,
                    session_id,
                    daemon_observes_tool_calls,
                )
                .await;
                if let Some(metadata) = initialize_metadata {
                    observe_proxy_session_started(metadata);
                    session_observed = true;
                }
                if let Some(timer) = tool_timer {
                    let outcome = timer.finish(&response);
                    if let Some(context) = session_context {
                        context.complete(&outcome);
                    }
                    observe_proxy_tool_completed(outcome);
                }
                response
            }
        };

        let serialized = serde_json::to_string(&response).unwrap_or_else(|e| {
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#
            )
        });
        debug!(raw = %serialized, "← proxy response");

        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    // Reached on a clean stdin EOF (the `n == 0` break above) — the normal
    // "MCP client disconnected" seam. Session teardown is NO LONGER done here:
    // it's fully subsumed by the persistent control connection spawned at
    // startup. On any proxy exit — graceful stdin EOF (this path), an I/O
    // error propagated via `?`, OR a SIGKILL/crash — the kernel closes the
    // control socket, the daemon's reader hits EOF, and it fires
    // `session_end(session_id)` once (idempotent). That single path reliably
    // covers the ungraceful-death case the old best-effort exit hook missed.
    Ok(())
}

fn proxy_knows_tool(cached_tools_list: &serde_json::Value, name: &str) -> bool {
    if name == "type_text_chars" || cua_driver_core::tool::renamed_tool(name).is_some() {
        return true;
    }
    cached_tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(name))
        })
}

/// Own the proxy's single long-lived control connection. Connects directly to
/// the daemon socket (its OWN async open — `send_request` is sync, blocking,
/// and one-shot, so it cannot be reused here), sends one `session_begin` line
/// carrying `session_id`, then parks in a read loop until the connection
/// closes. It never writes again. The daemon records `session_id` from
/// `session_begin` and fires `session_end` when this connection EOFs — which
/// the kernel triggers on proxy exit AND on kill -9.
///
/// On EOF/error (daemon-side close, broken pipe), this future completes and
/// its supervisor closes the MCP proxy. Per-call connections must never keep
/// serving the ended identity. Startup connection failures follow the same path.
async fn run_control_connection(
    socket_path: String,
    session_id: String,
    control_ready: tokio::sync::oneshot::Sender<()>,
) {
    let begin = DaemonRequest {
        method: "session_begin".into(),
        name: None,
        args: None,
        session_id: Some(session_id.clone()),
        observation_origin: None,
        client_kind: None,
    };
    let line = match serde_json::to_string(&begin) {
        Ok(s) => s + "\n",
        Err(e) => {
            warn!("control connection: serialize session_begin failed: {e}");
            return;
        }
    };

    #[cfg(unix)]
    {
        use tokio::net::UnixStream;
        // Retry the connect briefly — the daemon may still be spinning up
        // (mirrors the windows pipe-open retry below). The is_daemon_listening
        // precheck makes the window tiny, but keep both paths symmetric.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut stream = loop {
            match UnixStream::connect(&socket_path).await {
                Ok(s) => break s,
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => {
                    debug!(session_id = %session_id, "control connect failed (daemon starting?): {e}");
                    return;
                }
            }
        };
        if let Err(e) = stream.write_all(line.as_bytes()).await {
            debug!("control connection: write session_begin failed: {e}");
            return;
        }
        let _ = stream.flush().await;
        debug!(session_id = %session_id, "control connection established (session_begin sent)");

        // Park: read until the daemon closes (it ACKs session_begin then keeps
        // the conn open; we drain anything and only return on EOF/error). The
        // proxy never writes here again — the connection lives until process
        // death, when the kernel closes it and the daemon reaps the session.
        let mut reader = BufReader::new(stream);
        let mut buf = String::new();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if let Err(error) = validate_session_begin_ack(&buf) {
                    warn!("control connection: {error}");
                    return;
                }
                let _ = control_ready.send(());
            }
        }
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) | Err(_) => break, // daemon closed or error — task done.
                Ok(_) => continue,       // ACK / stray line — ignore, keep parked.
            }
        }
        debug!(session_id = %session_id, "control connection closed");
    }

    #[cfg(all(not(unix), target_os = "windows"))]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        // Retry the pipe open briefly — the daemon may still be spinning up its
        // next instance (mirrors send_request's open-retry).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let client = loop {
            match ClientOptions::new().open(&socket_path) {
                Ok(c) => break Some(c),
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => {
                    debug!(session_id = %session_id, "control pipe open failed (daemon starting?): {e}");
                    break None;
                }
            }
        };
        let mut client = match client {
            Some(c) => c,
            None => return,
        };
        if let Err(e) = client.write_all(line.as_bytes()).await {
            debug!("control connection: write session_begin failed: {e}");
            return;
        }
        let _ = client.flush().await;
        debug!(session_id = %session_id, "control connection established (session_begin sent)");

        let mut reader = BufReader::new(client);
        let mut buf = String::new();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if let Err(error) = validate_session_begin_ack(&buf) {
                    warn!("control connection: {error}");
                    return;
                }
                let _ = control_ready.send(());
            }
        }
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        debug!(session_id = %session_id, "control connection closed");
    }

    #[cfg(all(not(unix), not(target_os = "windows")))]
    {
        let _ = (line, session_id, socket_path, control_ready);
    }
}

fn validate_session_begin_ack(line: &str) -> Result<(), String> {
    let response: DaemonResponse = serde_json::from_str(line)
        .map_err(|_| "malformed session_begin acknowledgement".to_owned())?;
    if !response.ok
        || response
            .result
            .as_ref()
            .and_then(|result| result.get("session_begin"))
            != Some(&serde_json::Value::Bool(true))
    {
        return Err("daemon did not acknowledge session_begin".to_owned());
    }
    Ok(())
}

/// Mint a session id unique among the live proxies sharing one daemon, for the
/// lifetime of this proxy process. `pid + process-start nanos` is dep-free and
/// sufficient: two proxies can't share a pid concurrently, and the nanos guard
/// disambiguates pid reuse across the daemon's lifetime. We deliberately avoid
/// the `uuid` crate — a single v4 mint isn't worth a new dependency.
fn mint_session_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("mcp-{pid}-{nanos}")
}

/// One-shot daemon `list` over the UDS, reshaped into a MCP
/// `tools/list` result. The daemon now returns the full ToolDef
/// (`name`, `description`, `input_schema`, annotation hints) per
/// commit 3's `serve.rs` change.
fn fetch_tools_list_from_daemon(
    socket_path: &str,
    session_id: &str,
) -> anyhow::Result<(serde_json::Value, bool)> {
    let req = DaemonRequest {
        method: "list".into(),
        name: None,
        args: None,
        session_id: Some(session_id.to_owned()),
        observation_origin: None,
        client_kind: None,
    };
    let resp = send_request(socket_path, &req)?;
    if !resp.ok {
        anyhow::bail!(
            "daemon refused tool list on {socket_path}: {}",
            resp.error.unwrap_or_else(|| "(no error message)".into())
        );
    }
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("daemon list response missing `result` field"))?;
    let tools_array = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("daemon list response missing `tools` array"))?;

    validate_daemon_output_schemas(tools_array)?;

    // Reshape the daemon's `{name, description, input_schema, output_schema,
    // read_only, ..., capabilities}` envelope into MCP's `{name, description,
    // inputSchema, outputSchema, annotations: {...}, capabilities}` shape.
    // Same translation `ToolDef::to_list_entry` defines for the core protocol.
    //
    // `capabilities` is passed through verbatim when the daemon
    // provides it; older daemons that don't emit the field fall back
    // to a name-keyed lookup so the proxy still surfaces capability
    // metadata without an extra round-trip.
    let mcp_tools: Vec<serde_json::Value> = tools_array
        .iter()
        .map(|t| {
            let name = t.get("name").cloned().unwrap_or(serde_json::Value::Null);
            let description = t
                .get("description")
                .cloned()
                .unwrap_or(serde_json::Value::String(String::new()));
            let input_schema = t
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
            let read_only = t
                .get("read_only")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let destructive = t
                .get("destructive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let idempotent = t
                .get("idempotent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let open_world = t
                .get("open_world")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let capabilities = t
                .get("capabilities")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_else(|| {
                    // Fallback: derive from the centralised name + schema
                    // resolver. Keeps the proxy compatible with daemon
                    // builds that pre-date the capabilities field.
                    name.as_str()
                        .map(|name| {
                            cua_driver_core::tool::advertised_capabilities_for(name, &input_schema)
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect()
                });
            let risk = t.get("risk").cloned().unwrap_or_else(|| {
                name.as_str()
                    .map(cua_driver_core::authorization::risk_metadata_json)
                    .unwrap_or_else(|| {
                        serde_json::json!({
                            "class": "unclassified",
                            "enforcement": "metadata_only",
                            "operation_sensitive": false,
                            "version": cua_driver_core::authorization::RISK_METADATA_VERSION,
                        })
                    })
            });
            let mut tool = serde_json::json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
                "annotations": {
                    "readOnlyHint": read_only,
                    "destructiveHint": destructive,
                    "idempotentHint": idempotent,
                    "openWorldHint": open_world,
                },
                "capabilities": capabilities,
                "risk": risk,
            });
            // Do not derive a new schema when an older daemon omitted it:
            // mixed-version proxies must advertise only the result contract
            // that the executing daemon actually owns.
            if let Some(output_schema) = t.get("output_schema") {
                tool.as_object_mut()
                    .expect("MCP tool entry is an object")
                    .insert("outputSchema".into(), output_schema.clone());
            }
            tool
        })
        .collect();

    // `capability_version` and `schema_version` are passed through
    // when the daemon emits them; older daemons fall back to the
    // proxy's compiled-in `CAPABILITY_VERSION` so MCP clients always
    // see the envelope keys.
    let capability_version = result
        .get("capability_version")
        .cloned()
        .unwrap_or_else(|| {
            serde_json::Value::String(cua_driver_core::tool::CAPABILITY_VERSION.to_owned())
        });
    let schema_version = result.get("schema_version").cloned().unwrap_or_else(|| {
        serde_json::Value::String(cua_driver_core::tool::TOOLS_LIST_SCHEMA_VERSION.to_owned())
    });

    let daemon_observes_tool_calls = daemon_owns_tool_observation(&result);

    Ok((
        serde_json::json!({
            "tools": mcp_tools,
            "capability_version": capability_version,
            "schema_version": schema_version,
        }),
        daemon_observes_tool_calls,
    ))
}

fn validate_daemon_output_schemas(tools: &[serde_json::Value]) -> anyhow::Result<()> {
    for tool in tools {
        let name = tool
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("daemon tool entry missing name"))?;
        cua_driver_core::mcp_result::validate_proxy_output_schema(name, tool.get("output_schema"))
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

fn proxy_output_schema<'a>(
    tools: &'a serde_json::Value,
    name: &str,
) -> Option<&'a serde_json::Value> {
    let tools = tools.get("tools")?.as_array()?;
    tools
        .iter()
        .find(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(name))
        .or_else(|| {
            let current = if name == "type_text_chars" {
                Some("type_text")
            } else {
                cua_driver_core::tool::renamed_tool(name)
            }?;
            tools
                .iter()
                .find(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(current))
        })?
        .get("outputSchema")
}

fn proxy_tool_result_response(
    id: serde_json::Value,
    result: Result<serde_json::Value, String>,
) -> Response {
    match result {
        Ok(result) => Response::ok(id, result),
        Err(error) => Response::error(id, -32603, error),
    }
}

/// With `CUA_DRIVER_AX_ONLY=1`, hide tools that the configured policy can
/// never allow (e.g. physical mouse/keyboard tools) from `tools/list`.
pub(crate) fn filtered_tools_list(cached: &serde_json::Value) -> serde_json::Value {
    if std::env::var_os("CUA_DRIVER_AX_ONLY").is_none() {
        return cached.clone();
    }
    let mut out = cached.clone();
    if let Some(tools) = out.get_mut("tools").and_then(|t| t.as_array_mut()) {
        tools.retain(|tool| {
            tool.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(is_tool_listable)
        });
    }
    out
}

fn daemon_owns_tool_observation(result: &serde_json::Value) -> bool {
    result
        .get("tool_observation_owner")
        .and_then(serde_json::Value::as_str)
        == Some("daemon")
}

/// JSON-RPC method dispatcher for the proxy. Mirrors
/// `cua_driver_core::server::handle_request`:
/// - `initialize` → static `initialize_result()` (same envelope as the core
///   protocol server; the daemon's identity is hidden from the MCP client).
/// - `tools/list` → return the cached daemon tool list.
/// - `tools/call` → forward to the daemon and reshape the response into MCP's
///   `CallTool.Result`.
/// - other → method-not-found.
async fn handle_proxy_request(
    req: Request,
    id: serde_json::Value,
    socket_path: &str,
    cached_tools_list: &Arc<serde_json::Value>,
    session_id: &str,
    daemon_observes_tool_calls: bool,
) -> Response {
    let era = match mcp_wire::classify_request(&req) {
        Ok(era) => era,
        Err(response) => return response,
    };
    if let Err(response) = mcp_wire::validate_tool_call(&req, id.clone(), era, cached_tools_list) {
        return response;
    }
    let method = req.method.clone();
    let response = if let Some(response) = mcp_wire::handle_metadata_request(&req, id.clone()) {
        response
    } else {
        handle_proxy_tool_request(
            req,
            id,
            socket_path,
            cached_tools_list,
            session_id,
            daemon_observes_tool_calls,
        )
        .await
    };
    mcp_wire::finish_response(era, &method, response)
}

async fn handle_proxy_tool_request(
    req: Request,
    id: serde_json::Value,
    socket_path: &str,
    cached_tools_list: &Arc<serde_json::Value>,
    session_id: &str,
    daemon_observes_tool_calls: bool,
) -> Response {
    match req.method.as_str() {
        "initialize"
            if mcp_wire::classify_request(&req).ok() == Some(mcp_wire::ProtocolEra::Legacy) =>
        {
            Response::ok(id, initialize_result())
        }

        "tools/list" => Response::ok(id, filtered_tools_list(cached_tools_list)),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                if let Err(error) = authorize_tool_call(&call.name, &call.args) {
                    return proxy_tool_result_response(
                        id,
                        cua_driver_core::mcp_result::conforming_proxy_tool_result(
                            &call.name,
                            cua_driver_core::mcp_result::tool_error_result(
                                error.to_string(),
                                serde_json::json!({"code": "permission_denied"}),
                            ),
                            proxy_output_schema(cached_tools_list, &call.name),
                        ),
                    );
                }
                forward_tool_call(
                    id,
                    call.name.clone(),
                    call.args,
                    socket_path,
                    session_id,
                    daemon_observes_tool_calls,
                    proxy_output_schema(cached_tools_list, &call.name).cloned(),
                )
                .await
            }
        },

        other => {
            warn!(method = other, "unknown method");
            Response::method_not_found(id, other)
        }
    }
}

/// Forward a single MCP `tools/call` to the daemon as a `call`
/// request, then translate the `DaemonResponse` back into an MCP
/// `CallTool.Result` envelope.
///
/// Error mapping:
///   - Tool ran and reported failure (`!resp.ok`, including unknown
///     tool / bad params) → JSON-RPC success with `result.isError =
///     true`. Mirrors the core protocol's tool-error envelope.
///   - Transport failure (UDS unreachable, decode error, blocking
///     task panic) → JSON-RPC error (`-32603`), because the MCP
///     client really does need to distinguish "tool said no" from
///     "I couldn't reach the tool at all."
async fn forward_tool_call(
    id: serde_json::Value,
    name: String,
    mut args: serde_json::Value,
    socket_path: &str,
    session_id: &str,
    daemon_observes_tool_calls: bool,
    output_schema: Option<serde_json::Value>,
) -> Response {
    cua_driver_core::tool_args::sanitize_reserved_args(&mut args);
    let req = DaemonRequest {
        method: "call".into(),
        name: Some(name.clone()),
        args: Some(args),
        session_id: Some(session_id.to_owned()),
        observation_origin: daemon_observes_tool_calls.then_some(ToolObservationOrigin::McpProxy),
        client_kind: None,
    };

    // The daemon client is sync, so jump to a blocking thread to keep
    // the tokio reactor responsive while the AX-heavy call (e.g.
    // `get_window_state`) does its thing on the daemon
    // side.
    let socket = socket_path.to_owned();
    let blocking = tokio::task::spawn_blocking(move || send_request(&socket, &req)).await;

    let resp = match blocking {
        Err(join_err) => {
            return Response::error(
                id,
                -32603,
                format!("internal join error forwarding to daemon: {join_err}"),
            );
        }
        Ok(Err(e)) => {
            return Response::error(
                id,
                -32603,
                format!("daemon transport error forwarding `{name}`: {e}"),
            );
        }
        Ok(Ok(r)) => r,
    };

    proxy_tool_result_response(
        id,
        daemon_response_to_tool_result(&name, resp, output_schema.as_ref()),
    )
}

/// Translate one `DaemonResponse` into the `CallTool.Result` the MCP client
/// receives.
///
/// MCP separates two failure modes:
///   - JSON-RPC errors → `Response::error(...)`, used for transport /
///     protocol failures (unknown method, bad params shape, server crash).
///     Those are handled by the caller, above.
///   - Tool-level errors → `Response::ok(...)` carrying a `CallTool.Result`
///     with `isError: true` and the error message in `content[]`. The tool
///     ran, returned a well-formed result that says "I failed."
///
/// A non-`ok` daemon response means the tool call reached the daemon and the
/// daemon decided the tool returned an error (or rejected the call). That's
/// tool-level, not transport-level, so the core protocol surfaces it as
/// `Response::ok` with `isError: true`. Mirror that shape here — CodeRabbit #2.
///
/// Every branch — including the empty result synthesised for an `ok` response
/// that carries none — leaves through the same boundary the direct dispatch
/// uses, so a daemon-backed call is held to the tool's advertised
/// `outputSchema` exactly as a direct one is.
fn daemon_response_to_tool_result(
    name: &str,
    resp: DaemonResponse,
    output_schema: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let result = if resp.ok {
        resp.result.unwrap_or_else(|| {
            serde_json::json!({
                "content": [],
                "isError": false
            })
        })
    } else {
        let msg = resp
            .error
            .unwrap_or_else(|| "daemon reported failure".into());
        let exit_code = resp.exit_code.unwrap_or(1);
        cua_driver_core::mcp_result::tool_error_result(
            msg,
            serde_json::json!({ "exit_code": exit_code }),
        )
    };
    cua_driver_core::mcp_result::conforming_proxy_tool_result(name, result, output_schema)
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The daemon-backed integration harness exercises the full proxy lifecycle.
// These tests lock in the I/O loop's transport contract and the per-branch
// response reshaping without requiring a live daemon.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_lifecycle_controls_do_not_refresh_real_use_timeout() {
        assert!(worker_method_counts_as_use("tools/call"));
        assert!(worker_method_counts_as_use("cua/driver/v1/open"));
        assert!(worker_method_counts_as_use("cua/driver/v1/exchange"));
        assert!(!worker_method_counts_as_use("cua/driver/v1/cancel"));
        assert!(!worker_method_counts_as_use("cua/driver/v1/close"));
        assert!(!worker_method_counts_as_use("ping"));
        assert!(!worker_method_counts_as_use("tools/list"));
    }

    #[test]
    fn envelope_multiplexer_rejects_ids_the_worker_rejects() {
        for id in [
            serde_json::json!("request"),
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(u64::MAX),
        ] {
            assert!(valid_worker_request_id(&id), "id={id}");
        }
        for id in [
            serde_json::json!(null),
            serde_json::json!(false),
            serde_json::json!(1.5),
            serde_json::json!({}),
            serde_json::json!([]),
        ] {
            assert!(!valid_worker_request_id(&id), "id={id}");
        }
    }

    #[tokio::test]
    async fn supervisor_forwards_envelope_cancel_and_close_during_exchange() {
        let (mut proxy_stdin, worker_stdin) = tokio::io::duplex(4096);
        let (mut worker_stdout, proxy_stdout) = tokio::io::duplex(4096);
        let (mut client_stdin, client_reader) = tokio::io::duplex(4096);
        let (client_writer, client_stdout) = tokio::io::duplex(4096);

        let worker = tokio::spawn(async move {
            let mut reader = BufReader::new(worker_stdin);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
                "action"
            );
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
                2
            );
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
                3
            );
            worker_stdout
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"ok\":true}}\n{\"jsonrpc\":\"2.0\",\"id\":\"action\",\"result\":{\"ok\":false}}\n",
                )
                .await
                .unwrap();
        });

        let exchange = tokio::spawn(async move {
            let mut reader = BufReader::new(client_reader).lines();
            let mut writer = tokio::io::BufWriter::new(client_writer);
            let mut worker_reader = BufReader::new(proxy_stdout);
            let mut output_buffer = Vec::new();
            let mut deferred = VecDeque::new();
            exchange_worker_request_with_controls(
                &mut proxy_stdin,
                &mut worker_reader,
                &mut output_buffer,
                r#"{"jsonrpc":"2.0","id":"action","method":"cua/driver/v1/exchange","params":{}}"#,
                &mut reader,
                &mut writer,
                &mut deferred,
                true,
            )
            .await
        });

        client_stdin
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"cua/driver/v1/cancel\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"cua/driver/v1/close\",\"params\":{}}\n",
            )
            .await
            .unwrap();

        let mut replies = BufReader::new(client_stdout);
        let mut line = String::new();
        replies.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
            2
        );
        line.clear();
        replies.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
            3
        );
        let action = exchange.await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&action).unwrap()["id"],
            "action"
        );
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn fractional_control_id_does_not_block_exchange_response() {
        let (mut proxy_stdin, worker_stdin) = tokio::io::duplex(4096);
        let (mut worker_stdout, proxy_stdout) = tokio::io::duplex(4096);
        let (mut client_stdin, client_reader) = tokio::io::duplex(4096);
        client_stdin
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1.5,\"method\":\"cua/driver/v1/cancel\",\"params\":{}}\n",
            )
            .await
            .unwrap();

        let worker = tokio::spawn(async move {
            let mut reader = BufReader::new(worker_stdin);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
                "action"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            worker_stdout
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"action\",\"result\":{\"ok\":true}}\n")
                .await
                .unwrap();
            line.clear();
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                reader.read_line(&mut line),
            )
            .await
            .expect("worker stdin should close after the action response")
            .unwrap()
        });

        let mut reader = BufReader::new(client_reader).lines();
        let mut writer = Vec::new();
        let mut worker_reader = BufReader::new(proxy_stdout);
        let mut output_buffer = Vec::new();
        let mut deferred = VecDeque::new();
        let action = exchange_worker_request_with_controls(
            &mut proxy_stdin,
            &mut worker_reader,
            &mut output_buffer,
            r#"{"jsonrpc":"2.0","id":"action","method":"cua/driver/v1/exchange","params":{}}"#,
            &mut reader,
            &mut writer,
            &mut deferred,
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&action).unwrap()["id"],
            "action"
        );
        assert!(
            writer.is_empty(),
            "a rejected ID must not produce a control response"
        );
        assert_eq!(deferred.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(deferred.front().unwrap()).unwrap()["id"],
            1.5
        );
        drop(proxy_stdin);
        assert_eq!(
            worker.await.unwrap(),
            0,
            "fractional control was not sent to the worker"
        );
    }

    #[tokio::test]
    async fn modern_proxy_metadata_and_rejections_need_no_daemon_call() {
        use serde_json::json;
        let meta = json!({
            mcp_wire::PROTOCOL_VERSION_KEY: mcp_wire::MODERN_PROTOCOL_VERSION,
            mcp_wire::CLIENT_CAPABILITIES_KEY: {}
        });
        let mut input = Vec::new();
        for (id, method, mut params) in [
            (1, "server/discover", json!({})),
            (2, "skills/list", json!({})),
            (
                3,
                "resources/read",
                json!({"uri":"skill://cua-driver/SKILL.md"}),
            ),
            (4, "tools/call", json!({"name":"get_config","arguments":[]})),
            (
                5,
                "tools/call",
                json!({"name":"unknown_tool","arguments":{"session":"must-not-start"}}),
            ),
        ] {
            params["_meta"] = meta.clone();
            serde_json::to_writer(
                &mut input,
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .unwrap();
            input.push(b'\n');
        }
        let inventory = Arc::new(json!({"tools":[{"name":"get_config"}]}));
        let mut output = Vec::new();
        run_proxy_io(
            BufReader::new(input.as_slice()),
            &mut output,
            "unreachable-test-endpoint",
            &inventory,
            "modern-test",
            false,
        )
        .await
        .unwrap();
        let replies: Vec<serde_json::Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 5);
        assert_eq!(
            replies[0]["result"]["supportedVersions"],
            json!(["2026-07-28"])
        );
        assert_eq!(
            replies[1]["result"]["skills"][0]["uri"],
            "skill://cua-driver/SKILL.md"
        );
        assert!(replies[2]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("---"));
        for reply in &replies[..3] {
            assert_eq!(reply["result"]["resultType"], "complete");
            assert_eq!(reply["result"]["cacheScope"], "private");
        }
        for reply in &replies[3..] {
            assert_eq!(reply["error"]["code"], -32602);
        }
    }

    #[tokio::test]
    async fn proxy_authorization_refusal_is_a_tool_error_without_forwarding() {
        const CHILD_ENV: &str = "CUA_DRIVER_PROXY_REFUSAL_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            // Policy configuration is process-wide and immutable; keep this
            // denial out of other tests without mutating the parent environment.
            let directory = tempfile::tempdir().expect("temporary policy directory");
            let policy = directory.path().join("deny.yaml");
            std::fs::write(&policy, "deny:\n  tools: [click]\n").expect("write policy");
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "proxy::tests::proxy_authorization_refusal_is_a_tool_error_without_forwarding",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .env(cua_driver_core::policy::POLICY_FILE_ENV, &policy)
                .env_remove(cua_driver_core::policy::MANAGED_POLICY_FILE_ENV)
                .output()
                .expect("run isolated refusal test");
            assert!(
                output.status.success(),
                "isolated refusal test failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let directory = tempfile::tempdir().expect("temporary socket directory");
        let socket = directory.path().join("daemon.sock");
        #[cfg(unix)]
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind test daemon");
        let request = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": 19, "method": "tools/call",
            "params": {"name": "click", "arguments": {"pid": 42, "x": 10, "y": 20}}
        }))
        .expect("valid tool request");
        let cached_tools = Arc::new(serde_json::json!({"tools": [{"name": "click"}]}));
        let response = handle_proxy_request(
            request,
            serde_json::json!(19),
            socket.to_str().unwrap(),
            &cached_tools,
            "refusal-test-session",
            false,
        );
        #[cfg(unix)]
        let response = tokio::select! {
            response = response => response,
            connection = listener.accept() => {
                drop(connection);
                panic!("a locally denied call must never connect to the daemon");
            }
        };
        #[cfg(not(unix))]
        let response = response.await;
        let value = serde_json::to_value(response).expect("serialize response");
        assert_eq!(value["id"], 19);
        assert!(value.get("error").is_none(), "{value}");
        let result = &value["result"];
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["code"], "permission_denied");
        assert_eq!(result["content"][0]["type"], "text");
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("explicitly denied"));
    }

    #[test]
    fn control_disconnect_exits_with_stdin_open() {
        const CHILD_ENV: &str = "CUA_TEST_MCP_CONTROL_STDIN_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let result = crate::cli::run_mcp_runtime(async {
                let cached_tools = Arc::new(serde_json::json!({"tools": []}));
                supervise_control_connection(
                    // Let run_proxy_io park in the real blocking stdin read.
                    tokio::time::sleep(std::time::Duration::from_millis(100)),
                    run_proxy_io(
                        BufReader::new(tokio::io::stdin()),
                        tokio::io::sink(),
                        "unused.sock",
                        &cached_tools,
                        "disconnect",
                        false,
                    ),
                )
                .await
            });
            assert!(result.is_err());
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "proxy::tests::control_disconnect_exits_with_stdin_open",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "MCP runtime subprocess failed: {:?}",
                    child.wait_with_output().unwrap()
                );
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("control disconnect left the MCP process waiting for client stdin EOF");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[tokio::test]
    async fn control_disconnect_ends_idle_proxy() {
        let (control, daemon) = tokio::io::duplex(64);
        let control = async move {
            let mut reader = BufReader::new(control);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await;
        };
        let proxy =
            supervise_control_connection(control, std::future::pending::<anyhow::Result<()>>());
        tokio::pin!(proxy);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut proxy)
                .await
                .is_err(),
            "an idle live control connection must remain usable"
        );
        drop(daemon);
        let error = tokio::time::timeout(std::time::Duration::from_millis(250), proxy)
            .await
            .expect("control EOF must end the proxy promptly")
            .unwrap_err();
        assert!(error.to_string().contains("reconnect the MCP client"));
    }

    #[tokio::test]
    async fn proxy_eof_drops_its_control_connection() {
        let (control, daemon) = tokio::io::duplex(64);
        let control = async move {
            let mut reader = BufReader::new(control);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await;
        };
        let cached_tools = Arc::new(serde_json::json!({"tools": []}));
        let mut writer = Vec::new();
        supervise_control_connection(
            control,
            run_proxy_io(
                BufReader::new(&b""[..]),
                &mut writer,
                "unused.sock",
                &cached_tools,
                "eof",
                false,
            ),
        )
        .await
        .unwrap();
        let mut daemon = BufReader::new(daemon);
        let mut line = String::new();
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                daemon.read_line(&mut line)
            )
            .await
            .expect("proxy EOF must release daemon session ownership")
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn closed_control_prevents_ready_request_dispatch() {
        let dispatched = std::sync::atomic::AtomicBool::new(false);
        let result = supervise_control_connection(async {}, async {
            dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(result.is_err());
        assert!(!dispatched.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn proxy_loop_returns_promptly_on_clean_eof() {
        let reader = BufReader::new(&b""[..]);
        let mut writer = Vec::new();
        let cached_tools = Arc::new(serde_json::json!({"tools": []}));

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            run_proxy_io(
                reader,
                &mut writer,
                "unused.sock",
                &cached_tools,
                "eof-test-session",
                false,
            ),
        )
        .await
        .expect("clean EOF must return promptly");

        assert!(result.is_ok(), "clean EOF must not error: {result:?}");
        assert!(writer.is_empty(), "no request means no output");
    }

    #[tokio::test]
    async fn proxy_loop_serves_initialize_before_eof() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
        let reader = BufReader::new(&input[..]);
        let mut writer = Vec::new();
        let cached_tools = Arc::new(serde_json::json!({"tools": []}));

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            run_proxy_io(
                reader,
                &mut writer,
                "unused.sock",
                &cached_tools,
                "initialize-test-session",
                false,
            ),
        )
        .await
        .expect("initialize followed by EOF must return promptly");

        assert!(
            result.is_ok(),
            "initialize exchange must not error: {result:?}"
        );
        let response: serde_json::Value =
            serde_json::from_slice(&writer).expect("response must be JSON");
        assert_eq!(response["id"], 1);
        assert!(response.get("result").is_some());
    }

    #[test]
    fn session_begin_requires_a_positive_typed_acknowledgement() {
        assert!(
            validate_session_begin_ack(r#"{"ok":true,"result":{"session_begin":true}}"#).is_ok()
        );
        for line in [
            "not JSON",
            r#"{"ok":false,"error":"denied"}"#,
            r#"{"ok":false,"result":{"session_begin":true}}"#,
            r#"{"ok":true}"#,
            r#"{"ok":true,"result":{"session_begin":false}}"#,
            r#"{"ok":true,"result":{"session_begin":"true"}}"#,
            r#"{"ok":true,"result":{"other":true}}"#,
        ] {
            assert!(validate_session_begin_ack(line).is_err(), "accepted {line}");
        }
    }

    #[test]
    fn incompatible_daemon_schema_is_rejected_before_advertising() {
        let schema = serde_json::json!({"type": "object", "required": ["legacy_clicked"]});
        let tools = vec![serde_json::json!({"name": "click", "output_schema": schema})];
        let error = validate_daemon_output_schemas(&tools)
            .unwrap_err()
            .to_string();
        assert!(error.contains("incompatible daemon output schema for click"));
        assert!(error.contains("matching proxy and daemon versions"));

        let daemon_resp = DaemonResponse {
            ok: true,
            result: Some(serde_json::json!({
                "content": [], "structuredContent": {"legacy_clicked": true}
            })),
            error: None,
            exit_code: None,
        };
        let response = proxy_tool_result_response(
            serde_json::json!(1),
            daemon_response_to_tool_result("click", daemon_resp, Some(&schema)),
        );
        let response = serde_json::to_value(response).unwrap();
        assert_eq!(response["error"]["code"], -32603);
        assert!(response.get("result").is_none());
    }

    #[test]
    fn daemon_without_schema_preserves_legacy_success() {
        let tools = vec![serde_json::json!({"name": "click"})];
        validate_daemon_output_schemas(&tools).unwrap();
        for result in [
            serde_json::json!({"content": [], "isError": false}),
            serde_json::json!({
                "content": [], "isError": false,
                "structuredContent": {"legacy_clicked": true}
            }),
        ] {
            let daemon_resp = DaemonResponse {
                ok: true,
                result: Some(result.clone()),
                error: None,
                exit_code: None,
            };
            assert_eq!(
                daemon_response_to_tool_result("click", daemon_resp, None).unwrap(),
                result
            );
        }
    }

    #[test]
    fn matching_daemon_schema_retains_local_output_validation() {
        let tools =
            vec![serde_json::json!({"name": "click", "output_schema": click_output_schema()})];
        validate_daemon_output_schemas(&tools).unwrap();
    }

    fn click_output_schema() -> Option<serde_json::Value> {
        cua_driver_contract::advertised_tool_output_schema("click")
    }

    /// Serialize the production conversion the way the proxy answers it, so
    /// these assertions run against `forward_tool_call`'s own branch without
    /// spinning up a real daemon.
    fn tool_call_response(id: serde_json::Value, resp: DaemonResponse) -> Response {
        Response::ok(
            id,
            daemon_response_to_tool_result("click", resp, click_output_schema().as_ref()).unwrap(),
        )
    }

    #[test]
    fn daemon_tool_failure_wraps_as_jsonrpc_success_with_iserror_true() {
        let daemon_resp = DaemonResponse {
            ok: false,
            result: None,
            error: Some("missing required field `pid`".into()),
            exit_code: Some(64),
        };
        let resp = tool_call_response(serde_json::json!(7), daemon_resp);
        let value = serde_json::to_value(&resp).expect("serialize");

        // Top-level JSON-RPC envelope: success (`result`), not error.
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], serde_json::json!(7));
        assert!(
            value.get("error").is_none(),
            "tool-level failure must NOT surface as JSON-RPC error: got {value}"
        );
        assert!(
            value.get("result").is_some(),
            "tool-level failure must carry a `result` payload: got {value}"
        );

        // CallTool.Result inside `result`: isError + content text.
        let result = &value["result"];
        assert_eq!(result["isError"], serde_json::json!(true));
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "missing required field `pid`");
        assert_eq!(result["structuredContent"]["exit_code"], 64);
        // Strict MCP clients validate this payload against the tool's
        // advertised `outputSchema`. The refusal marker is what keeps the
        // response readable instead of being discarded as `-32602`.
        assert_eq!(
            result["structuredContent"]["code"],
            cua_driver_core::TOOL_INVOCATION_FAILED_CODE
        );
    }

    #[test]
    fn daemon_failure_with_no_error_message_uses_fallback_text() {
        let daemon_resp = DaemonResponse {
            ok: false,
            result: None,
            error: None,
            exit_code: None,
        };
        let resp = tool_call_response(serde_json::json!("abc"), daemon_resp);
        let value = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(value["result"]["isError"], serde_json::json!(true));
        assert_eq!(
            value["result"]["content"][0]["text"],
            "daemon reported failure"
        );
        assert_eq!(value["result"]["structuredContent"]["exit_code"], 1);
        assert_eq!(
            value["result"]["structuredContent"]["code"],
            cua_driver_core::TOOL_INVOCATION_FAILED_CODE
        );
    }

    /// An `ok` daemon response that carries no result used to be forwarded as
    /// an empty success. For a tool that declares an `outputSchema` that is a
    /// response a strict client rejects outright (`-32600`), so the boundary
    /// converts it into an error the client can actually read.
    #[test]
    fn an_ok_daemon_response_with_no_result_becomes_a_conforming_error() {
        let daemon_resp = DaemonResponse {
            ok: true,
            result: None,
            error: None,
            exit_code: None,
        };

        let result =
            daemon_response_to_tool_result("click", daemon_resp, click_output_schema().as_ref())
                .unwrap();

        assert_eq!(result["isError"], serde_json::json!(true));
        assert_eq!(
            result["structuredContent"]["code"],
            cua_driver_core::mcp_result::TOOL_OUTPUT_INVALID_CODE
        );
    }

    /// A daemon result that already conforms is forwarded untouched.
    #[test]
    fn a_conforming_daemon_success_is_forwarded_unchanged() {
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": "clicked" }],
            "isError": false,
            "structuredContent": {
                "effect": "confirmed",
                "route": "accessibility",
                "delivery": { "mode": "background" },
                "evidence": [{ "kind": "value_readback" }],
            },
        });
        let daemon_resp = DaemonResponse {
            ok: true,
            result: Some(result.clone()),
            error: None,
            exit_code: None,
        };

        assert_eq!(
            daemon_response_to_tool_result("click", daemon_resp, click_output_schema().as_ref())
                .unwrap(),
            result
        );
    }

    #[test]
    fn cached_proxy_tool_allowlist_is_exact() {
        let cached = serde_json::json!({
            "tools": [
                {"name":"click"},
                {"name":"type_text"}
            ]
        });
        assert!(proxy_knows_tool(&cached, "click"));
        assert!(
            proxy_knows_tool(&cached, "type_text_chars"),
            "deprecated alias stays bounded/known"
        );
        assert!(!proxy_knows_tool(&cached, "click/private-user-value"));
        assert!(!proxy_knows_tool(&cached, ""));
    }

    #[test]
    fn observation_ownership_requires_the_daemon_capability() {
        assert!(daemon_owns_tool_observation(&serde_json::json!({
            "tool_observation_owner": "daemon"
        })));
        assert!(!daemon_owns_tool_observation(&serde_json::json!({})));
        assert!(!daemon_owns_tool_observation(&serde_json::json!({
            "tool_observation_owner": "proxy"
        })));
    }
}
