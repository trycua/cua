//! Opt-in typed envelopes over an existing MCP transport, never an agent tool.
//!
//! One service registry belongs to one accepted transport. The existing receiver
//! owns generations, request ledgers, cancellation, permissions and session close.
//! No listener, credential issuer, action replay or runtime fallback is added.

use crate::driver_service_http::{Request as ServiceRequest, Service};
use cua_driver_core::protocol::{initialize_result, Request, Response};
use cua_driver_core::server::{
    observe_proxy_session_started, observe_proxy_tool_completed, tool_observation_timer,
    StdioExecutionPath,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

pub(crate) const CAPABILITY: &str = "ai.cua.driver.envelopes";
const PREFIX: &str = "cua/driver/v1/";
const MAX_LINE: usize = 1024 * 1024;
const MAX_PENDING: usize = 32;

pub(crate) fn configured() -> anyhow::Result<bool> {
    match std::env::var("CUA_DRIVER_MCP_ENVELOPES") {
        Err(std::env::VarError::NotPresent) => Ok(false),
        Ok(value) if value == "0" => Ok(false),
        Ok(value) if value == "1" => Ok(true),
        _ => anyhow::bail!("CUA_DRIVER_MCP_ENVELOPES must be 0 or 1"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    connection_id: String,
    generation: String,
    #[serde(default)]
    envelope: Option<Value>,
    #[serde(default)]
    request_id: Option<String>,
}

fn service_request(request: &Request) -> Result<ServiceRequest, &'static str> {
    let params = request.params.clone().ok_or("parameters_required")?;
    if request.method == format!("{PREFIX}open") {
        if params != json!({}) {
            return Err("invalid_open_parameters");
        }
        return Ok(ServiceRequest {
            method: "POST".into(),
            path: "/v1/connections".into(),
            generation: None,
            body: b"{}".to_vec(),
        });
    }
    let fields = params.as_object().ok_or("invalid_binding")?;
    let allowed = match request.method.strip_prefix(PREFIX) {
        Some("exchange") => &["connection_id", "generation", "envelope"][..],
        Some("cancel") => &["connection_id", "generation", "request_id"][..],
        Some("close") => &["connection_id", "generation"][..],
        _ => return Err("unknown_operation"),
    };
    if fields
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        return Err("invalid_operation_parameters");
    }
    let binding: Binding = serde_json::from_value(params).map_err(|_| "invalid_binding")?;
    if uuid::Uuid::parse_str(&binding.connection_id).is_err()
        || uuid::Uuid::parse_str(&binding.generation).is_err()
    {
        return Err("invalid_binding");
    }
    let base = format!("/v1/connections/{}", binding.connection_id);
    let (method, path, body) = match request.method.strip_prefix(PREFIX) {
        Some("exchange") if binding.request_id.is_none() => (
            "POST",
            format!("{base}/exchange"),
            binding.envelope.ok_or("envelope_required")?,
        ),
        Some("cancel") if binding.envelope.is_none() => (
            "POST",
            format!("{base}/cancel"),
            json!({"request_id": binding.request_id.ok_or("request_id_required")?}),
        ),
        Some("close") if binding.envelope.is_none() && binding.request_id.is_none() => {
            ("DELETE", base, json!({}))
        }
        _ => return Err("invalid_operation_parameters"),
    };
    Ok(ServiceRequest {
        method: method.into(),
        path,
        generation: Some(binding.generation),
        body: body.to_string().into_bytes(),
    })
}

async fn typed_request(service: Arc<Service>, request: Request) -> Response {
    let id = request.id.clone().unwrap_or(Value::Null);
    let route = match service_request(&request) {
        Ok(route) => route,
        Err(reason) => return Response::error(id, -32602, reason),
    };
    match service.route(route).await {
        Ok(result) => Response::ok(id, result),
        Err((status, reason)) => Response::error(id, -32000 - i64::from(status), reason),
    }
}

struct Cleanup {
    service: Arc<Service>,
    sdk: Arc<crate::sdk_adapter::SdkAdapter>,
    session: String,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.service.close_all();
        self.sdk.end_transport_sessions(&self.session);
    }
}

/// Shared direct-stdio / daemon-hosted stream. EOF closes owned sessions only.
pub(crate) async fn run<R, W>(
    sdk: Arc<crate::sdk_adapter::SdkAdapter>,
    mut reader: R,
    writer: W,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let service = Service::for_sdk(sdk.clone())?;
    run_with_service(sdk, service, &mut reader, writer).await
}

async fn run_with_service<R, W>(
    sdk: Arc<crate::sdk_adapter::SdkAdapter>,
    service: Arc<Service>,
    reader: &mut R,
    writer: W,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let session = format!("mcp-typed-{}", uuid::Uuid::new_v4());
    // Declared first so receiver invalidation precedes task abort on every exit,
    // including cancellation of the enclosing transport future.
    let mut tasks = tokio::task::JoinSet::new();
    let cleanup = Cleanup {
        service: service.clone(),
        sdk: sdk.clone(),
        session: session.clone(),
    };
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let mut line = Vec::new();
    let legacy_serial = Arc::new(tokio::sync::Mutex::new(()));
    let mut pending = HashSet::new();
    let mut reaper = tokio::time::interval(Duration::from_secs(30));
    let mut session_observed = false;
    loop {
        // read_until is cancellation-safe. Keep partial bytes across task and
        // reaper wakeups, and retain the original total line budget.
        let mut bounded_reader = (&mut *reader).take((MAX_LINE + 1 - line.len()) as u64);
        let n = tokio::select! {
            read = bounded_reader.read_until(b'\n', &mut line) => read?,
            done = tasks.join_next(), if !tasks.is_empty() => {
                let id = done.expect("nonempty task set")??;
                pending.remove(&id);
                continue;
            }
            _ = reaper.tick() => {
                service.reap();
                continue;
            }
        };
        if n == 0 {
            break;
        }
        anyhow::ensure!(
            line.len() <= MAX_LINE,
            "MCP envelope request exceeds size limit"
        );
        let raw: Value = match serde_json::from_slice(&line) {
            Ok(raw) => raw,
            Err(_) => {
                line.clear();
                write_response(&writer, Response::parse_error()).await?;
                continue;
            }
        };
        line.clear();
        let valid_id = raw
            .get("id")
            .is_none_or(|id| id.is_string() || id.as_i64().is_some() || id.as_u64().is_some());
        let request: Request = match serde_json::from_value::<Request>(raw) {
            Ok(request) if valid_id && request.jsonrpc == "2.0" => request,
            _ => {
                write_response(
                    &writer,
                    Response::error(Value::Null, -32600, "invalid_request"),
                )
                .await?;
                continue;
            }
        };
        // Typed control methods require acknowledged requests. Ordinary MCP
        // notifications retain the legacy behavior; no cancellation is invented.
        if request.is_notification() {
            continue;
        }
        let id = request.id.clone().unwrap_or(Value::Null);
        let key = id.to_string();
        // Responding to a duplicate active ID would itself be ambiguous. End
        // this transport rather than miscorrelating an action or replaying it.
        anyhow::ensure!(!pending.contains(&key), "duplicate active MCP request ID");
        if request.method == "initialize" {
            let mut result = initialize_result();
            result["capabilities"]["experimental"][CAPABILITY] = json!({"version": 1});
            if !session_observed {
                if let Some(metadata) = request.initialize_metadata() {
                    observe_proxy_session_started(metadata);
                    session_observed = true;
                }
            }
            write_response(&writer, Response::ok(id, result)).await?;
            continue;
        }
        if request.method.starts_with(PREFIX) {
            // Open/cancel/close never wait behind a native action. The receiver
            // bounds exchange concurrency independently from these controls.
            if request.method != format!("{PREFIX}exchange") {
                write_response(&writer, typed_request(service.clone(), request).await).await?;
                continue;
            }
        }
        if tasks.len() >= MAX_PENDING {
            write_response(&writer, Response::error(id, -32029, "in_flight_limit")).await?;
            continue;
        }
        let service = service.clone();
        let writer = writer.clone();
        let sdk = sdk.clone();
        let session = session.clone();
        let serial = legacy_serial.clone();
        pending.insert(key.clone());
        tasks.spawn(async move {
            let response = if request.method.starts_with(PREFIX) {
                typed_request(service, request).await
            } else {
                let _guard = serial.lock().await;
                let mut request = request;
                crate::proxy::apply_direct_session_identity(&mut request, &session);
                let context = request.tool_call().ok().and_then(|call| {
                    sdk.begin_tool_call(
                        &call.name,
                        &call.args,
                        cua_driver_core::session::SessionTransport::McpStdio,
                        cua_driver_core::session::SessionClientKind::Mcp,
                    )
                });
                let timer = tool_observation_timer(
                    &request,
                    |name| sdk.is_known_tool(name),
                    StdioExecutionPath::DirectDaemon,
                );
                let response = cua_driver_core::server::handle_request_with_transport_session(
                    request,
                    id,
                    sdk.as_ref(),
                    &session,
                )
                .await;
                if let Some(timer) = timer {
                    let outcome = timer.finish(&response);
                    if let Some(context) = context {
                        context.complete(&outcome);
                    }
                    observe_proxy_tool_completed(outcome);
                }
                response
            };
            write_response(&writer, response).await?;
            Ok::<_, anyhow::Error>(key)
        });
    }
    // Close before abort: the receiver invalidates in-flight and queued work.
    // Dropping JoinSet also aborts on malformed input or broken output.
    drop(cleanup);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

async fn write_response<W: AsyncWrite + Unpin>(
    writer: &Arc<tokio::sync::Mutex<W>>,
    response: Response,
) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut writer = writer.lock().await;
        writer.write_all(&bytes).await?;
        writer.flush().await
    })
    .await??;
    Ok(())
}

/// Called only after the daemon's existing local-peer authentication succeeds.
pub(crate) async fn accept<R, W>(
    sdk: Arc<crate::sdk_adapter::SdkAdapter>,
    reader: R,
    mut writer: W,
) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let service = Service::for_sdk(sdk.clone());
    let enabled = configured()?;
    let acknowledgement = if enabled && service.is_ok() {
        crate::serve::DaemonResponse::ok(json!({"mcp_envelope_stream": 1}))
    } else {
        crate::serve::DaemonResponse::err("MCP envelope stream unavailable", 77)
    };
    writer
        .write_all((serde_json::to_string(&acknowledgement)? + "\n").as_bytes())
        .await?;
    writer.flush().await?;
    anyhow::ensure!(enabled, "MCP envelope stream is disabled");
    run_with_service(sdk, service?, &mut { reader }, writer).await
}

pub(crate) async fn proxy(socket_path: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(socket_path).await?;
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(socket_path)?;
    relay(stream).await
}

async fn relay<S: AsyncRead + AsyncWrite + Unpin>(stream: S) -> anyhow::Result<()> {
    relay_io(stream, tokio::io::stdin(), tokio::io::stdout()).await
}

async fn relay_io<S, I, O>(stream: S, mut stdin: I, mut stdout: O) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    writer
        .write_all(b"{\"method\":\"mcp_envelope_stream\"}\n")
        .await?;
    writer.flush().await?;
    let mut ack = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(4),
        (&mut reader).take(16385).read_until(b'\n', &mut ack),
    )
    .await??;
    anyhow::ensure!(ack.len() <= 16384, "MCP envelope handshake exceeds limit");
    let ack: Value = serde_json::from_slice(&ack)
        .map_err(|_| anyhow::anyhow!("Invalid MCP envelope handshake"))?;
    anyhow::ensure!(
        ack["ok"] == true && ack["result"]["mcp_envelope_stream"] == 1,
        "Selected daemon does not support MCP envelopes"
    );
    // No reconnect: daemon replacement closes this transport and its handles.
    tokio::select! {
        result = tokio::io::copy(&mut stdin, &mut writer) => { result?; writer.shutdown().await?; }
        result = tokio::io::copy(&mut reader, &mut stdout) => { result?; anyhow::bail!("MCP envelope daemon connection ended"); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_sdk::remote_receiver::{DriverEnvelopeExecutor, DriverEnvelopeReceiver};
    use cua_driver_sdk::DriverError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

    fn request(method: &str, params: Value) -> Request {
        serde_json::from_value(
            json!({"jsonrpc":"2.0","id":1,"method":format!("{PREFIX}{method}"),"params":params}),
        )
        .unwrap()
    }

    fn binding() -> Value {
        json!({"connection_id":uuid::Uuid::new_v4().to_string(),"generation":uuid::Uuid::new_v4().to_string()})
    }

    #[test]
    fn strict_operation_parameters_never_accept_authority_or_paths() {
        assert!(service_request(&request("open", json!({}))).is_ok());
        for invalid in [
            Value::Null,
            json!([]),
            json!({"permission_mode":"unrestricted"}),
            json!({"session":"other"}),
        ] {
            assert!(service_request(&request("open", invalid)).is_err());
        }
        for operation in ["exchange", "cancel", "close"] {
            let mut valid = binding();
            if operation == "exchange" {
                valid["envelope"] = json!({});
            }
            if operation == "cancel" {
                valid["request_id"] = json!("r1");
            }
            assert!(service_request(&request(operation, valid.clone())).is_ok());
            for field in ["path", "session", "permission_mode", "extra"] {
                let mut invalid = valid.clone();
                invalid[field] = Value::Null;
                assert!(service_request(&request(operation, invalid)).is_err());
            }
            valid["connection_id"] = json!("../other");
            assert!(service_request(&request(operation, valid)).is_err());
        }
        let mut invalid = binding();
        invalid["envelope"] = Value::Null;
        assert!(service_request(&request("close", invalid)).is_err());
        assert!(service_request(&request("unknown", binding())).is_err());
    }

    #[test]
    fn extension_requires_explicit_launcher_opt_in() {
        const CHILD: &str = "CUA_TEST_MCP_ENVELOPE_OPT_IN";
        if let Ok(expected) = std::env::var(CHILD) {
            match expected.as_str() {
                "on" => assert!(configured().unwrap()),
                "off" => assert!(!configured().unwrap()),
                "error" => assert!(configured().is_err()),
                _ => panic!("invalid synthetic expectation"),
            }
            return;
        }
        for (value, expected) in [
            (None, "off"),
            (Some("0"), "off"),
            (Some("1"), "on"),
            (Some("true"), "error"),
            (Some(""), "error"),
        ] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "mcp_envelope::tests::extension_requires_explicit_launcher_opt_in",
                ])
                .env(CHILD, expected)
                .env_remove("CUA_DRIVER_MCP_ENVELOPES");
            if let Some(value) = value {
                child.env("CUA_DRIVER_MCP_ENVELOPES", value);
            }
            assert!(child.output().unwrap().status.success());
        }
    }

    struct Fake {
        closed: Arc<AtomicUsize>,
        dispatched: Arc<AtomicUsize>,
        slow: bool,
    }
    #[async_trait::async_trait]
    impl DriverEnvelopeExecutor for Fake {
        async fn metadata(&self) -> Result<Value, DriverError> {
            self.dispatched.fetch_add(1, Ordering::SeqCst);
            if self.slow {
                std::future::pending().await
            } else {
                Ok(json!({"probe":true}))
            }
        }
        async fn list_tools(&self) -> Result<Value, DriverError> {
            Ok(json!({"tools":[]}))
        }
        async fn call(&self, _: String, _: Value) -> Result<Value, DriverError> {
            unreachable!()
        }
        fn close(&self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn service(slow: bool) -> (Arc<Service>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let closed = Arc::new(AtomicUsize::new(0));
        let dispatched = Arc::new(AtomicUsize::new(0));
        let c = closed.clone();
        let d = dispatched.clone();
        (
            Service::for_test(Arc::new(move || {
                Ok((
                    DriverEnvelopeReceiver::new(Arc::new(Fake {
                        closed: c.clone(),
                        dispatched: d.clone(),
                        slow,
                    })),
                    "synthetic-session".into(),
                ))
            })),
            closed,
            dispatched,
        )
    }

    fn envelope(id: &str) -> Value {
        json!({"envelope_version":1,"request_id":id,"operation":"metadata","deadline_unix_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()+60000})
    }

    #[tokio::test]
    async fn real_receiver_rejects_foreign_generation_and_duplicate_action() {
        let (first, closed, dispatched) = service(false);
        let (second, _, _) = service(false);
        let opened =
            serde_json::to_value(typed_request(first.clone(), request("open", json!({}))).await)
                .unwrap();
        let mut params = opened["result"].clone();
        params
            .as_object_mut()
            .unwrap()
            .retain(|key, _| key == "connection_id" || key == "generation");
        let foreign =
            serde_json::to_value(typed_request(second, request("close", params.clone())).await)
                .unwrap();
        assert_eq!(foreign["error"]["code"], -32404);
        let mut stale = params.clone();
        stale["generation"] = json!(uuid::Uuid::new_v4().to_string());
        let stale =
            serde_json::to_value(typed_request(first.clone(), request("close", stale)).await)
                .unwrap();
        assert_eq!(stale["error"]["code"], -32409);
        params["envelope"] = envelope("same-action");
        let success = serde_json::to_value(
            typed_request(first.clone(), request("exchange", params.clone())).await,
        )
        .unwrap();
        assert_eq!(success["result"]["ok"], true);
        let duplicate =
            serde_json::to_value(typed_request(first.clone(), request("exchange", params)).await)
                .unwrap();
        assert_eq!(duplicate["result"]["ok"], false);
        assert_eq!(dispatched.load(Ordering::SeqCst), 1);
        first.close_all();
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn early_cancel_and_connection_limit_use_the_receiver_ledger() {
        let (service, _, dispatched) = service(false);
        let opened =
            serde_json::to_value(typed_request(service.clone(), request("open", json!({}))).await)
                .unwrap();
        let mut params = opened["result"].clone();
        params
            .as_object_mut()
            .unwrap()
            .retain(|key, _| key == "connection_id" || key == "generation");
        let mut cancel = params.clone();
        cancel["request_id"] = json!("late-arrival");
        let response =
            serde_json::to_value(typed_request(service.clone(), request("cancel", cancel)).await)
                .unwrap();
        assert_eq!(response["result"]["ok"], true);
        params["envelope"] = envelope("late-arrival");
        let response =
            serde_json::to_value(typed_request(service.clone(), request("exchange", params)).await)
                .unwrap();
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(dispatched.load(Ordering::SeqCst), 0);
        for _ in 1..64 {
            let response = serde_json::to_value(
                typed_request(service.clone(), request("open", json!({}))).await,
            )
            .unwrap();
            assert!(response.get("result").is_some());
        }
        let response =
            serde_json::to_value(typed_request(service.clone(), request("open", json!({}))).await)
                .unwrap();
        assert_eq!(response["error"]["code"], -32503);
    }

    async fn sdk() -> Arc<crate::sdk_adapter::SdkAdapter> {
        crate::sdk_adapter::SdkAdapter::load(cua_driver_sdk::CuaDriver::create_for_host(
            cua_driver_sdk::DriverHostOptions {
                cursor: cursor_overlay::CursorConfig {
                    enabled: false,
                    ..Default::default()
                },
                host_owns_permission_ux: false,
                host_bundle_id: None,
                claude_code_compatibility: false,
                prepare_desktop_environment: false,
                register_host_tools: None,
                authorization_host: None,
                activity_observer: None,
            },
        ))
        .await
        .unwrap()
    }

    struct Client {
        read: BufReader<ReadHalf<DuplexStream>>,
        write: WriteHalf<DuplexStream>,
    }
    impl Client {
        async fn send(&mut self, id: u64, method: &str, params: Value) {
            self.write
                .write_all(
                    (json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string()
                        + "\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
        }
        async fn receive(&mut self) -> Value {
            let mut line = String::new();
            let n = tokio::time::timeout(Duration::from_secs(2), self.read.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            assert_ne!(n, 0, "unexpected EOF");
            serde_json::from_str(&line).unwrap()
        }
        async fn open(&mut self) -> Value {
            self.send(1, &format!("{PREFIX}open"), json!({})).await;
            let mut result = self.receive().await["result"].clone();
            result
                .as_object_mut()
                .unwrap()
                .retain(|key, _| key == "connection_id" || key == "generation");
            result
        }
    }
    fn start(
        sdk: Arc<crate::sdk_adapter::SdkAdapter>,
        service: Arc<Service>,
    ) -> (Client, tokio::task::JoinHandle<anyhow::Result<()>>) {
        let (client, server) = tokio::io::duplex(1024 * 1024);
        let (read, write) = tokio::io::split(client);
        let task = tokio::spawn(async move {
            let (read, write) = tokio::io::split(server);
            run_with_service(sdk, service, &mut BufReader::new(read), write).await
        });
        (
            Client {
                read: BufReader::new(read),
                write,
            },
            task,
        )
    }

    #[tokio::test]
    async fn stream_preserves_legacy_tools_and_closes_on_eof() {
        let _guard = crate::test_runtime_lock().lock().await;
        let sdk = sdk().await;
        let (service, closed, _) = service(false);
        let (mut client, task) = start(sdk.clone(), service);
        client.send(1, "initialize", json!({})).await;
        assert_eq!(
            client.receive().await["result"]["capabilities"]["experimental"][CAPABILITY]["version"],
            1
        );
        client.send(2, "tools/list", json!({})).await;
        let listed = client.receive().await;
        assert!(listed["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()));
        assert!(!listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"].as_str().unwrap().starts_with(PREFIX)));
        client.open().await;
        client.write.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        sdk.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn saturated_stream_keeps_acknowledged_cancel_and_close_responsive() {
        let _guard = crate::test_runtime_lock().lock().await;
        let sdk = sdk().await;
        let (service, closed, dispatched) = service(true);
        let (mut client, task) = start(sdk.clone(), service);
        let connection = client.open().await;
        for i in 0..MAX_PENDING {
            let mut params = connection.clone();
            params["envelope"] = envelope(&format!("action-{i}"));
            client
                .send(100 + i as u64, &format!("{PREFIX}exchange"), params)
                .await;
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while dispatched.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut params = connection.clone();
        params["envelope"] = envelope("overflow");
        client.send(200, &format!("{PREFIX}exchange"), params).await;
        let limit = client.receive().await;
        assert_eq!(limit["id"], 200);
        assert_eq!(limit["error"]["code"], -32029);
        let mut cancel = connection.clone();
        cancel["request_id"] = json!("action-0");
        client.send(201, &format!("{PREFIX}cancel"), cancel).await;
        client
            .send(202, &format!("{PREFIX}close"), connection)
            .await;
        let mut responses = Vec::new();
        for _ in 0..MAX_PENDING + 2 {
            responses.push(client.receive().await);
        }
        for id in [201, 202] {
            assert_eq!(
                responses.iter().find(|r| r["id"] == id).unwrap()["result"]["ok"],
                true
            );
        }
        assert_eq!(
            dispatched.load(Ordering::SeqCst),
            1,
            "queued actions must not execute"
        );
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        client.write.shutdown().await.unwrap();
        task.await.unwrap().unwrap();
        sdk.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_active_rpc_id_ends_stream_without_replay() {
        let _guard = crate::test_runtime_lock().lock().await;
        let sdk = sdk().await;
        let (service, closed, dispatched) = service(true);
        let (mut client, task) = start(sdk.clone(), service);
        let mut params = client.open().await;
        params["envelope"] = envelope("pending");
        client.send(2, &format!("{PREFIX}exchange"), params).await;
        client.send(2, "ping", json!({})).await;
        assert!(tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        assert!(dispatched.load(Ordering::SeqCst) <= 1);
        sdk.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn abort_and_oversized_input_close_owned_receivers() {
        let _guard = crate::test_runtime_lock().lock().await;
        let sdk = sdk().await;
        for abort in [true, false] {
            let (service, closed, _) = service(true);
            let (mut client, task) = start(sdk.clone(), service);
            let mut params = client.open().await;
            params["envelope"] = envelope("in-flight");
            client.send(2, &format!("{PREFIX}exchange"), params).await;
            if abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                let _ = client.write.write_all(&vec![b'x'; MAX_LINE + 1]).await;
                assert!(tokio::time::timeout(Duration::from_secs(2), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err());
            }
            assert_eq!(closed.load(Ordering::SeqCst), 1);
        }
        sdk.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_jsonrpc_shape_never_allocates_a_receiver() {
        let _guard = crate::test_runtime_lock().lock().await;
        let sdk = sdk().await;
        let (service, closed, _) = service(false);
        let (mut client, task) = start(sdk.clone(), service);
        for invalid in [
            json!({"jsonrpc":"1.0","id":1,"method":format!("{PREFIX}open")}),
            json!({"jsonrpc":"2.0","id":[],"method":format!("{PREFIX}open")}),
            json!({"jsonrpc":"2.0","id":null,"method":format!("{PREFIX}open")}),
        ] {
            client
                .write
                .write_all((invalid.to_string() + "\n").as_bytes())
                .await
                .unwrap();
            assert_eq!(client.receive().await["error"]["code"], -32600);
        }
        client.write.shutdown().await.unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(closed.load(Ordering::SeqCst), 0);
        sdk.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn relay_refuses_old_daemon_without_forwarding_actions() {
        let (client, server) = tokio::io::duplex(4096);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut read = BufReader::new(read);
            let mut line = String::new();
            read.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["method"],
                "mcp_envelope_stream"
            );
            write.write_all(b"{\"ok\":false}\n").await.unwrap();
            line.clear();
            assert_eq!(read.read_line(&mut line).await.unwrap(), 0);
        });
        assert!(
            relay_io(client, &b"must-not-forward\n"[..], tokio::io::sink())
                .await
                .is_err()
        );
        daemon.await.unwrap();
    }

    #[tokio::test]
    async fn relay_forwards_only_after_ack_and_fails_when_daemon_disappears() {
        let (client, server) = tokio::io::duplex(4096);
        let (mut input, source) = tokio::io::duplex(4096);
        input.write_all(b"synthetic-request\n").await.unwrap();
        let (output, sink) = tokio::io::duplex(4096);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut read = BufReader::new(read);
            let mut line = String::new();
            read.read_line(&mut line).await.unwrap();
            write
                .write_all(b"{\"ok\":true,\"result\":{\"mcp_envelope_stream\":1}}\n")
                .await
                .unwrap();
            line.clear();
            read.read_line(&mut line).await.unwrap();
            assert_eq!(line, "synthetic-request\n");
            write.write_all(b"synthetic-response\n").await.unwrap();
        });
        assert!(relay_io(client, source, sink).await.is_err());
        daemon.await.unwrap();
        let mut response = String::new();
        BufReader::new(output)
            .read_to_string(&mut response)
            .await
            .unwrap();
        assert_eq!(response, "synthetic-response\n");
    }
}
