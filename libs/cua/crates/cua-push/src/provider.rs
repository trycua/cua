// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The provider interface: how one alert reaches one phone.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::event::Payload;
use crate::registry::Registration;

/// Why a send failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// The token is no longer valid (the app was removed, or the token
    /// belongs to another environment): the registration is dropped.
    #[error("the phone's push token is no longer valid")]
    InvalidToken,
    /// Try again later: the provider was busy or unreachable.
    #[error("the provider could not be reached: {0}")]
    Transient(String),
    /// The provider refused the alert (a configuration problem).
    #[error("the provider refused the alert: {0}")]
    Rejected(String),
    /// No provider is set up on this relay.
    #[error("push is not set up on this relay")]
    NotConfigured,
}

/// Sends one alert to one phone.
#[async_trait]
pub trait PushProvider: Send + Sync {
    /// A name for logs and the status line.
    fn name(&self) -> &'static str;
    /// Whether this provider serves `reg` (an APNs provider serves iOS).
    fn serves(&self, reg: &Registration) -> bool;
    async fn send(&self, reg: &Registration, payload: &Payload) -> Result<(), ProviderError>;
}

/// Nothing is configured: every send says so. Registrations are still kept,
/// so pushes start the moment a provider is set up.
pub struct NoProvider;

#[async_trait]
impl PushProvider for NoProvider {
    fn name(&self) -> &'static str {
        "none"
    }
    fn serves(&self, _: &Registration) -> bool {
        true
    }
    async fn send(&self, _: &Registration, _: &Payload) -> Result<(), ProviderError> {
        Err(ProviderError::NotConfigured)
    }
}

/// Appends each alert it is asked to send to a JSON-lines file instead of
/// sending it, for a local run without an Apple key. A line holds the
/// registration's id and the payload, never the device token.
pub struct LogProvider {
    path: std::path::PathBuf,
}

impl LogProvider {
    pub fn new(path: impl Into<std::path::PathBuf>) -> LogProvider {
        LogProvider { path: path.into() }
    }
}

#[async_trait]
impl PushProvider for LogProvider {
    fn name(&self) -> &'static str {
        "log"
    }
    fn serves(&self, _: &Registration) -> bool {
        true
    }
    async fn send(&self, reg: &Registration, payload: &Payload) -> Result<(), ProviderError> {
        use std::io::Write as _;
        let line = serde_json::json!({
            "registration": reg.id,
            "platform": reg.platform,
            "payload": payload.apns_json(),
        });
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| ProviderError::Transient(e.to_string()))?;
        writeln!(f, "{line}").map_err(|e| ProviderError::Transient(e.to_string()))
    }
}

/// Records what it was asked to send, for tests (and for a local run that
/// has no Apple key). It can be told to fail for a token.
#[derive(Default)]
pub struct MockProvider {
    sent: Mutex<Vec<(Registration, Payload)>>,
    fail: Mutex<Vec<(String, ProviderError)>>,
}

impl MockProvider {
    pub fn new() -> MockProvider {
        MockProvider::default()
    }

    /// Everything sent so far.
    pub fn sent(&self) -> Vec<(Registration, Payload)> {
        self.sent.lock().expect("mock").clone()
    }

    /// Sends to `token` fail with `error` from now on.
    pub fn fail_token(&self, token: &str, error: ProviderError) {
        self.fail
            .lock()
            .expect("mock")
            .push((token.to_string(), error));
    }
}

#[async_trait]
impl PushProvider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn serves(&self, _: &Registration) -> bool {
        true
    }
    async fn send(&self, reg: &Registration, payload: &Payload) -> Result<(), ProviderError> {
        if let Some((_, e)) = self
            .fail
            .lock()
            .expect("mock")
            .iter()
            .find(|(t, _)| *t == reg.token)
        {
            return Err(e.clone());
        }
        self.sent
            .lock()
            .expect("mock")
            .push((reg.clone(), payload.clone()));
        Ok(())
    }
}
