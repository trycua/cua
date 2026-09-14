//! Private unary carrier, disabled unless CUA_DRIVER_ENVELOPE_HTTP_PORT is set.
//!
//! This loopback endpoint is NOT an authentication boundary. A trusted upstream
//! service must authorize and forward requests inside the guest before any later
//! Fleet exposure; packaging and exposure require a separate explicit change.
//! Connection IDs and generations are routing/lifecycle markers, not credentials.
//! Each connection already has a host-bound root session; independent bound
//! sessions are unsupported. No request supplies permission options or paths.
//! Standard is the default. Unrestricted sessions require a separate trusted
//! launcher opt-in and an already acknowledged unrestricted runtime.

use cua_driver_sdk::remote::DriverRequestEnvelope;
use cua_driver_sdk::remote_receiver::DriverEnvelopeReceiver;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 64;
const MAX_EXCHANGES: usize = 32;
const IDLE: Duration = Duration::from_secs(300);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
type Factory = dyn Fn() -> Result<(Arc<DriverEnvelopeReceiver>, String), String> + Send + Sync;
type HttpResult<T> = Result<T, (u16, &'static str)>;

struct Entry {
    receiver: Arc<DriverEnvelopeReceiver>,
    touched: Instant,
    active: usize,
}

pub(crate) struct Service {
    entries: Mutex<HashMap<String, Entry>>,
    factory: Arc<Factory>,
    exchanges: tokio::sync::Semaphore,
}

struct Active {
    service: Arc<Service>,
    id: String,
    receiver: Arc<DriverEnvelopeReceiver>,
}

impl Drop for Active {
    fn drop(&mut self) {
        if let Some(entry) = self.service.entries.lock().unwrap().get_mut(&self.id) {
            entry.active -= 1;
            entry.touched = Instant::now();
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        for entry in self.entries.get_mut().unwrap().values() {
            entry.receiver.close();
        }
    }
}

impl Service {
    #[cfg(test)]
    pub(crate) fn for_test(factory: Arc<Factory>) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            factory,
            exchanges: tokio::sync::Semaphore::new(MAX_EXCHANGES),
        })
    }

    pub(crate) fn for_sdk(sdk: Arc<crate::sdk_adapter::SdkAdapter>) -> anyhow::Result<Arc<Self>> {
        let mode = configured_session_mode()?;
        Ok(Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            factory: Arc::new(move || sdk.create_envelope_receiver(mode)),
            exchanges: tokio::sync::Semaphore::new(MAX_EXCHANGES),
        }))
    }

    pub(crate) fn close_all(&self) {
        for entry in self.entries.lock().unwrap().values() {
            entry.receiver.close();
        }
    }

    pub(crate) fn reap(&self) {
        self.entries.lock().unwrap().retain(|_, entry| {
            if entry.active == 0 && entry.touched.elapsed() >= IDLE {
                entry.receiver.close();
                false
            } else {
                true
            }
        });
    }

    fn lookup(self: &Arc<Self>, id: &str, generation: &str) -> HttpResult<Active> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(id).ok_or((404, "connection_not_found"))?;
        if entry.receiver.generation() != generation {
            return Err((409, "stale_connection"));
        }
        entry.active += 1;
        entry.touched = Instant::now();
        Ok(Active {
            service: self.clone(),
            id: id.into(),
            receiver: entry.receiver.clone(),
        })
    }

    pub(crate) async fn route(self: &Arc<Self>, request: Request) -> HttpResult<Value> {
        self.reap();
        if request.method == "POST" && request.path == "/v1/connections" {
            let _: Empty = decode(&request.body)?;
            let mut entries = self.entries.lock().unwrap();
            if entries.len() >= MAX_CONNECTIONS {
                return Err((503, "connection_limit"));
            }
            let (receiver, public_session) =
                (self.factory)().map_err(|_| (503, "session_unavailable"))?;
            let id = uuid::Uuid::new_v4().to_string();
            let response = json!({"connection_id": id, "generation": receiver.generation(), "capabilities": receiver.capabilities(), "public_session": public_session});
            entries.insert(
                id,
                Entry {
                    receiver,
                    touched: Instant::now(),
                    active: 0,
                },
            );
            return Ok(response);
        }
        let suffix = request
            .path
            .strip_prefix("/v1/connections/")
            .ok_or((404, "unknown_route"))?;
        let parts: Vec<_> = suffix.split('/').collect();
        let id = parts[0];
        if uuid::Uuid::parse_str(id).is_err() {
            return Err((404, "unknown_route"));
        }
        let valid_route = matches!(
            (request.method.as_str(), parts.as_slice()),
            ("POST", [_, "exchange"]) | ("POST", [_, "cancel"]) | ("DELETE", [_])
        );
        if !valid_route {
            return Err((404, "unknown_route"));
        }
        let generation = request
            .generation
            .as_deref()
            .ok_or((400, "generation_required"))?;
        let active = self.lookup(id, generation)?;
        match (request.method.as_str(), parts.as_slice()) {
            ("POST", [_, "exchange"]) => {
                // Do not let queued actions consume every HTTP task: control
                // requests must still be admitted while exchanges are waiting.
                let _permit = self
                    .exchanges
                    .try_acquire()
                    .map_err(|_| (429, "exchange_limit"))?;
                let envelope: DriverRequestEnvelope = decode(&request.body)?;
                let mut response = active.receiver.exchange(generation, envelope).await;
                if !response_fits(&response) {
                    active.receiver.close();
                    response.ok = false;
                    response.result = None;
                    response.error = Some(
                        "Driver response exceeded carrier limit; completion is unknown".into(),
                    );
                    response.error_code = Some("response_too_large".into());
                    response.completion_known = false;
                }
                serde_json::to_value(response).map_err(|_| (500, "serialization_failed"))
            }
            ("POST", [_, "cancel"]) => {
                let cancel: Cancel = decode(&request.body)?;
                if cancel.request_id.is_empty() || cancel.request_id.len() > 256 {
                    return Err((400, "invalid_request_id"));
                }
                active
                    .receiver
                    .cancel(generation, &cancel.request_id)
                    .map_err(|_| (409, "cancel_failed"))?;
                Ok(json!({"ok": true}))
            }
            ("DELETE", [_]) => {
                if !request.body.is_empty() {
                    let _: Empty = decode(&request.body)?;
                }
                // Retain the receiver's closed ledger until idle removal.
                active.receiver.close();
                Ok(json!({"ok": true}))
            }
            _ => unreachable!(),
        }
    }
}

fn response_fits(value: &impl serde::Serialize) -> bool {
    // Count serialized bytes without allocating another copy of a potentially
    // large native result, stopping as soon as the wire limit is exceeded.
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("response_too_large"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(MAX_RESPONSE), value).is_ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    request_id: String,
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> HttpResult<T> {
    serde_json::from_slice(body).map_err(|_| (400, "invalid_json"))
}

#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) generation: Option<String>,
    pub(crate) body: Vec<u8>,
}

fn parse_headers(bytes: &[u8]) -> HttpResult<(Request, usize)> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Request::new(&mut headers);
    if !parsed
        .parse(bytes)
        .map_err(|_| (400, "invalid_http"))?
        .is_complete()
    {
        return Err((400, "incomplete_headers"));
    }
    if parsed.version != Some(1) {
        return Err((400, "http_1_1_required"));
    }
    let method = parsed.method.ok_or((400, "method_required"))?;
    if !matches!(method, "POST" | "DELETE") {
        return Err((405, "unsupported_method"));
    }
    let path = parsed.path.ok_or((400, "path_required"))?;
    if path.contains(['?', '#', '%']) || !path.starts_with('/') {
        return Err((400, "invalid_path"));
    }
    let mut length = None;
    let mut generation = None;
    for header in parsed.headers.iter() {
        if header.name.eq_ignore_ascii_case("origin") {
            return Err((403, "browser_origin_forbidden"));
        }
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err((400, "transfer_encoding_forbidden"));
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            if length.is_some()
                || header.value.is_empty()
                || !header.value.iter().all(u8::is_ascii_digit)
            {
                return Err((400, "invalid_content_length"));
            }
            let value =
                std::str::from_utf8(header.value).map_err(|_| (400, "invalid_content_length"))?;
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| (413, "body_too_large"))?,
            );
        }
        if header.name.eq_ignore_ascii_case("x-cua-driver-generation") {
            if generation.is_some() || header.value.is_empty() || header.value.len() > 128 {
                return Err((400, "invalid_generation"));
            }
            generation = Some(
                std::str::from_utf8(header.value)
                    .map_err(|_| (400, "invalid_generation"))?
                    .to_owned(),
            );
        }
    }
    let length = match length {
        Some(n) => n,
        None if method == "DELETE" => 0,
        None => return Err((411, "content_length_required")),
    };
    if length > MAX_BODY {
        return Err((413, "body_too_large"));
    }
    Ok((
        Request {
            method: method.into(),
            path: path.into(),
            generation,
            body: Vec::new(),
        },
        length,
    ))
}

async fn read_request(reader: &mut (impl AsyncRead + Unpin)) -> HttpResult<Request> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= MAX_HEADERS {
            return Err((431, "headers_too_large"));
        }
        let mut chunk = [0; 1024];
        let remaining = (MAX_HEADERS - bytes.len()).min(chunk.len());
        let n = reader
            .read(&mut chunk[..remaining])
            .await
            .map_err(|_| (400, "read_failed"))?;
        if n == 0 {
            return Err((400, "incomplete_headers"));
        }
        bytes.extend_from_slice(&chunk[..n]);
    };
    let (mut request, length) = parse_headers(&bytes[..header_end])?;
    request
        .body
        .extend_from_slice(&bytes[header_end..bytes.len().min(header_end + length)]);
    let received = request.body.len();
    request.body.resize(length, 0);
    reader
        .read_exact(&mut request.body[received..])
        .await
        .map_err(|_| (400, "incomplete_body"))?;
    Ok(request)
}

pub fn configured_port() -> anyhow::Result<Option<u16>> {
    match std::env::var("CUA_DRIVER_ENVELOPE_HTTP_PORT") {
        Err(std::env::VarError::NotPresent) => Ok(None),
        Ok(value) => {
            let port = value.parse::<u16>()?;
            anyhow::ensure!(port != 0, "CUA_DRIVER_ENVELOPE_HTTP_PORT must be nonzero");
            Ok(Some(port))
        }
        Err(error) => Err(error.into()),
    }
}

pub struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn select_session_mode(
    requested: Option<&str>,
    host: cua_driver_core::authorization::PermissionMode,
    has_manifest: bool,
) -> anyhow::Result<cua_driver_sdk::SessionPermissionMode> {
    use cua_driver_core::authorization::PermissionMode;
    use cua_driver_sdk::SessionPermissionMode;
    // This carrier cannot propagate a host manifest into its bound session.
    anyhow::ensure!(
        !has_manifest,
        "envelope sessions with a host capability manifest are unsupported"
    );
    match requested {
        None | Some("standard") => Ok(SessionPermissionMode::Standard),
        Some("unrestricted") => {
            anyhow::ensure!(
                host == PermissionMode::Unrestricted,
                "unrestricted envelope sessions require an acknowledged unrestricted host"
            );
            Ok(SessionPermissionMode::Unrestricted)
        }
        Some(_) => {
            anyhow::bail!("CUA_DRIVER_ENVELOPE_PERMISSION_MODE must be standard or unrestricted")
        }
    }
}

fn configured_session_mode() -> anyhow::Result<cua_driver_sdk::SessionPermissionMode> {
    let requested = match std::env::var("CUA_DRIVER_ENVELOPE_PERMISSION_MODE") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let host =
        cua_driver_core::authorization::configured_permission_mode().map_err(anyhow::Error::msg)?;
    let has_manifest = cua_driver_core::session_manifest::configured_capability_manifest()
        .map_err(anyhow::Error::msg)?
        .is_some();
    select_session_mode(requested.as_deref(), host, has_manifest)
}

pub async fn start(sdk: Arc<crate::sdk_adapter::SdkAdapter>, port: u16) -> anyhow::Result<Server> {
    let service = Service::for_sdk(sdk)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let task = tokio::spawn(async move {
        let permits = Arc::new(tokio::sync::Semaphore::new(64));
        let mut tasks = tokio::task::JoinSet::new();
        let mut reaper = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { break };
                    let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                    let service = service.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let result = match tokio::time::timeout(IO_TIMEOUT, read_request(&mut stream)).await {
                            Ok(Ok(request)) => service.route(request).await,
                            Ok(Err(error)) => Err(error),
                            Err(_) => Err((408, "read_timeout")),
                        };
                        let (status, body) = match result { Ok(body) => (200, body), Err((status, error)) => (status, json!({"error": error})) };
                        let body = body.to_string();
                        let response = format!("HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = tokio::time::timeout(IO_TIMEOUT, stream.write_all(response.as_bytes())).await;
                    });
                }
                _ = reaper.tick() => service.reap(),
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
    });
    eprintln!("Private Driver envelope HTTP listening on 127.0.0.1:{port}");
    Ok(Server(task))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_sdk::{remote_receiver::DriverEnvelopeExecutor, DriverError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn envelope_mode_never_implicitly_inherits_unrestricted() {
        use cua_driver_core::authorization::PermissionMode as Host;
        for host in [Host::Standard, Host::Bounded, Host::Unrestricted] {
            for requested in [None, Some("standard")] {
                assert_eq!(
                    select_session_mode(requested, host, false).unwrap(),
                    cua_driver_sdk::SessionPermissionMode::Standard
                );
            }
        }
    }

    #[test]
    fn envelope_unrestricted_requires_matching_host_and_no_manifest() {
        use cua_driver_core::authorization::PermissionMode as Host;
        assert_eq!(
            select_session_mode(Some("unrestricted"), Host::Unrestricted, false).unwrap(),
            cua_driver_sdk::SessionPermissionMode::Unrestricted
        );
        for host in [Host::Standard, Host::Bounded] {
            assert!(select_session_mode(Some("unrestricted"), host, false).is_err());
        }
        assert!(select_session_mode(Some("unrestricted"), Host::Unrestricted, true).is_err());
        assert!(select_session_mode(None, Host::Standard, true).is_err());
        assert!(select_session_mode(Some("standard"), Host::Standard, true).is_err());
    }

    #[test]
    fn envelope_mode_rejects_unknown_and_bounded_values() {
        use cua_driver_core::authorization::PermissionMode;
        for value in ["", "bounded", "UNRESTRICTED", " unrestricted", "inherit"] {
            assert!(select_session_mode(Some(value), PermissionMode::Unrestricted, false).is_err());
        }
    }

    #[test]
    fn envelope_startup_mode_requires_explicit_acknowledgement() {
        const CHILD: &str = "CUA_TEST_ENVELOPE_MODE_CHILD";
        if let Ok(expected) = std::env::var(CHILD) {
            let selected = configured_session_mode();
            match expected.as_str() {
                "standard" => assert_eq!(
                    selected.unwrap(),
                    cua_driver_sdk::SessionPermissionMode::Standard
                ),
                "unrestricted" => assert_eq!(
                    selected.unwrap(),
                    cua_driver_sdk::SessionPermissionMode::Unrestricted
                ),
                "error" => assert!(selected.is_err()),
                "manifest_error" => {
                    assert!(
                        cua_driver_core::session_manifest::configured_capability_manifest()
                            .unwrap()
                            .is_some()
                    );
                    assert!(selected.is_err());
                }
                _ => panic!("unknown synthetic test expectation"),
            }
            return;
        }
        // Startup mode is process-cached; each case must get a fresh process.
        for (requested, host, acknowledged, manifest, expected) in [
            (None, "standard", false, false, "standard"),
            (None, "unrestricted", true, false, "standard"),
            (Some("unrestricted"), "standard", false, false, "error"),
            (Some("unrestricted"), "unrestricted", false, false, "error"),
            (
                Some("unrestricted"),
                "unrestricted",
                true,
                false,
                "unrestricted",
            ),
            (Some("bounded"), "unrestricted", true, false, "error"),
            (None, "standard", false, true, "manifest_error"),
            (
                Some("unrestricted"),
                "unrestricted",
                true,
                true,
                "manifest_error",
            ),
        ] {
            use std::io::Write;
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(b"version: 3\nallow:\n  tools: [get_config]\n")
                .unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "driver_service_http::tests::envelope_startup_mode_requires_explicit_acknowledgement",
                    "--nocapture",
                ])
                .env(CHILD, expected)
                .env("CUA_DRIVER_PERMISSION_MODE", host)
                .env("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", if acknowledged { "1" } else { "0" })
                .env_remove("CUA_DRIVER_ENVELOPE_PERMISSION_MODE")
                .env_remove("CUA_DRIVER_CAPABILITY_MANIFEST_FILE")
                .env_remove("CUA_DRIVER_SESSION_POLICY_FILE");
            if manifest {
                command.env("CUA_DRIVER_CAPABILITY_MANIFEST_FILE", file.path());
            }
            if let Some(requested) = requested {
                command.env("CUA_DRIVER_ENVELOPE_PERMISSION_MODE", requested);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "startup mode case failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    struct Fake(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl DriverEnvelopeExecutor for Fake {
        async fn metadata(&self) -> Result<Value, DriverError> {
            Ok(json!({"raw": [1, {"x": true}]}))
        }
        async fn list_tools(&self) -> Result<Value, DriverError> {
            Ok(json!({"tools": []}))
        }
        async fn call(&self, _: String, _: Value) -> Result<Value, DriverError> {
            Ok(json!({"native": true}))
        }
        fn close(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn service() -> (Arc<Service>, Arc<AtomicUsize>) {
        let closed = Arc::new(AtomicUsize::new(0));
        let count = closed.clone();
        (
            Arc::new(Service {
                entries: Mutex::new(HashMap::new()),
                exchanges: tokio::sync::Semaphore::new(MAX_EXCHANGES),
                factory: Arc::new(move || {
                    Ok((
                        DriverEnvelopeReceiver::new(Arc::new(Fake(count.clone()))),
                        "test-session".into(),
                    ))
                }),
            }),
            closed,
        )
    }
    fn request(method: &str, path: &str, generation: Option<&str>, body: Value) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            generation: generation.map(str::to_owned),
            body: body.to_string().into_bytes(),
        }
    }
    async fn create(service: &Arc<Service>) -> Value {
        service
            .route(request("POST", "/v1/connections", None, json!({})))
            .await
            .unwrap()
    }

    struct SpecialFake {
        oversized: bool,
    }
    #[async_trait::async_trait]
    impl DriverEnvelopeExecutor for SpecialFake {
        async fn metadata(&self) -> Result<Value, DriverError> {
            if self.oversized {
                Ok(json!({"data": "x".repeat(MAX_RESPONSE)}))
            } else {
                std::future::pending().await
            }
        }
        async fn list_tools(&self) -> Result<Value, DriverError> {
            unreachable!()
        }
        async fn call(&self, _: String, _: Value) -> Result<Value, DriverError> {
            unreachable!()
        }
        fn close(&self) {}
    }
    fn special_service(oversized: bool) -> Arc<Service> {
        Arc::new(Service {
            entries: Mutex::new(HashMap::new()),
            exchanges: tokio::sync::Semaphore::new(MAX_EXCHANGES),
            factory: Arc::new(move || {
                Ok((
                    DriverEnvelopeReceiver::new(Arc::new(SpecialFake { oversized })),
                    "test-session".into(),
                ))
            }),
        })
    }
    fn metadata_envelope(id: &str) -> Value {
        json!({"envelope_version":1,"request_id":id,"operation":"metadata","deadline_unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()+10000})
    }

    #[tokio::test]
    async fn saturated_exchanges_leave_cancel_and_close_admitted() {
        let service = special_service(false);
        let mut tasks = tokio::task::JoinSet::new();
        let mut connections = Vec::new();
        for _ in 0..MAX_EXCHANGES {
            let connection = create(&service).await;
            let path = format!(
                "/v1/connections/{}/exchange",
                connection["connection_id"].as_str().unwrap()
            );
            let request = request(
                "POST",
                &path,
                connection["generation"].as_str(),
                metadata_envelope("pending"),
            );
            let service = service.clone();
            tasks.spawn(async move { service.route(request).await });
            connections.push(connection);
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while service.exchanges.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let first = &connections[0];
        let path = format!(
            "/v1/connections/{}",
            first["connection_id"].as_str().unwrap()
        );
        assert_eq!(
            service
                .route(request(
                    "POST",
                    &format!("{path}/exchange"),
                    first["generation"].as_str(),
                    metadata_envelope("extra")
                ))
                .await
                .unwrap_err(),
            (429, "exchange_limit")
        );
        service
            .route(request(
                "POST",
                &format!("{path}/cancel"),
                first["generation"].as_str(),
                json!({"request_id":"pending"}),
            ))
            .await
            .unwrap();
        for connection in &connections {
            let path = format!(
                "/v1/connections/{}",
                connection["connection_id"].as_str().unwrap()
            );
            service
                .route(request(
                    "DELETE",
                    &path,
                    connection["generation"].as_str(),
                    json!({}),
                ))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(result) = tasks.join_next().await {
                assert!(result.unwrap().is_ok());
            }
        })
        .await
        .unwrap();
        assert_eq!(service.exchanges.available_permits(), MAX_EXCHANGES);
    }

    #[tokio::test]
    async fn oversized_response_closes_connection_with_unknown_completion() {
        let service = special_service(true);
        let connection = create(&service).await;
        let path = format!(
            "/v1/connections/{}/exchange",
            connection["connection_id"].as_str().unwrap()
        );
        let response = service
            .route(request(
                "POST",
                &path,
                connection["generation"].as_str(),
                metadata_envelope("large"),
            ))
            .await
            .unwrap();
        assert_eq!(response["error_code"], "response_too_large");
        assert_eq!(response["completion_known"], false);
        assert_eq!(response["ok"], false);
        assert!(response.get("result").is_none());
        assert!(response_fits(&response));
        let response = service
            .route(request(
                "POST",
                &path,
                connection["generation"].as_str(),
                metadata_envelope("after"),
            ))
            .await
            .unwrap();
        assert_eq!(response["error_code"], "connection_closed");
        assert!(response_fits(&"x".repeat(MAX_RESPONSE - 2)));
        assert!(!response_fits(&"x".repeat(MAX_RESPONSE - 1)));
    }

    #[test]
    fn strict_headers() {
        for header in [
            "Origin: null\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Content-Length: 0\r\n",
            "X-Cua-Driver-Generation: a\r\nX-Cua-Driver-Generation: b\r\n",
        ] {
            let bytes =
                format!("POST /v1/connections HTTP/1.1\r\nContent-Length: 2\r\n{header}\r\n");
            assert!(parse_headers(bytes.as_bytes()).is_err(), "{header}");
        }
        for line in [
            "GET /v1/connections HTTP/1.1",
            "POST /v1/connections?x HTTP/1.1",
            "POST /v1/connections HTTP/1.0",
        ] {
            assert!(
                parse_headers(format!("{line}\r\nContent-Length: 2\r\n\r\n").as_bytes()).is_err()
            );
        }
        for length in ["-1", "+1", "1,1", "1048577", "999999999999999999999999"] {
            assert!(parse_headers(
                format!("POST /v1/connections HTTP/1.1\r\nContent-Length: {length}\r\n\r\n")
                    .as_bytes()
            )
            .is_err());
        }
    }

    #[tokio::test]
    async fn bounded_read_and_unary_pipelining() {
        let data = b"POST /v1/connections HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}POST /ignored HTTP/1.1\r\n\r\n";
        let result = read_request(&mut &data[..]).await.unwrap();
        assert_eq!(result.body, b"{}");
        assert_eq!(result.path, "/v1/connections");
        assert_eq!(
            read_request(&mut &vec![b'x'; MAX_HEADERS + 1][..])
                .await
                .unwrap_err()
                .0,
            431
        );
        assert!(read_request(
            &mut &b"POST /v1/connections HTTP/1.1\r\nContent-Length: 2\r\n\r\n{"[..]
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn generation_required_on_every_connection_route() {
        let (service, closed) = service();
        let connection = create(&service).await;
        let id = connection["connection_id"].as_str().unwrap();
        for (method, suffix, body) in [
            ("POST", "/exchange", json!({})),
            ("POST", "/cancel", json!({"request_id":"r"})),
            ("DELETE", "", json!({})),
        ] {
            let path = format!("/v1/connections/{id}{suffix}");
            assert_eq!(
                service
                    .route(request(method, &path, None, body.clone()))
                    .await
                    .unwrap_err()
                    .0,
                400
            );
            assert_eq!(
                service
                    .route(request(method, &path, Some("stale"), body))
                    .await
                    .unwrap_err()
                    .0,
                409
            );
        }
        assert_eq!(closed.load(Ordering::SeqCst), 0);
        assert!(service
            .route(request(
                "POST",
                &format!("/v1/connections/{id}/bound_session"),
                None,
                json!({})
            ))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn raw_result_and_closed_ledger() {
        let (service, closed) = service();
        let connection = create(&service).await;
        assert_eq!(connection["public_session"], "test-session");
        let path = format!(
            "/v1/connections/{}",
            connection["connection_id"].as_str().unwrap()
        );
        let generation = connection["generation"].as_str();
        let envelope = json!({"envelope_version":1,"request_id":"first","operation":"metadata","deadline_unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()+10000});
        let result = service
            .route(request(
                "POST",
                &format!("{path}/exchange"),
                generation,
                envelope.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(result["result"], json!({"raw": [1, {"x": true}]}));
        service
            .route(request("DELETE", &path, generation, json!({})))
            .await
            .unwrap();
        let result = service
            .route(request(
                "POST",
                &format!("{path}/exchange"),
                generation,
                envelope,
            ))
            .await
            .unwrap();
        assert_eq!(result["error_code"], "connection_closed");
        assert_eq!(closed.load(Ordering::SeqCst), 1);
        assert_eq!(service.entries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn capacity_and_active_reaping() {
        let (service, closed) = service();
        let first = create(&service).await;
        for _ in 1..MAX_CONNECTIONS {
            create(&service).await;
        }
        assert_eq!(
            service
                .route(request("POST", "/v1/connections", None, json!({})))
                .await
                .unwrap_err()
                .0,
            503
        );
        let active = service
            .lookup(
                first["connection_id"].as_str().unwrap(),
                first["generation"].as_str().unwrap(),
            )
            .unwrap();
        for entry in service.entries.lock().unwrap().values_mut() {
            entry.touched = Instant::now() - IDLE;
        }
        service.reap();
        assert_eq!(service.entries.lock().unwrap().len(), 1);
        assert_eq!(closed.load(Ordering::SeqCst), MAX_CONNECTIONS - 1);
        drop(active);
        assert_eq!(service.entries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn creation_rejects_wire_session_options() {
        let (service, _) = service();
        assert!(service
            .route(request(
                "POST",
                "/v1/connections",
                None,
                json!({"mode":"unrestricted"})
            ))
            .await
            .is_err());
        assert!(service.entries.lock().unwrap().is_empty());
    }
}
