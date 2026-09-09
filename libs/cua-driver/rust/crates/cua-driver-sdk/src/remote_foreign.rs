//! Foreign-language adapter for an authenticated remote envelope carrier.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::remote::{
    DriverChannelCapabilities, DriverEnvelopeChannel, DriverRequestEnvelope, DriverResponseEnvelope,
};
use crate::{CuaDriver, CuaDriverSession, DriverError, TrustedSessionOptions};

/// Opaque context established by the authenticated carrier, not guest claims.
#[derive(Clone, Debug, uniffi::Record)]
pub struct ForeignDriverChannelIdentity {
    pub authenticated_principal: String,
    pub connection_generation: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ForeignDriverChannelCapabilities {
    pub minimum_envelope_version: u32,
    pub maximum_envelope_version: u32,
    pub supports_cancellation: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ForeignDriverRequestEnvelope {
    pub envelope_version: u32,
    pub request_id: String,
    pub operation: String,
    pub name: Option<String>,
    pub arguments_json: Option<String>,
    pub deadline_unix_ms: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ForeignDriverResponseEnvelope {
    pub envelope_version: u32,
    pub request_id: String,
    pub ok: bool,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub completion_known: bool,
}

#[derive(Debug, Error, uniffi::Error)]
pub enum ForeignDriverChannelError {
    #[error("remote carrier failed: {reason}")]
    Failed { reason: String },
}

#[derive(Clone, uniffi::Record)]
pub struct ForeignDriverBoundChannel {
    pub channel: Arc<dyn ForeignDriverEnvelopeChannel>,
}

/// Trusted host implementation of the existing remote transport contract.
/// Identity is captured once when each channel is adapted. A bound channel
/// must retain its parent's authenticated principal and connection generation.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait ForeignDriverEnvelopeChannel: Send + Sync + 'static {
    fn identity(&self) -> Result<ForeignDriverChannelIdentity, ForeignDriverChannelError>;
    async fn negotiate(
        &self,
    ) -> Result<ForeignDriverChannelCapabilities, ForeignDriverChannelError>;
    async fn exchange(
        &self,
        request: ForeignDriverRequestEnvelope,
    ) -> Result<ForeignDriverResponseEnvelope, ForeignDriverChannelError>;
    async fn bind_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<ForeignDriverBoundChannel, ForeignDriverChannelError>;
    async fn cancel(&self, request_id: String) -> Result<(), ForeignDriverChannelError>;
    async fn close(&self) -> Result<(), ForeignDriverChannelError>;
}

struct ForeignChannelAdapter {
    channel: Arc<dyn ForeignDriverEnvelopeChannel>,
    identity: ForeignDriverChannelIdentity,
}

impl ForeignChannelAdapter {
    fn new(channel: Arc<dyn ForeignDriverEnvelopeChannel>) -> Result<Arc<Self>, String> {
        let identity = channel.identity().map_err(|error| error.to_string())?;
        Ok(Arc::new(Self { channel, identity }))
    }
}

#[async_trait]
impl DriverEnvelopeChannel for ForeignChannelAdapter {
    async fn negotiate(&self) -> Result<DriverChannelCapabilities, String> {
        let capabilities = self
            .channel
            .negotiate()
            .await
            .map_err(|error| error.to_string())?;
        Ok(DriverChannelCapabilities {
            minimum_envelope_version: capabilities.minimum_envelope_version,
            maximum_envelope_version: capabilities.maximum_envelope_version,
            supports_cancellation: capabilities.supports_cancellation,
        })
    }

    async fn exchange(
        &self,
        request: DriverRequestEnvelope,
    ) -> Result<DriverResponseEnvelope, String> {
        let request = ForeignDriverRequestEnvelope {
            envelope_version: request.envelope_version,
            request_id: request.request_id,
            operation: request.operation,
            name: request.name,
            arguments_json: request.arguments.map(|value| value.to_string()),
            deadline_unix_ms: u64::try_from(request.deadline_unix_ms)
                .map_err(|_| "remote request deadline exceeds foreign u64 range".to_owned())?,
        };
        let response = self
            .channel
            .exchange(request)
            .await
            .map_err(|error| error.to_string())?;
        Ok(DriverResponseEnvelope {
            envelope_version: response.envelope_version,
            request_id: response.request_id,
            ok: response.ok,
            result: response
                .result_json
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .map_err(|_| "remote carrier returned invalid result JSON".to_owned())?,
            error: response.error,
            error_code: response.error_code,
            completion_known: response.completion_known,
        })
    }

    async fn bind_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<dyn DriverEnvelopeChannel>, String> {
        let channel = self
            .channel
            .bind_session(options)
            .await
            .map_err(|error| error.to_string())?;
        let channel = channel.channel;
        let bound = match Self::new(channel.clone()) {
            Ok(bound)
                if bound.authenticated_principal() == self.authenticated_principal()
                    && bound.connection_generation() == self.connection_generation() =>
            {
                bound
            }
            result => {
                let _ = channel.close().await;
                return Err(match result {
                    Err(reason) => reason,
                    Ok(_) => {
                        "remote bound session changed principal or connection generation".into()
                    }
                });
            }
        };
        Ok(bound as Arc<dyn DriverEnvelopeChannel>)
    }

    async fn cancel(&self, request_id: &str) -> Result<(), String> {
        self.channel
            .cancel(request_id.to_owned())
            .await
            .map_err(|error| error.to_string())
    }

    async fn close(&self) -> Result<(), String> {
        self.channel
            .close()
            .await
            .map_err(|error| error.to_string())
    }

    fn authenticated_principal(&self) -> &str {
        &self.identity.authenticated_principal
    }

    fn connection_generation(&self) -> &str {
        &self.identity.connection_generation
    }
}

/// Connect the canonical typed Driver to a host-provided remote carrier.
#[uniffi::export]
pub fn connect_remote_channel(
    channel: Arc<dyn ForeignDriverEnvelopeChannel>,
) -> Result<Arc<CuaDriver>, DriverError> {
    CuaDriver::connect_remote(
        ForeignChannelAdapter::new(channel).map_err(|reason| DriverError::Remote { reason })?,
    )
}

/// Bind a logical remote session without serializing its authority.
#[uniffi::export(async_runtime = "tokio")]
pub async fn create_remote_trusted_session(
    driver: Arc<CuaDriver>,
    options: TrustedSessionOptions,
) -> Result<Arc<CuaDriverSession>, DriverError> {
    driver.create_remote_trusted_session(options).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Carrier(AtomicUsize);

    #[async_trait]
    impl ForeignDriverEnvelopeChannel for Carrier {
        fn identity(&self) -> Result<ForeignDriverChannelIdentity, ForeignDriverChannelError> {
            Ok(ForeignDriverChannelIdentity {
                authenticated_principal: "test-context".into(),
                connection_generation: "test-generation".into(),
            })
        }

        async fn negotiate(
            &self,
        ) -> Result<ForeignDriverChannelCapabilities, ForeignDriverChannelError> {
            unreachable!()
        }

        async fn exchange(
            &self,
            request: ForeignDriverRequestEnvelope,
        ) -> Result<ForeignDriverResponseEnvelope, ForeignDriverChannelError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.deadline_unix_ms, u64::MAX);
            Ok(ForeignDriverResponseEnvelope {
                envelope_version: request.envelope_version,
                request_id: request.request_id,
                ok: true,
                result_json: request.arguments_json,
                error: None,
                error_code: None,
                completion_known: true,
            })
        }

        async fn bind_session(
            &self,
            _: TrustedSessionOptions,
        ) -> Result<ForeignDriverBoundChannel, ForeignDriverChannelError> {
            unreachable!()
        }

        async fn cancel(&self, _: String) -> Result<(), ForeignDriverChannelError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), ForeignDriverChannelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn foreign_projection_preserves_null_absence_and_checks_deadline() {
        let carrier = Arc::new(Carrier(AtomicUsize::new(0)));
        let adapter = ForeignChannelAdapter::new(carrier.clone()).unwrap();
        let mut request = DriverRequestEnvelope {
            envelope_version: 1,
            request_id: "test-request".into(),
            operation: "call".into(),
            name: Some("health_report".into()),
            arguments: None,
            deadline_unix_ms: u64::MAX as u128,
        };
        assert_eq!(
            adapter.exchange(request.clone()).await.unwrap().result,
            None
        );
        request.arguments = Some(serde_json::Value::Null);
        assert_eq!(
            adapter.exchange(request.clone()).await.unwrap().result,
            Some(serde_json::Value::Null)
        );
        request.deadline_unix_ms += 1;
        assert!(adapter
            .exchange(request)
            .await
            .unwrap_err()
            .contains("deadline"));
        assert_eq!(carrier.0.load(Ordering::SeqCst), 2);
    }
}
