//! Stateful typed MCP carrier. The host supplies only authenticated service bytes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};

use crate::remote::{
    DriverChannelCapabilities, DriverEnvelopeChannel, DriverRequestEnvelope, DriverResponseEnvelope,
};
use crate::{CuaDriver, DriverError, TrustedSessionOptions};

const PROTOCOL: &str = "2025-06-18";
const REQUEST_LIMIT: usize = 1024 * 1024;
const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const CLEANUP: Duration = Duration::from_secs(2);
const MALFORMED: &str = "MCP Driver response is malformed; completion is unknown";

#[derive(Clone, Debug, uniffi::Record)]
pub struct DriverServiceHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct DriverServiceRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<DriverServiceHeader>,
    pub body: Vec<u8>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct DriverServiceResponse {
    pub status: u16,
    pub headers: Vec<DriverServiceHeader>,
    pub body: Vec<u8>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum DriverServiceTransportError {
    #[error("Driver service transport failed: {reason}")]
    Failed { reason: String },
}

/// A host-bound named service, never a guest-selected URL or authority.
/// The transport must enforce `timeout_ms` and bound buffered response bytes to
/// 16 MiB. Rust bounds caller waits independently but deliberately never aborts
/// an active response stream during receiver teardown.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait DriverServiceTransport: Send + Sync + 'static {
    async fn send(
        &self,
        request: DriverServiceRequest,
    ) -> Result<DriverServiceResponse, DriverServiceTransportError>;
}

#[derive(Default)]
struct State {
    started: bool,
    closed: bool,
    broken: bool,
    session: Option<String>,
    connection: Option<String>,
    public_session: Option<String>,
    capabilities: Option<DriverChannelCapabilities>,
    pending: HashMap<String, watch::Receiver<bool>>,
    cancelled: HashSet<String>,
}

struct Inner {
    transport: Arc<dyn DriverServiceTransport>,
    principal: String,
    generation: OnceLock<String>,
    state: Mutex<State>,
    opening: watch::Sender<bool>,
    cleanup: tokio::sync::Mutex<Option<Result<(), String>>>,
    this: Weak<Inner>,
}

#[derive(uniffi::Object)]
pub struct McpDriverChannel {
    inner: Arc<Inner>,
    driver: Mutex<Option<Arc<CuaDriver>>>,
}

fn remote(reason: String) -> DriverError {
    DriverError::Remote { reason }
}

/// Construct before opening so the owner can invalidate an in-flight initialize.
#[uniffi::export]
pub fn open_mcp_driver_channel(
    transport: Arc<dyn DriverServiceTransport>,
    authenticated_principal: String,
) -> Result<Arc<McpDriverChannel>, DriverError> {
    if authenticated_principal.trim().is_empty() {
        return Err(remote(
            "Driver service requires a host authenticated principal".into(),
        ));
    }
    let inner = Arc::new_cyclic(|this| Inner {
        transport,
        principal: authenticated_principal,
        generation: OnceLock::new(),
        state: Mutex::new(State::default()),
        opening: watch::channel(false).0,
        cleanup: tokio::sync::Mutex::new(None),
        this: this.clone(),
    });
    Ok(Arc::new(McpDriverChannel {
        inner,
        driver: Mutex::new(None),
    }))
}

#[uniffi::export(async_runtime = "tokio")]
impl McpDriverChannel {
    pub async fn open(&self) -> Result<(), DriverError> {
        {
            let mut state = self.inner.state.lock().unwrap();
            if state.started || state.closed {
                return Err(remote("MCP Driver carrier cannot reconnect".into()));
            }
            state.started = true;
            self.inner.opening.send_replace(true);
        }
        let inner = self.inner.clone();
        let (tx, rx) = oneshot::channel();
        let mut guard = CancelOnDrop(Some(inner.clone()));
        tokio::spawn(async move {
            let result = inner.initialize().await;
            if result.is_err() {
                inner.state.lock().unwrap().closed = true;
            }
            inner.opening.send_replace(false);
            if inner.state.lock().unwrap().closed {
                let _ = inner.close_owned().await;
            }
            let _ = tx.send(result);
        });
        let result = tokio::time::timeout(Duration::from_secs(120), rx)
            .await
            .map_err(|_| {
                remote("MCP Driver initialization timed out; completion is unknown".into())
            })?
            .map_err(|_| remote("MCP Driver initialization failed".into()))?;
        guard.0.take();
        result.map_err(remote)
    }

    pub fn public_session(&self) -> Result<String, DriverError> {
        let state = self.inner.state.lock().unwrap();
        if state.closed {
            return Err(remote("Driver connection is closed".into()));
        }
        state
            .public_session
            .clone()
            .ok_or_else(|| remote("Driver connection is not open".into()))
    }

    pub fn driver(&self) -> Result<Arc<CuaDriver>, DriverError> {
        self.public_session()?;
        let mut driver = self.driver.lock().unwrap();
        if let Some(driver) = driver.as_ref() {
            return Ok(driver.clone());
        }
        let connected = CuaDriver::connect_remote(self.inner.clone())?;
        *driver = Some(connected.clone());
        Ok(connected)
    }

    pub async fn close(&self) -> Result<(), DriverError> {
        self.inner.close_owned().await.map_err(remote)
    }
}

struct CancelOnDrop(Option<Arc<Inner>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(inner) = self.0.take() {
            inner.state.lock().unwrap().closed = true;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = inner.close_owned().await;
                });
            }
        }
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Inner {
    async fn http(
        &self,
        method: &str,
        body: Option<Value>,
        timeout_ms: u64,
        initializing: bool,
    ) -> Result<DriverServiceResponse, String> {
        let body = body
            .map(|body| serde_json::to_vec(&body))
            .transpose()
            .map_err(|_| "Driver request is malformed".to_owned())?
            .unwrap_or_default();
        if body.len() > REQUEST_LIMIT {
            return Err("MCP Driver request exceeds the size limit".into());
        }
        let mut headers = vec![DriverServiceHeader {
            name: "Accept".into(),
            value: "application/json, text/event-stream".into(),
        }];
        if !body.is_empty() {
            headers.push(DriverServiceHeader {
                name: "Content-Type".into(),
                value: "application/json".into(),
            });
        }
        if let Some(session) = self.state.lock().unwrap().session.clone() {
            headers.push(DriverServiceHeader {
                name: "Mcp-Session-Id".into(),
                value: session,
            });
            headers.push(DriverServiceHeader {
                name: "MCP-Protocol-Version".into(),
                value: PROTOCOL.into(),
            });
        }
        let response = self
            .transport
            .send(DriverServiceRequest {
                method: method.into(),
                path: "/mcp".into(),
                headers,
                body,
                timeout_ms,
            })
            .await;
        let mut state = self.state.lock().unwrap();
        let response = response.map_err(|_| {
            state.broken = true;
            "MCP Driver transport failed; completion is unknown".to_owned()
        })?;
        if !(200..300).contains(&response.status) {
            state.broken = true;
            return Err(match response.status {
                401 | 403 => "Driver service authorization denied",
                404 | 409 => "MCP Driver session is stale or unavailable",
                _ => "MCP Driver request failed; completion is unknown",
            }
            .into());
        }
        let sessions: Vec<_> = response
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("mcp-session-id"))
            .map(|h| h.value.as_str())
            .collect();
        if initializing {
            if sessions.len() != 1
                || sessions[0].is_empty()
                || sessions[0].len() > 256
                || !sessions[0].bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                state.broken = true;
                return Err("MCP Driver session negotiation is malformed".into());
            }
            // Preserve allocated state even when the following body is invalid.
            state.session = Some(sessions[0].to_owned());
        } else if !sessions.is_empty()
            && (sessions.len() != 1 || Some(sessions[0]) != state.session.as_deref())
        {
            state.broken = true;
            return Err("MCP Driver session changed unexpectedly".into());
        }
        if response.body.len() > RESPONSE_LIMIT {
            state.broken = true;
            return Err("MCP Driver response exceeds the size limit".into());
        }
        Ok(response)
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        initializing: bool,
    ) -> Result<Value, String> {
        if self.state.lock().unwrap().broken {
            return Err("MCP Driver carrier is closed".into());
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let response = self
            .http(
                "POST",
                Some(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})),
                timeout_ms,
                initializing,
            )
            .await?;
        decode_rpc(&response, &id).map_err(|error| {
            self.state.lock().unwrap().broken = true;
            error
        })
    }

    async fn initialize(&self) -> Result<(), String> {
        let result = self.rpc("initialize", json!({"protocolVersion": PROTOCOL, "capabilities":{}, "clientInfo":{"name":"cua-driver-sdk","version":"1"}}), 120000, true).await?;
        if result.get("protocolVersion").and_then(Value::as_str) != Some(PROTOCOL)
            || result
                .pointer("/capabilities/experimental/ai.cua.driver.envelopes/version")
                .and_then(Value::as_u64)
                != Some(1)
        {
            self.state.lock().unwrap().broken = true;
            return Err("MCP endpoint does not support typed Driver envelopes v1".into());
        }
        if self.state.lock().unwrap().closed {
            return Err("Driver connection was closed during initialization".into());
        }
        let ack = self
            .http(
                "POST",
                Some(json!({"jsonrpc":"2.0", "method":"notifications/initialized"})),
                120000,
                false,
            )
            .await?;
        if !matches!(ack.status, 202 | 204) || !ack.body.is_empty() {
            self.state.lock().unwrap().broken = true;
            return Err("MCP Driver initialization acknowledgement is incompatible".into());
        }
        if self.state.lock().unwrap().closed {
            return Err("Driver connection was closed during initialization".into());
        }
        let result = self
            .rpc("cua/driver/v1/open", json!({}), 120000, false)
            .await?;
        let malformed = "Driver service negotiation is malformed or incompatible";
        let connection = result
            .get("connection_id")
            .and_then(Value::as_str)
            .filter(|v| token(v))
            .ok_or(malformed)?;
        let generation = result
            .get("generation")
            .and_then(Value::as_str)
            .filter(|v| token(v))
            .ok_or(malformed)?;
        // Capture receiver ownership before validating the remaining negotiation.
        self.state.lock().unwrap().connection = Some(connection.to_owned());
        self.generation
            .set(generation.to_owned())
            .map_err(|_| malformed)?;
        let caps: DriverChannelCapabilities =
            serde_json::from_value(result.get("capabilities").cloned().ok_or(malformed)?)
                .map_err(|_| malformed)?;
        if caps.minimum_envelope_version > 1
            || caps.maximum_envelope_version < 1
            || !caps.supports_cancellation
        {
            return Err(malformed.into());
        }
        let public = result
            .get("public_session")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or(malformed)?;
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("Driver connection was closed during initialization".into());
        }
        state.public_session = Some(public.into());
        state.capabilities = Some(caps);
        Ok(())
    }

    fn params(&self) -> Result<Value, String> {
        let state = self.state.lock().unwrap();
        Ok(
            json!({"connection_id":state.connection.as_ref().ok_or("Driver connection is not open")?, "generation": self.generation.get().ok_or("Driver connection is not open")?}),
        )
    }

    async fn cleanup_rpc(&self, operation: &str, id: Option<&str>) -> Result<(), String> {
        let mut params = self.params()?;
        if let Some(id) = id {
            params["request_id"] = json!(id);
        }
        let result = self
            .rpc(&format!("cua/driver/v1/{operation}"), params, 2000, false)
            .await?;
        if result != json!({"ok":true}) {
            self.state.lock().unwrap().broken = true;
            return Err("MCP Driver cleanup acknowledgement is malformed".into());
        }
        Ok(())
    }

    async fn close_owned(&self) -> Result<(), String> {
        self.state.lock().unwrap().closed = true;
        let inner = self.this.upgrade().ok_or("Driver connection is closed")?;
        // Dropping a foreign close future must not drop the teardown operation.
        tokio::spawn(async move { inner.cleanup_run().await })
            .await
            .map_err(|_| {
                "Driver cleanup failed; remote session cleanup is unconfirmed".to_owned()
            })?
    }

    async fn cleanup_run(&self) -> Result<(), String> {
        let mut opening = self.opening.subscribe();
        let is_opening = *opening.borrow();
        if is_opening
            && tokio::time::timeout(CLEANUP, async {
                while *opening.borrow_and_update() {
                    if opening.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await
            .is_err()
        {
            return Err(
                "Driver initialization drain timed out; remote session cleanup is unconfirmed"
                    .into(),
            );
        }
        let mut memo = self.cleanup.lock().await;
        if let Some(result) = memo.as_ref() {
            return result.clone();
        }
        let (pending, ids, has_connection, has_session) = {
            let state = self.state.lock().unwrap();
            (
                state.pending.values().cloned().collect::<Vec<_>>(),
                state
                    .cancelled
                    .union(&state.pending.keys().cloned().collect())
                    .cloned()
                    .collect::<Vec<_>>(),
                state.connection.is_some(),
                state.session.is_some(),
            )
        };
        // A close before open must not memoize an empty teardown.
        if !has_session {
            return Ok(());
        }
        let result = self.cleanup_steps(pending, ids, has_connection).await;
        *memo = Some(result.clone());
        result
    }

    async fn cleanup_steps(
        &self,
        pending: Vec<watch::Receiver<bool>>,
        ids: Vec<String>,
        has_connection: bool,
    ) -> Result<(), String> {
        let mut confirmed = true;
        let mut cancellation = Vec::new();
        for id in ids {
            let inner = self.this.upgrade().unwrap();
            cancellation.push(tokio::spawn(async move {
                inner.cleanup_rpc("cancel", Some(&id)).await
            }));
        }
        if tokio::time::timeout(CLEANUP, async {
            for task in cancellation {
                confirmed &= matches!(task.await, Ok(Ok(())));
            }
        })
        .await
        .is_err()
        {
            return Err(
                "Driver cancellation timed out; remote session cleanup is unconfirmed".into(),
            );
        }
        if tokio::time::timeout(CLEANUP, async {
            for mut response in pending {
                while !*response.borrow_and_update() {
                    if response.changed().await.is_err() {
                        break;
                    }
                }
            }
        })
        .await
        .is_err()
        {
            return Err(
                "Driver response drain timed out; remote session cleanup is unconfirmed".into(),
            );
        }
        if has_connection {
            let inner = self.this.upgrade().unwrap();
            match tokio::time::timeout(
                CLEANUP,
                tokio::spawn(async move { inner.cleanup_rpc("close", None).await }),
            )
            .await
            {
                Ok(result) => confirmed &= matches!(result, Ok(Ok(()))),
                Err(_) => {
                    return Err(
                        "Driver receiver close timed out; remote session cleanup is unconfirmed"
                            .into(),
                    )
                }
            }
        }
        let inner = self.this.upgrade().unwrap();
        confirmed &= matches!(
            tokio::time::timeout(
                CLEANUP,
                tokio::spawn(async move { inner.http("DELETE", None, 2000, false).await })
            )
            .await,
            Ok(Ok(Ok(_)))
        );
        if confirmed {
            Ok(())
        } else {
            Err("Driver cleanup failed; remote session cleanup is unconfirmed".into())
        }
    }
}

#[async_trait]
impl DriverEnvelopeChannel for Inner {
    async fn negotiate(&self) -> Result<DriverChannelCapabilities, String> {
        let state = self.state.lock().unwrap();
        if state.closed {
            return Err("Driver connection is closed".into());
        }
        state
            .capabilities
            .clone()
            .ok_or_else(|| "Driver connection is not open".into())
    }

    async fn exchange(
        &self,
        request: DriverRequestEnvelope,
    ) -> Result<DriverResponseEnvelope, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Driver clock is invalid")?
            .as_millis();
        let timeout = request.deadline_unix_ms.saturating_sub(now).min(120000) as u64;
        if timeout == 0 {
            return Err("Driver request deadline has expired".into());
        }
        let mut params = self.params()?;
        params["envelope"] =
            serde_json::to_value(&request).map_err(|_| "Driver request is malformed")?;
        if serde_json::to_vec(&params)
            .map_err(|_| "Driver request is malformed")?
            .len()
            > REQUEST_LIMIT
        {
            return Err("Driver request exceeds the size limit".into());
        }
        let (done, drained) = watch::channel(false);
        {
            let mut state = self.state.lock().unwrap();
            if state.closed
                || state.cancelled.contains(&request.request_id)
                || state.pending.contains_key(&request.request_id)
            {
                return Err("Driver connection is closed or request was cancelled".into());
            }
            state.pending.insert(request.request_id.clone(), drained);
        }
        let inner = self.this.upgrade().unwrap();
        let mut guard = CancelOnDrop(Some(inner.clone()));
        let id = request.request_id.clone();
        let exchange = tokio::spawn(async move {
            let result = inner
                .rpc("cua/driver/v1/exchange", params, timeout + 4000, false)
                .await;
            inner.state.lock().unwrap().pending.remove(&id);
            done.send_replace(true);
            result
        });
        let result = match tokio::time::timeout(Duration::from_millis(timeout), exchange).await {
            Ok(Ok(Ok(value))) => {
                let state = self.state.lock().unwrap();
                if state.closed || state.cancelled.contains(&request.request_id) {
                    Err("Driver response arrived after close or cancellation; completion is unknown".into())
                } else {
                    decode_envelope(value, &request)
                }
            }
            Ok(Ok(Err(error))) => Err(error),
            _ => Err("Driver request deadline has expired; completion is unknown".into()),
        };
        if result
            .as_ref()
            .map_or(true, |response| !response.completion_known)
        {
            self.state
                .lock()
                .unwrap()
                .cancelled
                .insert(request.request_id);
            let _ = self.close_owned().await;
        }
        guard.0.take();
        result
    }

    async fn bind_session(
        &self,
        _: TrustedSessionOptions,
    ) -> Result<Arc<dyn DriverEnvelopeChannel>, String> {
        Err("Fleet Driver connections use a host-bound session; rebinding is unsupported".into())
    }
    async fn cancel(&self, request_id: &str) -> Result<(), String> {
        self.state
            .lock()
            .unwrap()
            .cancelled
            .insert(request_id.into());
        self.close_owned().await
    }
    async fn close(&self) -> Result<(), String> {
        self.close_owned().await
    }
    fn authenticated_principal(&self) -> &str {
        &self.principal
    }
    fn connection_generation(&self) -> &str {
        self.generation.get().map(String::as_str).unwrap_or("")
    }
}

fn decode_envelope(
    value: Value,
    request: &DriverRequestEnvelope,
) -> Result<DriverResponseEnvelope, String> {
    let mut response: DriverResponseEnvelope = serde_json::from_value(value.clone())
        .map_err(|_| "Driver response is malformed; completion is unknown".to_owned())?;
    if response.envelope_version != request.envelope_version
        || response.request_id != request.request_id
        || response.error_code.as_deref().is_some_and(|v| !token(v))
    {
        return Err("Driver response is malformed; completion is unknown".into());
    }
    response.result = value.get("result").cloned();
    response.error = response.error.map(|_| "Driver operation failed".into());
    Ok(response)
}

fn decode_rpc(response: &DriverServiceResponse, id: &str) -> Result<Value, String> {
    let malformed = || MALFORMED.to_owned();
    if response.body.len() > RESPONSE_LIMIT {
        return Err(malformed());
    }
    let text = std::str::from_utf8(&response.body).map_err(|_| malformed())?;
    let media: Vec<_> = response
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("content-type"))
        .collect();
    if media.len() != 1 {
        return Err(malformed());
    }
    let media = media[0]
        .value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let messages = match media.as_str() {
        "application/json" => vec![strict_json(text)?],
        "text/event-stream" => {
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            let blocks: Vec<_> = normalized.split("\n\n").collect();
            if blocks.len() > 128 || !blocks.last().unwrap().trim().is_empty() {
                return Err(malformed());
            }
            let mut messages = Vec::new();
            for block in &blocks[..blocks.len() - 1] {
                let mut data = Vec::new();
                for line in block.split('\n') {
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }
                    let (field, value) = line.split_once(':').unwrap_or((line, ""));
                    let value = value.strip_prefix(' ').unwrap_or(value);
                    match field {
                        "data" => data.push(value),
                        "event" if value.is_empty() || value == "message" => {}
                        "id" | "retry" => {}
                        _ => return Err(malformed()),
                    }
                }
                if !data.is_empty() {
                    messages.push(strict_json(&data.join("\n"))?);
                }
            }
            messages
        }
        _ => return Err(malformed()),
    };
    if messages.len() != 1 {
        return Err(malformed());
    }
    let message = &messages[0];
    if !message.is_object()
        || message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || message.get("id").and_then(Value::as_str) != Some(id)
        || message.get("result").is_some() == message.get("error").is_some()
    {
        return Err(malformed());
    }
    if let Some(error) = message.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(malformed)?;
        return Err(match code {
            -32404 | -32409 => "MCP Driver connection is stale or unavailable",
            -32601 => "MCP endpoint does not support typed Driver envelopes",
            _ => "MCP Driver request failed; completion is unknown",
        }
        .into());
    }
    Ok(message["result"].clone())
}

// serde_json's default Value visitor accepts duplicate members. Reject them at
// every depth while retaining the parser's recursion and finite-number bounds.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("strict JSON")
            }
            fn visit_bool<E>(self, value: bool) -> Result<Strict, E> {
                Ok(Strict(json!(value)))
            }
            fn visit_i64<E>(self, value: i64) -> Result<Strict, E> {
                Ok(Strict(json!(value)))
            }
            fn visit_u64<E>(self, value: u64) -> Result<Strict, E> {
                Ok(Strict(json!(value)))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Strict, E> {
                serde_json::Number::from_f64(value)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E>(self, value: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(value.into())))
            }
            fn visit_unit<E>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, Strict(value))) = map.next_entry::<String, Strict>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate JSON member"));
                    }
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}
fn strict_json(text: &str) -> Result<Value, String> {
    serde_json::from_str::<Strict>(text)
        .map(|v| v.0)
        .map_err(|_| MALFORMED.into())
}

#[cfg(test)]
#[path = "remote_mcp_tests.rs"]
mod tests;
