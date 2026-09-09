//! Guest-side dispatch for an already authorized, private service carrier.
//!
//! This module opens no listener and authenticates no credentials. The service
//! adapter must authorize access before creating or looking up a connection.
//! Generation and connection identities are lifecycle markers, not credentials.

use crate::remote::{
    DriverChannelCapabilities, DriverRequestEnvelope, DriverResponseEnvelope,
    DRIVER_ENVELOPE_VERSION,
};
use crate::{CuaDriver, CuaDriverSession, DriverError, TrustedSessionOptions};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use uuid::Uuid;

const MAX_REQUESTS: usize = 4096;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_DEADLINE_MS: u128 = 120_000;

/// A session-scoped executor. Implementations must preserve native results and
/// must not interpret closing a connection as shutting down the shared runtime.
#[async_trait]
pub trait DriverEnvelopeExecutor: Send + Sync {
    async fn metadata(&self) -> Result<Value, DriverError>;
    async fn list_tools(&self) -> Result<Value, DriverError>;
    async fn call(&self, name: String, arguments: Value) -> Result<Value, DriverError>;
    fn close(&self);
}

struct NativeExecutor {
    driver: Arc<CuaDriver>,
    session: Arc<CuaDriverSession>,
}

#[async_trait]
impl DriverEnvelopeExecutor for NativeExecutor {
    async fn metadata(&self) -> Result<Value, DriverError> {
        serde_json::to_value(self.driver.metadata().await?).map_err(json_error)
    }

    async fn list_tools(&self) -> Result<Value, DriverError> {
        let mut inventory: Value =
            serde_json::from_str(&self.driver.list_tools_json().await?).map_err(json_error)?;
        if let Some(tools) = inventory.get_mut("tools").and_then(Value::as_array_mut) {
            tools.retain(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(remote_tool)
            });
        }
        Ok(inventory)
    }

    async fn call(&self, name: String, arguments: Value) -> Result<Value, DriverError> {
        let result = self.session.call_tool(name, arguments.to_string()).await?;
        serde_json::from_str(&result.raw_json).map_err(json_error)
    }

    fn close(&self) {
        self.session.close();
    }
}

fn json_error(error: serde_json::Error) -> DriverError {
    DriverError::Protocol {
        reason: error.to_string(),
    }
}

struct State {
    closed: bool,
    // Never evict request identities within a connection: eviction could replay
    // an effectful request. A full ledger requires a new explicit connection.
    requests: HashMap<String, watch::Sender<bool>>,
}

/// Bounded connection state shared by unary exchange/cancel/close requests.
pub struct DriverEnvelopeReceiver {
    generation: String,
    executor: Arc<dyn DriverEnvelopeExecutor>,
    state: Mutex<State>,
    serial: tokio::sync::Mutex<()>,
}

impl DriverEnvelopeReceiver {
    /// Bind a native session using options supplied by trusted guest host code,
    /// never deserialized directly from an untrusted tool request.
    pub fn for_driver(
        driver: Arc<CuaDriver>,
        options: TrustedSessionOptions,
    ) -> Result<Arc<Self>, DriverError> {
        let session = driver.create_trusted_session(options)?;
        Ok(Self::new(Arc::new(NativeExecutor { driver, session })))
    }

    pub fn new(executor: Arc<dyn DriverEnvelopeExecutor>) -> Arc<Self> {
        Arc::new(Self {
            generation: Uuid::new_v4().to_string(),
            executor,
            state: Mutex::new(State {
                closed: false,
                requests: HashMap::new(),
            }),
            serial: tokio::sync::Mutex::new(()),
        })
    }

    pub fn generation(&self) -> &str {
        &self.generation
    }

    pub fn capabilities(&self) -> DriverChannelCapabilities {
        DriverChannelCapabilities {
            minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
            supports_cancellation: true,
        }
    }

    /// Reject stale generations before dispatch. The outer carrier must still
    /// authorize the request; matching a generation grants no access.
    pub async fn exchange(
        &self,
        generation: &str,
        request: DriverRequestEnvelope,
    ) -> DriverResponseEnvelope {
        if generation != self.generation {
            return refusal(
                &request,
                "stale_connection",
                "Driver connection was replaced",
                true,
            );
        }
        if let Err(reason) = validate_request(&request) {
            return refusal(&request, "invalid_request", reason, true);
        }
        let mut cancelled = {
            let mut state = self.state.lock().unwrap();
            if state.closed {
                return refusal(
                    &request,
                    "connection_closed",
                    "Driver connection is closed",
                    true,
                );
            }
            if state.requests.contains_key(&request.request_id) {
                // The first request might still be executing or have lost its
                // response. Refusing a duplicate does not prove its completion.
                return refusal(
                    &request,
                    "duplicate_request",
                    "Request identity already used",
                    false,
                );
            }
            if state.requests.len() >= MAX_REQUESTS {
                return refusal(
                    &request,
                    "connection_full",
                    "Open a new Driver connection",
                    true,
                );
            }
            let (sender, receiver) = watch::channel(false);
            state.requests.insert(request.request_id.clone(), sender);
            receiver
        };
        let remaining = request.deadline_unix_ms.saturating_sub(now_ms());
        let timeout = tokio::time::sleep(Duration::from_millis(remaining as u64));
        tokio::pin!(timeout);
        let guard = tokio::select! {
            biased;
            _ = cancelled.wait_for(|value| *value) => {
                return refusal(&request, "request_cancelled", "Cancelled before dispatch", true);
            }
            _ = &mut timeout => {
                return refusal(&request, "deadline_exceeded", "Deadline elapsed before dispatch", true);
            }
            guard = self.serial.lock() => guard,
        };
        if *cancelled.borrow() || self.state.lock().unwrap().closed {
            return refusal(
                &request,
                "request_cancelled",
                "Cancelled before dispatch",
                true,
            );
        }
        if now_ms() >= request.deadline_unix_ms {
            return refusal(
                &request,
                "deadline_exceeded",
                "Deadline elapsed before dispatch",
                true,
            );
        }
        let mut in_flight = InFlight {
            receiver: self,
            completed: false,
        };
        let dispatch = async {
            match request.operation.as_str() {
                "metadata" => self.executor.metadata().await,
                "list" => self.executor.list_tools().await,
                "call" => {
                    self.executor
                        .call(
                            request.name.clone().unwrap(),
                            request
                                .arguments
                                .clone()
                                .unwrap_or_else(|| serde_json::json!({})),
                        )
                        .await
                }
                _ => unreachable!("validated operation"),
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancelled.wait_for(|value| *value) => None,
            _ = &mut timeout => None,
            result = dispatch => Some(result),
        };
        let response = match result {
            None => {
                // A native effect might outlive a dropped future. Invalidate
                // this session instead of permitting another action or replay.
                self.close();
                refusal(
                    &request,
                    "action_interrupted",
                    "Request interrupted after dispatch",
                    request.operation != "call",
                )
            }
            Some(Ok(result)) => DriverResponseEnvelope {
                envelope_version: DRIVER_ENVELOPE_VERSION,
                request_id: request.request_id,
                ok: true,
                result: Some(result),
                error: None,
                error_code: None,
                completion_known: true,
            },
            Some(Err(error)) => {
                let (code, known) = match &error {
                    DriverError::ActionInterrupted { .. } => ("action_interrupted", false),
                    DriverError::Tool { error_code, .. } => (error_code.as_str(), true),
                    _ => ("driver_error", request.operation != "call"),
                };
                if !known {
                    self.close();
                }
                refusal(&request, code, &error.to_string(), known)
            }
        };
        in_flight.completed = true;
        drop(guard);
        response
    }

    /// Idempotent cancellation also records an early cancel, so a delayed
    /// exchange with the same request identity cannot execute afterward.
    pub fn cancel(&self, generation: &str, request_id: &str) -> Result<(), String> {
        if generation != self.generation {
            return Err("stale Driver connection".into());
        }
        if !valid_id(request_id) {
            return Err("invalid request identity".into());
        }
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Ok(());
        }
        if let Some(sender) = state.requests.get(request_id) {
            sender.send_replace(true);
        } else if state.requests.len() < MAX_REQUESTS {
            state
                .requests
                .insert(request_id.into(), watch::channel(true).0);
        } else {
            return Err("Driver connection request ledger is full".into());
        }
        Ok(())
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        for sender in state.requests.values() {
            sender.send_replace(true);
        }
        drop(state);
        self.executor.close();
    }
}

impl Drop for DriverEnvelopeReceiver {
    fn drop(&mut self) {
        self.close();
    }
}

struct InFlight<'a> {
    receiver: &'a DriverEnvelopeReceiver,
    completed: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.receiver.close();
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn validate_request(request: &DriverRequestEnvelope) -> Result<(), &'static str> {
    if request.envelope_version != DRIVER_ENVELOPE_VERSION {
        return Err("Unsupported envelope version");
    }
    if !valid_id(&request.request_id) {
        return Err("Invalid request identity");
    }
    let now = now_ms();
    if request.deadline_unix_ms <= now {
        return Err("Request deadline has elapsed");
    }
    if request.deadline_unix_ms - now > MAX_DEADLINE_MS {
        return Err("Request deadline exceeds limit");
    }
    if serde_json::to_vec(request).map_or(true, |bytes| bytes.len() > MAX_REQUEST_BYTES) {
        return Err("Request exceeds size limit");
    }
    match request.operation.as_str() {
        "metadata" | "list" if request.name.is_none() && request.arguments.is_none() => Ok(()),
        "call" => {
            if !request.name.as_deref().is_some_and(remote_tool) {
                return Err("Operation is not supported by this remote connection");
            }
            if let Some(arguments) = &request.arguments {
                let object = arguments
                    .as_object()
                    .ok_or("Arguments must be a JSON object")?;
                if object.keys().any(|key| key.starts_with('_')) {
                    return Err("Private session fields are not accepted");
                }
                for field in ["screenshot_out_file", "image_path", "file_path"] {
                    if object.get(field).is_some_and(|value| !value.is_null()) {
                        return Err(
                            "Local filesystem paths are not supported by this remote connection",
                        );
                    }
                }
            }
            Ok(())
        }
        _ => Err("Unsupported envelope operation"),
    }
}

// A deliberate first-slice desktop subset. Session authority, process/host
// management, recording paths, shell and files are not guest wire operations.
fn remote_tool(name: &str) -> bool {
    matches!(
        name,
        "get_desktop_state"
            | "get_screen_size"
            | "get_cursor_position"
            | "click"
            | "scroll"
            | "drag"
            | "move_cursor"
            | "type_text"
            | "press_key"
            | "hotkey"
            | "invoke_menu"
            | "set_window_frame"
            | "clipboard_read"
            | "clipboard_write"
            | "verify_state"
            | "get_agent_cursor_state"
            | "set_agent_cursor_enabled"
            | "set_agent_cursor_motion"
            | "set_agent_cursor_theme"
    )
}

fn refusal(
    request: &DriverRequestEnvelope,
    code: &str,
    reason: &str,
    known: bool,
) -> DriverResponseEnvelope {
    DriverResponseEnvelope {
        envelope_version: DRIVER_ENVELOPE_VERSION,
        request_id: request.request_id.clone(),
        ok: false,
        result: None,
        error: Some(reason.into()),
        error_code: Some(code.into()),
        completion_known: known,
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
#[path = "remote_receiver_tests.rs"]
mod tests;
