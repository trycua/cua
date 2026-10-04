//! Where batches go: PostHog over HTTPS, or memory (tests).

use serde_json::Value;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// PostHog EU ingest (the project cua-driver, Lume and the Python and
/// TypeScript SDKs already use). Batches go to `/batch/`.
pub const DEFAULT_ENDPOINT: &str = "https://eu.i.posthog.com/batch/";
/// The project's public, write-only ingest key. It is already public in
/// every Cua client; it cannot read data.
pub const POSTHOG_API_KEY: &str = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14";
/// Per-request budget. Telemetry never waits longer.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a batch was not delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// A non-loopback send while the network is forbidden (tests).
    Forbidden,
    /// The request failed (kept in the spool for a later retry).
    Failed(String),
}

/// A batch destination.
pub trait Sink: Send + Sync {
    /// Sends one `{"api_key", "batch": [...]}` body.
    fn send(&self, body: &Value) -> Result<(), SendError>;
    /// The endpoint, for `status`.
    fn endpoint(&self) -> String;
}

static FORBIDDEN_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

/// How many sends the no-network guard refused in this process.
pub fn forbidden_attempts() -> usize {
    FORBIDDEN_ATTEMPTS.load(Ordering::SeqCst)
}

/// Whether `url`'s host is loopback (`localhost`, `127.x`, `[::1]`).
pub fn is_loopback_url(url: &str) -> bool {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// PostHog over HTTPS (`ureq`, short timeout, no retries in-line).
pub struct HttpSink {
    endpoint: String,
    forbid_network: bool,
}

impl HttpSink {
    /// A sink for `endpoint`. With `forbid_network`, anything but a
    /// loopback endpoint is refused without opening a socket.
    pub fn new(endpoint: impl Into<String>, forbid_network: bool) -> Self {
        Self {
            endpoint: endpoint.into(),
            forbid_network,
        }
    }
}

impl Sink for HttpSink {
    fn send(&self, body: &Value) -> Result<(), SendError> {
        if self.forbid_network && !is_loopback_url(&self.endpoint) {
            FORBIDDEN_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
            return Err(SendError::Forbidden);
        }
        post(&self.endpoint, body)
    }

    fn endpoint(&self) -> String {
        self.endpoint.clone()
    }
}

#[cfg(feature = "http")]
fn post(endpoint: &str, body: &Value) -> Result<(), SendError> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(SEND_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();
    match agent
        .post(endpoint)
        .header("Content-Type", "application/json")
        .send(serde_json::to_vec(body).unwrap_or_default())
    {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => Err(SendError::Failed(format!("HTTP {}", r.status().as_u16()))),
        Err(e) => Err(SendError::Failed(e.to_string())),
    }
}

#[cfg(not(feature = "http"))]
fn post(_endpoint: &str, _body: &Value) -> Result<(), SendError> {
    Err(SendError::Failed("built without the http feature".into()))
}

/// Keeps every batch in memory (tests).
#[derive(Default)]
pub struct MemorySink {
    batches: Mutex<Vec<Value>>,
    fail: std::sync::atomic::AtomicBool,
}

impl MemorySink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every batch body received.
    pub fn batches(&self) -> Vec<Value> {
        self.batches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Every event received (flattened).
    pub fn events(&self) -> Vec<Value> {
        self.batches()
            .into_iter()
            .flat_map(|b| b["batch"].as_array().cloned().unwrap_or_default())
            .collect()
    }

    /// Make sends fail (offline).
    pub fn set_failing(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }
}

impl Sink for MemorySink {
    fn send(&self, body: &Value) -> Result<(), SendError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(SendError::Failed("offline".into()));
        }
        self.batches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(body.clone());
        Ok(())
    }

    fn endpoint(&self) -> String {
        "memory".into()
    }
}
