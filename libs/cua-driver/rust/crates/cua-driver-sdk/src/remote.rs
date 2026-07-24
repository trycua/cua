//! Transport-free remote Driver connection backend.
//!
//! A carrier (gRPC, HTTP/2, Fleet, or another authenticated channel) implements
//! [`DriverEnvelopeChannel`]. The SDK continues to expose the same typed
//! `CuaDriver` methods and never depends on that carrier.

use crate::{DriverError, DriverMetadata, TrustedSessionOptions};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const DRIVER_ENVELOPE_VERSION: u32 = 1;
const DEFAULT_REMOTE_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverRequestEnvelope {
    pub envelope_version: u32,
    pub request_id: String,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    pub deadline_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverResponseEnvelope {
    pub envelope_version: u32,
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// False means the carrier lost the response after dispatch and cannot
    /// prove whether an external side effect occurred.
    pub completion_known: bool,
}

/// Authenticated asynchronous carrier for generated Driver envelopes.
///
/// `bind_session` returns a new opaque channel already bound to the effective
/// session context. It never returns a serialized lease, token, or session
/// authority for the caller to replay on another connection.
#[async_trait]
pub trait DriverEnvelopeChannel: Send + Sync {
    async fn exchange(
        &self,
        request: DriverRequestEnvelope,
    ) -> Result<DriverResponseEnvelope, String>;

    async fn bind_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<dyn DriverEnvelopeChannel>, String>;

    async fn close(&self) -> Result<(), String>;

    fn authenticated_principal(&self) -> &str;
    fn connection_generation(&self) -> &str;
}

pub(crate) struct RemoteDriverClient {
    channel: Arc<dyn DriverEnvelopeChannel>,
    closed: AtomicBool,
}

impl RemoteDriverClient {
    pub(crate) fn connect(
        channel: Arc<dyn DriverEnvelopeChannel>,
    ) -> Result<Arc<Self>, DriverError> {
        if channel.authenticated_principal().trim().is_empty()
            || channel.connection_generation().trim().is_empty()
        {
            return Err(DriverError::Configuration {
                reason:
                    "remote Driver channels require an authenticated principal and connection generation"
                        .into(),
            });
        }
        Ok(Arc::new(Self {
            channel,
            closed: AtomicBool::new(false),
        }))
    }

    pub(crate) fn is_available(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }

    pub(crate) async fn metadata(&self) -> Result<DriverMetadata, DriverError> {
        let value = exchange(&self.channel, "metadata", None, None).await?;
        serde_json::from_value(value).map_err(|error| DriverError::Protocol {
            reason: format!("remote Driver returned invalid metadata: {error}"),
        })
    }

    pub(crate) async fn list_tools(&self) -> Result<Value, DriverError> {
        exchange(&self.channel, "list", None, None).await
    }

    pub(crate) async fn invoke(&self, name: &str, arguments: Value) -> Result<Value, DriverError> {
        exchange(
            &self.channel,
            "call",
            Some(name.to_owned()),
            Some(arguments),
        )
        .await
    }

    pub(crate) async fn bind_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<RemoteBoundSession>, DriverError> {
        let channel = self
            .channel
            .bind_session(options)
            .await
            .map_err(|reason| DriverError::Remote { reason })?;
        if channel.authenticated_principal() != self.channel.authenticated_principal()
            || channel.connection_generation() != self.channel.connection_generation()
        {
            return Err(DriverError::Remote {
                reason: "remote bound session changed principal or connection generation".into(),
            });
        }
        Ok(Arc::new(RemoteBoundSession {
            channel,
            closed: AtomicBool::new(false),
        }))
    }

    pub(crate) async fn shutdown(&self) -> Result<(), DriverError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.channel
            .close()
            .await
            .map_err(|reason| DriverError::Remote { reason })
    }
}

pub(crate) struct RemoteBoundSession {
    channel: Arc<dyn DriverEnvelopeChannel>,
    closed: AtomicBool,
}

impl RemoteBoundSession {
    pub(crate) async fn invoke(&self, name: &str, arguments: Value) -> Result<Value, DriverError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DriverError::Shutdown);
        }
        exchange(
            &self.channel,
            "call",
            Some(name.to_owned()),
            Some(arguments),
        )
        .await
    }

    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let channel = self.channel.clone();
        std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                let _ = runtime.block_on(channel.close());
            }
        });
    }
}

impl Drop for RemoteBoundSession {
    fn drop(&mut self) {
        self.close();
    }
}

async fn exchange(
    channel: &Arc<dyn DriverEnvelopeChannel>,
    operation: &str,
    name: Option<String>,
    arguments: Option<Value>,
) -> Result<Value, DriverError> {
    let request_id = Uuid::new_v4().to_string();
    let response = channel
        .exchange(DriverRequestEnvelope {
            envelope_version: DRIVER_ENVELOPE_VERSION,
            request_id: request_id.clone(),
            operation: operation.into(),
            name: name.clone(),
            arguments,
            deadline_unix_ms: now_unix_ms() + DEFAULT_REMOTE_DEADLINE.as_millis(),
        })
        .await
        .map_err(|reason| {
            if operation == "call" {
                DriverError::ActionInterrupted {
                    completion: crate::worker::ActionCompletion::Unknown,
                    reason,
                }
            } else {
                DriverError::Remote { reason }
            }
        })?;
    if response.envelope_version != DRIVER_ENVELOPE_VERSION || response.request_id != request_id {
        return Err(DriverError::Protocol {
            reason: "remote Driver response identity mismatch".into(),
        });
    }
    if !response.completion_known {
        return Err(DriverError::ActionInterrupted {
            completion: crate::worker::ActionCompletion::Unknown,
            reason: response
                .error
                .unwrap_or_else(|| "remote completion is unknown".into()),
        });
    }
    if !response.ok {
        return Err(DriverError::Tool {
            tool: name.unwrap_or_else(|| operation.into()),
            message: response
                .error
                .unwrap_or_else(|| "remote Driver request failed".into()),
            error_code: response.error_code.unwrap_or_default(),
        });
    }
    Ok(response.result.unwrap_or(Value::Null))
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
