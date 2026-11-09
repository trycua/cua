// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Enrolled devices, as the relay's device registry knows them.
//!
//! [`HttpDeviceDirectory`] calls the relay's cluster-internal, read-only
//! `GET /internal/v1/devices/{id}` with the relay's internal token (the
//! relay's ingress never routes `/internal/`, so this is the in-cluster
//! Service URL). [`MemoryDeviceDirectory`] is for tests.
//!
//! A device id is `dev_` + the first 24 hex characters of SHA-256 over the
//! device's uncompressed P-256 public key (`0x04 || x || y`), exactly as
//! `cua-host` and the relay form it ([`device_id`]).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A device's public key as a JWK (P-256).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcJwk {
    /// `EC`.
    pub kty: String,
    /// `P-256`.
    pub crv: String,
    /// X coordinate, base64url (32 bytes).
    pub x: String,
    /// Y coordinate, base64url (32 bytes).
    pub y: String,
}

impl EcJwk {
    /// The uncompressed point `0x04 || x || y`, when this is a well-formed
    /// P-256 key.
    pub fn uncompressed(&self) -> Option<Vec<u8>> {
        use base64::Engine as _;
        if self.kty != "EC" || self.crv != "P-256" {
            return None;
        }
        let b64 = crate::jwk::b64();
        let x = b64.decode(&self.x).ok()?;
        let y = b64.decode(&self.y).ok()?;
        if x.len() != 32 || y.len() != 32 {
            return None;
        }
        Some([&[4u8][..], &x, &y].concat())
    }
}

/// The device id of an uncompressed P-256 public key.
pub fn device_id(uncompressed: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, uncompressed);
    format!("dev_{}", &hex::encode(digest.as_ref())[..24])
}

/// A device as the relay reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// `dev_…`.
    pub id: String,
    /// The Keycloak account it belongs to.
    pub account: String,
    /// Its key.
    pub public_jwk: Option<EcJwk>,
    /// Enrolled and within its TTL.
    pub enrolled: bool,
    /// Revoked.
    pub revoked: bool,
}

/// Looks devices up.
#[async_trait]
pub trait DeviceDirectory: Send + Sync {
    /// The device, or `None` when the registry has no such device.
    async fn device(&self, id: &str) -> Result<Option<DeviceInfo>, String>;
}

/// The relay's internal device lookup over HTTP.
pub struct HttpDeviceDirectory {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for HttpDeviceDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpDeviceDirectory")
            .field("base", &self.base)
            .finish()
    }
}

impl HttpDeviceDirectory {
    /// `base` is the relay's in-cluster URL; `token` its internal token.
    pub fn new(base: &str, token: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
        }
    }
}

fn valid_device_id(id: &str) -> bool {
    id.strip_prefix("dev_")
        .is_some_and(|h| h.len() == 24 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[async_trait]
impl DeviceDirectory for HttpDeviceDirectory {
    async fn device(&self, id: &str) -> Result<Option<DeviceInfo>, String> {
        if !valid_device_id(id) {
            return Ok(None);
        }
        let url = format!("{}/internal/v1/devices/{id}", self.base);
        let r = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("device lookup: {}", e.without_url()))?;
        match r.status() {
            reqwest::StatusCode::OK => r
                .json::<DeviceInfo>()
                .await
                .map(Some)
                .map_err(|e| format!("device lookup: {}", e.without_url())),
            // The relay answers a known-route 404 with JSON
            // `device_not_found`; a plain-text 404 means the route is off
            // (no internal token configured there), which is an outage.
            reqwest::StatusCode::NOT_FOUND => {
                let body = r.text().await.unwrap_or_default();
                if body.contains("device_not_found") {
                    Ok(None)
                } else {
                    Err("device lookup: the relay's internal device route is not enabled".into())
                }
            }
            status => Err(format!("device lookup: relay answered {status}")),
        }
    }
}

/// Devices held in memory (tests).
#[derive(Default)]
pub struct MemoryDeviceDirectory {
    devices: Mutex<HashMap<String, DeviceInfo>>,
}

impl MemoryDeviceDirectory {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a device.
    pub fn put(&self, device: DeviceInfo) {
        self.devices
            .lock()
            .expect("devices")
            .insert(device.id.clone(), device);
    }
}

#[async_trait]
impl DeviceDirectory for MemoryDeviceDirectory {
    async fn device(&self, id: &str) -> Result<Option<DeviceInfo>, String> {
        Ok(self.devices.lock().expect("devices").get(id).cloned())
    }
}
