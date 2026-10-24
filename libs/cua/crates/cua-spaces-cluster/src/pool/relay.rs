// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay's pool API (`/v1/pool/*`, see `cua-relay`'s `pool` module) as
//! this machine uses it: the lease, the log and the mailbox. Every call
//! carries the account token and this device's session.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::coordinator::journal::{Journal, JournalError, LogEntry, LogReader};
use crate::pool::keys::{KeyDirectory, KeyError, KeyState};
use crate::pool::seal::{Keyring, Member};

/// What authenticates this device to the relay: the account token and the
/// device session header, fetched per call (they refresh).
#[async_trait]
pub trait PoolAuth: Send + Sync + 'static {
    /// `(header, value)` pairs for one call.
    async fn headers(&self) -> Result<Vec<(String, String)>, String>;

    /// Forgets a cached device session the relay refused (it restarted):
    /// the next call signs a fresh one.
    async fn reset(&self) {}
}

/// Why a pool call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    /// The relay has no pool API (an older relay) or account mode is off.
    #[error("this relay does not coordinate pools: {0}")]
    Unsupported(String),
    /// Someone else holds the lease (`held`), or this epoch is over
    /// (`fenced`, `lease_lost`).
    #[error("{code}: {message}")]
    Refused {
        /// The relay's code.
        code: String,
        /// Its words.
        message: String,
        /// Its JSON answer.
        body: Value,
    },
    /// Not reached, or failed for now.
    #[error("{0}")]
    Unavailable(String),
}

/// The relay's pool API for this account.
#[derive(Clone)]
pub struct RelayPool {
    base: String,
    account: String,
    http: reqwest::Client,
    auth: Arc<dyn PoolAuth>,
}

/// A granted lease.
#[derive(Clone, Debug, Deserialize)]
pub struct Granted {
    /// The epoch.
    pub epoch: u64,
    /// Its time to live.
    pub ttl_ms: u64,
    /// The log's head when granted.
    pub head: u64,
    /// A contender this holder ranks below, waiting: hand over.
    #[serde(default)]
    pub yield_to: Option<String>,
}

/// Building a client reads the operating system's root store (expensive on
/// macOS), and the lease loop makes a `RelayPool` every few seconds while
/// this device is signed out (F53): every `RelayPool` shares one client.
fn shared_http() -> reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        #[cfg(test)]
        CLIENT_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default()
    })
    .clone()
}

#[cfg(test)]
static CLIENT_BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl RelayPool {
    /// The pool API of the relay at `base` for `account` (who is signed
    /// in: a pool's log is the account's).
    pub fn new(base: &str, account: &str, auth: Arc<dyn PoolAuth>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            account: account.to_string(),
            http: shared_http(),
            auth,
        }
    }

    /// The relay's base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Which pool this is: the relay and the account.
    pub fn key(&self) -> String {
        format!("{}#{}", self.base, self.account)
    }

    async fn raw(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<(u16, Value), PoolError> {
        let (status, v) = self
            .raw_once(method.clone(), path, body.clone(), timeout)
            .await?;
        // A relay that restarted forgot every device session: sign a new
        // one and ask again, once.
        if status == 403 && v["code"] == "device_not_enrolled" {
            self.auth.reset().await;
            return self.raw_once(method, path, body, timeout).await;
        }
        Ok((status, v))
    }

    async fn raw_once(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<(u16, Value), PoolError> {
        let headers = self.auth.headers().await.map_err(PoolError::Unavailable)?;
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .timeout(timeout);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| PoolError::Unavailable(format!("relay {}: {e}", self.base)))?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        Ok((status, v))
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, PoolError> {
        let (status, v) = self.raw(method, path, body, timeout).await?;
        let status = reqwest::StatusCode::from_u16(status)
            .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_success() {
            return Ok(v);
        }
        let code = v["code"].as_str().unwrap_or_default().to_string();
        let message = v["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| status.to_string());
        match status.as_u16() {
            404 if code.is_empty() || code == "not_found" => Err(PoolError::Unsupported(message)),
            409 | 403 | 401 | 410 | 507 | 503 | 504 | 429 => Err(PoolError::Refused {
                code: if code.is_empty() {
                    status.as_u16().to_string()
                } else {
                    code
                },
                message,
                body: v,
            }),
            _ => Err(PoolError::Unavailable(format!("{status}: {message}"))),
        }
    }

    /// The pool's state: epoch, holder, contenders, head.
    pub async fn status(&self) -> Result<Value, PoolError> {
        self.call(
            reqwest::Method::GET,
            "/v1/pool",
            None,
            Duration::from_secs(10),
        )
        .await
    }

    /// Acquires (epoch 0) or renews the lease.
    pub async fn lease(
        &self,
        epoch: u64,
        name: &str,
        machine: &str,
        priority: u32,
        ttl: Duration,
    ) -> Result<Granted, PoolError> {
        let v = self
            .call(
                reqwest::Method::POST,
                "/v1/pool/lease",
                Some(json!({"epoch": epoch, "name": name, "machine": machine,
                            "priority": priority, "ttl_ms": ttl.as_millis() as u64})),
                Duration::from_secs(10),
            )
            .await?;
        serde_json::from_value(v).map_err(|e| PoolError::Unavailable(format!("lease: {e}")))
    }

    /// Gives the lease up.
    pub async fn release(&self, epoch: u64) -> Result<(), PoolError> {
        self.call(
            reqwest::Method::POST,
            "/v1/pool/release",
            Some(json!({"epoch": epoch})),
            Duration::from_secs(10),
        )
        .await
        .map(|_| ())
    }

    /// Sends a request to whoever coordinates and waits for its answer:
    /// `(status, body)` as the coordinator gave them. `params` are sealed
    /// ([`crate::pool::seal`]); `meta` is what the relay may read (a
    /// submit's run, job count and placement, for the account's audit). No
    /// coordinator (or it changed meanwhile) is [`PoolError::Refused`]
    /// with `no_coordinator` (or `retry`); keys that changed, `rekey`.
    pub async fn rpc(
        &self,
        method: &str,
        params: Value,
        meta: Value,
    ) -> Result<(u16, Value), PoolError> {
        let (status, v) = self
            .raw(
                reqwest::Method::POST,
                "/v1/pool/rpc",
                Some(json!({"method": method, "params": params, "meta": meta})),
                Duration::from_secs(75),
            )
            .await?;
        let code = v["code"].as_str().unwrap_or_default();
        match (status, code) {
            (404, "not_found") | (404, "") if v.get("error").is_none() || code == "not_found" => {
                Err(PoolError::Unsupported(
                    v["error"].as_str().unwrap_or("no pool API").to_string(),
                ))
            }
            (503, "no_coordinator")
            | (409, "rekey")
            | (503, "retry")
            | (504, "timeout")
            | (429, "busy")
            | (401, _)
            | (403, "device_not_enrolled") => Err(PoolError::Refused {
                code: if code.is_empty() {
                    status.to_string()
                } else {
                    code.to_string()
                },
                message: v["error"].as_str().unwrap_or_default().to_string(),
                body: v,
            }),
            _ => Ok((status, v)),
        }
    }

    /// Takes waiting requests (as the holder of `epoch`).
    pub async fn take(&self, epoch: u64, wait: Duration) -> Result<Vec<Value>, PoolError> {
        let v = self
            .call(
                reqwest::Method::POST,
                "/v1/pool/rpc/take",
                Some(json!({"epoch": epoch, "wait_ms": wait.as_millis() as u64})),
                wait + Duration::from_secs(10),
            )
            .await?;
        Ok(v["requests"].as_array().cloned().unwrap_or_default())
    }

    /// Answers requests.
    pub async fn reply(&self, epoch: u64, replies: Vec<Value>) -> Result<(), PoolError> {
        self.call(
            reqwest::Method::POST,
            "/v1/pool/rpc/reply",
            Some(json!({"epoch": epoch, "replies": replies})),
            Duration::from_secs(20),
        )
        .await
        .map(|_| ())
    }

    /// The log under `epoch` (a [`Journal`] and a [`LogReader`]).
    pub fn log(&self, epoch: u64) -> RelayLog {
        RelayLog {
            pool: self.clone(),
            epoch,
        }
    }
}

/// The relay's log, written under one epoch.
#[derive(Clone)]
pub struct RelayLog {
    pool: RelayPool,
    epoch: u64,
}

fn journal_error(e: PoolError) -> JournalError {
    match e {
        PoolError::Refused { code, message, .. } if code == "fenced" || code == "lease_lost" => {
            JournalError::Fenced(message)
        }
        PoolError::Refused { code, message, .. }
            if code == "conflict" || code == "duplicate_terminal" =>
        {
            JournalError::Conflict(format!("{code}: {message}"))
        }
        PoolError::Refused { code, message, .. } if code == "rekey" => JournalError::Rekey(message),
        PoolError::Refused { code, message, .. }
            if code == "snapshot_needed" || code == "quota" =>
        {
            JournalError::Full(format!("{code}: {message}"))
        }
        PoolError::Refused { code, message, .. } => {
            JournalError::Unavailable(format!("{code}: {message}"))
        }
        other => JournalError::Unavailable(other.to_string()),
    }
}

#[async_trait]
impl Journal for RelayLog {
    fn replicated(&self) -> bool {
        true
    }

    async fn append(&self, from: u64, entries: Vec<LogEntry>) -> Result<u64, JournalError> {
        let v = self
            .pool
            .call(
                reqwest::Method::POST,
                "/v1/pool/log",
                Some(json!({"epoch": self.epoch, "from": from, "entries": entries})),
                Duration::from_secs(30),
            )
            .await
            .map_err(journal_error)?;
        v["head"]
            .as_u64()
            .ok_or_else(|| JournalError::Unavailable("the relay answered without a head".into()))
    }

    async fn snapshot(&self, seq: u64, data: Vec<u8>) -> Result<(), JournalError> {
        use base64::Engine as _;
        self.pool
            .call(
                reqwest::Method::PUT,
                "/v1/pool/snapshot",
                Some(json!({"epoch": self.epoch, "seq": seq,
                            "data": base64::engine::general_purpose::STANDARD.encode(data)})),
                Duration::from_secs(300),
            )
            .await
            .map(|_| ())
            .map_err(journal_error)
    }
}

#[async_trait]
impl LogReader for RelayLog {
    async fn read(&self, after: u64) -> Result<Option<(Vec<LogEntry>, u64)>, JournalError> {
        match self
            .pool
            .call(
                reqwest::Method::GET,
                &format!("/v1/pool/log?after={after}&limit=5000"),
                None,
                Duration::from_secs(60),
            )
            .await
        {
            Ok(v) => {
                let entries: Vec<LogEntry> = serde_json::from_value(v["entries"].clone())
                    .map_err(|e| JournalError::Unavailable(format!("pool log: {e}")))?;
                Ok(Some((entries, v["head"].as_u64().unwrap_or(0))))
            }
            Err(PoolError::Refused { code, .. }) if code == "snapshot" => Ok(None),
            Err(e) => Err(journal_error(e)),
        }
    }

    async fn snapshot(&self) -> Result<Option<(u64, Vec<u8>)>, JournalError> {
        use base64::Engine as _;
        let v = self
            .pool
            .call(
                reqwest::Method::GET,
                "/v1/pool/snapshot",
                None,
                Duration::from_secs(300),
            )
            .await
            .map_err(journal_error)?;
        let Some(data) = v["data"].as_str() else {
            return Ok(None);
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|e| JournalError::Unavailable(format!("pool snapshot: {e}")))?;
        Ok(Some((v["seq"].as_u64().unwrap_or(0), bytes)))
    }
}

fn key_error(e: PoolError) -> KeyError {
    match e {
        PoolError::Refused { code, .. } if code == "keyring_changed" => KeyError::Conflict,
        PoolError::Refused { code, message, .. } => KeyError::Refused(format!("{code}: {message}")),
        other => KeyError::Unavailable(other.to_string()),
    }
}

/// The account's keyring and members on the relay (`/v1/pool/keys`,
/// `/v1/pool/member`).
#[async_trait]
impl KeyDirectory for RelayPool {
    async fn register(&self, me: &Member) -> Result<(), KeyError> {
        self.call(
            reqwest::Method::POST,
            "/v1/pool/member",
            Some(json!({"pool_key": me.pool_key, "sig": me.sig})),
            Duration::from_secs(10),
        )
        .await
        .map(|_| ())
        .map_err(key_error)
    }

    async fn fetch(&self) -> Result<KeyState, KeyError> {
        let v = self
            .call(
                reqwest::Method::GET,
                "/v1/pool/keys",
                None,
                Duration::from_secs(10),
            )
            .await
            .map_err(key_error)?;
        serde_json::from_value(v).map_err(|e| KeyError::Unavailable(format!("pool keys: {e}")))
    }

    async fn put(&self, version: u64, ring: &Keyring, reset: bool) -> Result<u64, KeyError> {
        let v = self
            .call(
                reqwest::Method::PUT,
                "/v1/pool/keys",
                Some(json!({"version": version, "keyring": ring, "reset": reset})),
                Duration::from_secs(10),
            )
            .await
            .map_err(key_error)?;
        v["version"]
            .as_u64()
            .ok_or_else(|| KeyError::Unavailable("the relay answered without a version".into()))
    }

    async fn last_seen(&self) -> std::collections::HashMap<String, u64> {
        // The account's device list (`cua devices`): who is who, for people.
        let Ok(v) = self
            .call(
                reqwest::Method::GET,
                "/v1/devices",
                None,
                Duration::from_secs(10),
            )
            .await
        else {
            return Default::default();
        };
        v["devices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| Some((d["id"].as_str()?.to_string(), d["last_seen"].as_u64()?)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoAuth;

    #[async_trait]
    impl PoolAuth for NoAuth {
        async fn headers(&self) -> Result<Vec<(String, String)>, String> {
            Err("signed out".into())
        }
    }

    /// F53: signed out, the lease loop makes a `RelayPool` every few
    /// seconds; building an HTTPS client per pool read the root store each
    /// time (a few percent of a core on macOS).
    #[test]
    fn every_pool_shares_one_http_client() {
        let auth: Arc<dyn PoolAuth> = Arc::new(NoAuth);
        for _ in 0..100 {
            let _ = RelayPool::new("https://relay.example", "", auth.clone());
        }
        assert_eq!(CLIENT_BUILDS.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    fn refused(code: &str) -> PoolError {
        PoolError::Refused {
            code: code.into(),
            message: "m".into(),
            body: Value::Null,
        }
    }

    #[test]
    fn a_log_refused_for_space_asks_for_a_snapshot_and_a_full_volume_for_a_retry() {
        // A full log or a pool at its quota: snapshot, then append again.
        assert!(matches!(
            journal_error(refused("snapshot_needed")),
            JournalError::Full(_)
        ));
        assert!(matches!(
            journal_error(refused("quota")),
            JournalError::Full(_)
        ));
        // The volume at its reserve: a snapshot would not help; retry later.
        assert!(matches!(
            journal_error(refused("disk_reserve")),
            JournalError::Unavailable(_)
        ));
        assert!(matches!(
            journal_error(refused("fenced")),
            JournalError::Fenced(_)
        ));
    }
}
