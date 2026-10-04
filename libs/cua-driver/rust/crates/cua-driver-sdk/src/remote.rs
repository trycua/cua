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

/// Version and lifecycle features negotiated with an authenticated remote
/// carrier before any Driver envelope is dispatched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriverChannelCapabilities {
    pub minimum_envelope_version: u32,
    pub maximum_envelope_version: u32,
    pub supports_cancellation: bool,
}

/// Authenticated asynchronous carrier for generated Driver envelopes.
///
/// `bind_session` returns a new opaque channel already bound to the effective
/// session context. It never returns a serialized lease, token, or session
/// authority for the caller to replay on another connection.
#[async_trait]
pub trait DriverEnvelopeChannel: Send + Sync {
    /// Report the carrier's compatible envelope range and lifecycle support.
    ///
    /// The default preserves source compatibility for carriers compiled
    /// against the first public trait revision, but deliberately reports no
    /// cancellation support so new action dispatch fails closed until that
    /// carrier implements the lifecycle contract.
    async fn negotiate(&self) -> Result<DriverChannelCapabilities, String> {
        Ok(DriverChannelCapabilities {
            minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
            supports_cancellation: false,
        })
    }

    async fn exchange(
        &self,
        request: DriverRequestEnvelope,
    ) -> Result<DriverResponseEnvelope, String>;

    async fn bind_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<dyn DriverEnvelopeChannel>, String>;

    async fn close(&self) -> Result<(), String>;

    /// Cancel one in-flight request by its opaque request identity. Carriers
    /// must make this idempotent because local future destruction can race a
    /// response already in transit.
    async fn cancel(&self, _request_id: &str) -> Result<(), String> {
        Err("remote Driver carrier does not implement request cancellation".into())
    }

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

    pub(crate) async fn list_host_sessions(&self) -> Result<Value, DriverError> {
        exchange(&self.channel, "sessions_list", None, None).await
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
        negotiate(&self.channel).await?;
        let channel = self
            .channel
            .bind_session(options)
            .await
            .map_err(|reason| DriverError::Remote { reason })?;
        let mut pending = PendingBoundChannel(Some(channel.clone()));
        if channel.authenticated_principal() != self.channel.authenticated_principal()
            || channel.connection_generation() != self.channel.connection_generation()
        {
            pending.close().await;
            return Err(DriverError::Remote {
                reason: "remote bound session changed principal or connection generation".into(),
            });
        }
        if let Err(error) = negotiate(&channel).await {
            pending.close().await;
            return Err(error);
        }
        pending.0.take();
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

/// Own a newly bound channel until validation completes, including when the
/// caller cancels the binding future during negotiation.
struct PendingBoundChannel(Option<Arc<dyn DriverEnvelopeChannel>>);

impl PendingBoundChannel {
    async fn close(&mut self) {
        if let Some(channel) = &self.0 {
            let _ = channel.close().await;
        }
        self.0.take();
    }
}

impl Drop for PendingBoundChannel {
    fn drop(&mut self) {
        let Some(channel) = self.0.take() else { return };
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
    negotiate(channel).await?;
    let request_id = Uuid::new_v4().to_string();
    let mut cancellation = RemoteCancellationGuard::new(channel.clone(), request_id.clone());
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
    cancellation.disarm();
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

async fn negotiate(channel: &Arc<dyn DriverEnvelopeChannel>) -> Result<(), DriverError> {
    let capabilities = channel
        .negotiate()
        .await
        .map_err(|reason| DriverError::Remote { reason })?;
    if capabilities.minimum_envelope_version > DRIVER_ENVELOPE_VERSION
        || capabilities.maximum_envelope_version < DRIVER_ENVELOPE_VERSION
    {
        return Err(DriverError::Protocol {
            reason: format!(
                "remote Driver envelope version {} is outside carrier range {}..={}",
                DRIVER_ENVELOPE_VERSION,
                capabilities.minimum_envelope_version,
                capabilities.maximum_envelope_version
            ),
        });
    }
    if !capabilities.supports_cancellation {
        return Err(DriverError::Protocol {
            reason: "remote Driver carrier does not support request cancellation".into(),
        });
    }
    Ok(())
}

struct RemoteCancellationGuard {
    channel: Arc<dyn DriverEnvelopeChannel>,
    request_id: Option<String>,
}

impl RemoteCancellationGuard {
    fn new(channel: Arc<dyn DriverEnvelopeChannel>, request_id: String) -> Self {
        Self {
            channel,
            request_id: Some(request_id),
        }
    }

    fn disarm(&mut self) {
        self.request_id = None;
    }
}

impl Drop for RemoteCancellationGuard {
    fn drop(&mut self) {
        let Some(request_id) = self.request_id.take() else {
            return;
        };
        let channel = self.channel.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = channel.cancel(&request_id).await;
            });
        } else {
            std::thread::spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    let _ = runtime.block_on(channel.cancel(&request_id));
                }
            });
        }
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionPermissionMode;
    use std::sync::atomic::AtomicUsize;

    struct BindingCarrier {
        child: Option<Arc<BindingCarrier>>,
        principal: &'static str,
        version: u32,
        cancellation: bool,
        closes: AtomicUsize,
    }

    #[async_trait]
    impl DriverEnvelopeChannel for BindingCarrier {
        async fn negotiate(&self) -> Result<DriverChannelCapabilities, String> {
            Ok(DriverChannelCapabilities {
                minimum_envelope_version: self.version,
                maximum_envelope_version: self.version,
                supports_cancellation: self.cancellation,
            })
        }

        async fn exchange(
            &self,
            _: DriverRequestEnvelope,
        ) -> Result<DriverResponseEnvelope, String> {
            panic!("rejected binding must not dispatch")
        }

        async fn bind_session(
            &self,
            _: TrustedSessionOptions,
        ) -> Result<Arc<dyn DriverEnvelopeChannel>, String> {
            Ok(self.child.as_ref().unwrap().clone())
        }

        async fn close(&self) -> Result<(), String> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn authenticated_principal(&self) -> &str {
            self.principal
        }
        fn connection_generation(&self) -> &str {
            "generation"
        }
    }

    #[tokio::test]
    async fn rejected_bound_channels_close_before_returning() {
        for (principal, version, cancellation) in [
            ("other", 1, true),
            ("principal", 2, true),
            ("principal", 1, false),
        ] {
            let child = Arc::new(BindingCarrier {
                child: None,
                principal,
                version,
                cancellation,
                closes: AtomicUsize::new(0),
            });
            let parent = Arc::new(BindingCarrier {
                child: Some(child.clone()),
                principal: "principal",
                version: 1,
                cancellation: true,
                closes: AtomicUsize::new(0),
            });
            let client = RemoteDriverClient::connect(parent.clone()).unwrap();
            let result = client
                .bind_session(TrustedSessionOptions {
                    public_session: "test".into(),
                    mode: SessionPermissionMode::Standard,
                    ttl_seconds: 60,
                    idle_ttl_seconds: 30,
                    capability_manifest_path: None,
                    bounded_manifest_path: None,
                })
                .await;
            assert!(result.is_err());
            assert_eq!(child.closes.load(Ordering::SeqCst), 1);
            assert_eq!(parent.closes.load(Ordering::SeqCst), 0);
        }
    }
}
