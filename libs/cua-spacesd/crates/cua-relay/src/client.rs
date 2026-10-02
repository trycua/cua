// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Machine side of the reverse tunnel (`cua-spacesd join`).
//!
//! Holds one outbound WebSocket to the relay, multiplexed with yamux. The
//! relay opens a yamux stream per client connection; each is spliced,
//! unchanged, into the local spacesd listener, so gRPC (h2), gRPC-Web,
//! WebSockets and `/files` work end to end. In account mode the relay never
//! sees the client's account token or device session (it strips both and
//! forwards a short-lived relay-signed assertion instead); it does still
//! parse every request in the clear to route and authorize it, the way any
//! TLS-terminating proxy does (see the crate README's "What the relay can
//! see"). Reconnects with exponential backoff (capped, with jitter) and
//! detects dead links with a heartbeat stream.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures_util::io::{AsyncReadExt as _, AsyncWriteExt as _};
use rand::Rng as _;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_util::compat::FuturesAsyncReadCompatExt as _;
use tokio_util::sync::CancellationToken;

use crate::assertion::{TrustedKeys, JWKS_HEADER, OWNER_HEADER};
use crate::{mux, ws::WsIo, CONNECT_PATH, HEARTBEAT_BYTE, MACHINE_ID_HEADER, VERSION_HEADER};

/// What the relay told the machine about its account registration: the
/// keys that sign principal assertions and the owning account. Empty for
/// static-token relays.
#[derive(Debug, Default)]
pub struct AccountLink {
    /// Relay keys trusted for assertions.
    pub keys: TrustedKeys,
    /// Owning account, as the relay reported it.
    pub owner: RwLock<Option<String>>,
    /// Keys came from a pinned file: the handshake must present one of
    /// them, and only the pinned keys are trusted.
    pub pinned: std::sync::atomic::AtomicBool,
    /// Pinned Ed25519 keys (`x`).
    pinned_keys: RwLock<Vec<String>>,
}

impl AccountLink {
    /// Pins the relay keys from a JWKS file.
    pub fn pin_jwks_file(&self, path: &Path) -> Result<usize, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let n = self.keys.set_jwks_json(&raw)?;
        *self.pinned_keys.write().expect("pinned") = crate::assertion::ed25519_keys(&raw)?;
        self.pinned.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(n)
    }

    /// Pins the relay keys from JWKS JSON (the registration reply's
    /// `jwks`).
    pub fn pin_jwks_json(&self, raw: &str) -> Result<usize, String> {
        let n = self.keys.set_jwks_json(raw)?;
        *self.pinned_keys.write().expect("pinned") = crate::assertion::ed25519_keys(raw)?;
        self.pinned.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(n)
    }

    /// The owning account, if known.
    pub fn owner(&self) -> Option<String> {
        self.owner.read().expect("owner").clone()
    }
}

/// Join configuration.
#[derive(Debug, Clone)]
pub struct JoinConfig {
    /// Relay base URL (`wss://relay.example`, `https://…`, `ws://…`).
    pub relay_url: String,
    /// Machine-registration token for the relay (not the env token).
    pub relay_token: String,
    /// Persistent machine id.
    pub machine_id: String,
    /// spacesd version sent to the relay.
    pub version: String,
    /// Local spacesd listener every stream is spliced into.
    pub local: SocketAddr,
    /// Heartbeat interval.
    pub heartbeat: Duration,
    /// Backoff cap.
    pub max_backoff: Duration,
    /// Re-read the relay token from this file before every attempt (so
    /// `cua host setup` can rotate the machine token under a running
    /// service). Overrides `relay_token` when readable and non-empty.
    pub relay_token_file: Option<PathBuf>,
    /// Account registration details learned on the handshake.
    pub account: Arc<AccountLink>,
}

impl JoinConfig {
    /// Defaults for everything but the relay, token, id and local address.
    pub fn new(
        relay_url: String,
        relay_token: String,
        machine_id: String,
        local: SocketAddr,
    ) -> Self {
        Self {
            relay_url,
            relay_token,
            machine_id,
            version: env!("CARGO_PKG_VERSION").into(),
            local,
            heartbeat: Duration::from_secs(15),
            max_backoff: Duration::from_secs(30),
            relay_token_file: None,
            account: Arc::default(),
        }
    }
}

/// Loads the machine id at `path`, creating a random one on first use.
pub fn load_or_create_machine_id(path: &Path) -> std::io::Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let id = existing.trim().to_owned();
        if crate::valid_machine_id(&id) {
            return Ok(id);
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::assertion::write_private(path, format!("{id}\n").as_bytes())?;
    Ok(id)
}

/// The WebSocket URL of the relay's connect endpoint.
pub fn connect_url(relay_url: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(relay_url).map_err(|e| format!("relay url {relay_url:?}: {e}"))?;
    let scheme = match url.scheme() {
        "wss" | "https" => "wss",
        "ws" | "http" => "ws",
        other => return Err(format!("unsupported relay scheme {other:?}")),
    };
    url.set_scheme(scheme)
        .map_err(|_| "cannot set scheme".to_owned())?;
    let base = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{base}{CONNECT_PATH}"));
    Ok(url.to_string())
}

/// Backoff with full jitter in [base/2, base], doubling up to `cap`.
pub fn backoff(attempt: u32, cap: Duration) -> Duration {
    let base = Duration::from_millis(500)
        .saturating_mul(1u32 << attempt.min(16))
        .min(cap);
    let jitter = rand::thread_rng().gen_range(0.5..=1.0);
    base.mul_f64(jitter)
}

/// Why a session ended.
#[derive(Debug)]
pub enum SessionEnd {
    /// Shutdown requested.
    Shutdown,
    /// The relay refused the handshake (bad token, conflict, limit).
    Refused(String),
    /// Transport failure.
    Lost(String),
}

/// Runs until `shutdown`: connect, serve, reconnect with backoff.
pub async fn run(config: JoinConfig, shutdown: CancellationToken) {
    let mut attempt = 0u32;
    loop {
        let started = Instant::now();
        let end = session(&config, &shutdown).await;
        match &end {
            SessionEnd::Shutdown => return,
            SessionEnd::Refused(reason) => tracing::warn!(%reason, "relay refused the machine"),
            SessionEnd::Lost(reason) => tracing::info!(%reason, "relay connection lost"),
        }
        // A session that stayed up for a while resets the backoff.
        if started.elapsed() > Duration::from_secs(60) {
            attempt = 0;
        }
        let delay = backoff(attempt, config.max_backoff);
        attempt = attempt.saturating_add(1);
        tracing::info!(?delay, "reconnecting to the relay");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

/// One relay session.
pub async fn session(config: &JoinConfig, shutdown: &CancellationToken) -> SessionEnd {
    let url = match connect_url(&config.relay_url) {
        Ok(url) => url,
        Err(e) => return SessionEnd::Refused(e),
    };
    let mut request = match url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return SessionEnd::Refused(e.to_string()),
    };
    let headers = request.headers_mut();
    let header = |v: &str| http::HeaderValue::from_str(v).map_err(|e| e.to_string());
    let token = config
        .relay_token_file
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| config.relay_token.clone());
    match (
        header(&format!("Bearer {token}")),
        header(&config.machine_id),
        header(&config.version),
    ) {
        (Ok(auth), Ok(id), Ok(version)) => {
            headers.insert(http::header::AUTHORIZATION, auth);
            headers.insert(MACHINE_ID_HEADER, id);
            headers.insert(VERSION_HEADER, version);
        }
        _ => return SessionEnd::Refused("invalid header value".into()),
    }
    let connect = tokio_tungstenite::connect_async(request);
    let socket = tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(20), connect) => match r {
            Ok(Ok((socket, response))) => {
                if let Err(reason) = learn_account(&config.account, response.headers()) {
                    tracing::error!(%reason, "refusing the relay");
                    return SessionEnd::Refused(reason);
                }
                socket
            }
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
                return SessionEnd::Refused(format!("HTTP {}", response.status()));
            }
            Ok(Err(e)) => return SessionEnd::Lost(e.to_string()),
            Err(_) => return SessionEnd::Lost("connect timed out".into()),
        },
        _ = shutdown.cancelled() => return SessionEnd::Shutdown,
    };
    tracing::info!(relay = %config.relay_url, machine = %config.machine_id, "joined relay");
    let (inbound_tx, mut inbound_rx) = mpsc::channel(256);
    let (handle, mut driver) = mux::spawn(
        WsIo::new(socket),
        yamux::Mode::Server,
        inbound_tx,
        mux::MAX_STREAMS,
        mux::DEFAULT_WINDOW_BYTES,
    );
    let local = config.local;
    let accept = async move {
        while let Some(stream) = inbound_rx.recv().await {
            tokio::spawn(splice(stream, local));
        }
    };
    let heartbeat = heartbeat(handle.clone(), config.heartbeat);
    tokio::select! {
        _ = shutdown.cancelled() => SessionEnd::Shutdown,
        result = &mut driver => SessionEnd::Lost(match result {
            Ok(Ok(())) => "relay closed the connection".into(),
            Ok(Err(e)) => e.to_string(),
            Err(e) => e.to_string(),
        }),
        _ = accept => SessionEnd::Lost("stream acceptor ended".into()),
        reason = heartbeat => SessionEnd::Lost(reason),
    }
}

/// Records the relay keys and owner from the 101 response. With pinned
/// keys, the relay must present one of them (else it is not the relay this
/// machine was set up with, and the session is refused).
pub fn learn_account(link: &AccountLink, headers: &http::HeaderMap) -> Result<(), String> {
    let owner = headers
        .get(OWNER_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let jwks = headers.get(JWKS_HEADER).and_then(|v| v.to_str().ok());
    if link.pinned.load(std::sync::atomic::Ordering::SeqCst) {
        let pinned = link.pinned_keys.read().expect("pinned").clone();
        let presented = match jwks {
            Some(value) => {
                crate::assertion::ed25519_keys(&crate::assertion::decode_jwks_header(value)?)?
            }
            None => {
                return Err("the relay presented no assertion key; this machine pins one".into())
            }
        };
        if !presented.iter().any(|k| pinned.contains(k)) {
            return Err(
                "the relay's assertion key does not match the pinned key (relay-jwks.json)".into(),
            );
        }
        *link.owner.write().expect("owner") = owner;
        return Ok(());
    }
    *link.owner.write().expect("owner") = owner.clone();
    if let Some(jwks) = jwks {
        match link.keys.set_from_header(jwks) {
            Ok(n) => {
                tracing::info!(keys = n, owner = ?owner, "account mode: trusting the relay's assertion keys")
            }
            Err(error) => tracing::warn!(%error, "ignoring the relay's key header"),
        }
    }
    Ok(())
}

async fn splice(stream: yamux::Stream, local: SocketAddr) {
    let mut remote = stream.compat();
    match tokio::net::TcpStream::connect(local).await {
        Ok(mut tcp) => {
            let _ = tcp.set_nodelay(true);
            let _ = tokio::io::copy_bidirectional(&mut remote, &mut tcp).await;
        }
        Err(error) => tracing::warn!(%error, %local, "cannot reach the local spacesd"),
    }
}

/// Opens a stream every `interval`, writes one byte and expects it back.
/// Returns the failure reason.
async fn heartbeat(handle: mux::MuxHandle, interval: Duration) -> String {
    let mut misses = 0u32;
    loop {
        tokio::time::sleep(interval).await;
        let probe = async {
            let mut stream = handle.open().await.map_err(|e| e.to_string())?;
            stream
                .write_all(&[HEARTBEAT_BYTE])
                .await
                .map_err(|e| e.to_string())?;
            stream.flush().await.map_err(|e| e.to_string())?;
            let mut buf = [0u8; 1];
            stream
                .read_exact(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            let _ = stream.close().await;
            if buf[0] == HEARTBEAT_BYTE {
                Ok(())
            } else {
                Err("bad heartbeat reply".to_owned())
            }
        };
        // A busy tunnel (a multi-GiB transfer shares the one WebSocket) can
        // delay the echo; only a dead link fails twice in a row.
        match tokio::time::timeout(heartbeat_deadline(interval), probe).await {
            Ok(Ok(())) => misses = 0,
            Ok(Err(e)) => return format!("heartbeat failed: {e}"),
            Err(_) => {
                misses += 1;
                if misses >= 2 {
                    return "heartbeat timed out twice".into();
                }
                tracing::debug!("heartbeat slow; retrying");
            }
        }
    }
}

/// How long one heartbeat echo may take.
pub fn heartbeat_deadline(interval: Duration) -> Duration {
    (interval * 3).max(Duration::from_secs(20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_urls() {
        assert_eq!(
            connect_url("https://relay.example").unwrap(),
            "wss://relay.example/relay/v1/connect"
        );
        assert_eq!(
            connect_url("ws://r:8080/base/").unwrap(),
            "ws://r:8080/base/relay/v1/connect"
        );
        assert!(connect_url("ftp://x").is_err());
    }

    #[test]
    fn backoff_is_capped_and_jittered() {
        let cap = Duration::from_secs(30);
        for attempt in 0..40 {
            let d = backoff(attempt, cap);
            assert!(d <= cap);
            assert!(d >= Duration::from_millis(250));
        }
        assert!(backoff(20, cap) >= Duration::from_secs(15));
    }

    #[test]
    fn pinned_key_must_match_the_handshake() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let pinned = crate::assertion::RelayKey::generate();
        let path = dir.path().join("relay-jwks.json");
        std::fs::write(&path, pinned.jwks().to_string()).unwrap();
        let link = AccountLink::default();
        link.pin_jwks_file(&path).unwrap();
        let header = |k: &crate::assertion::RelayKey| {
            let mut h = http::HeaderMap::new();
            h.insert(
                JWKS_HEADER,
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(k.jwks().to_string())
                    .parse()
                    .unwrap(),
            );
            h.insert(OWNER_HEADER, "ada".parse().unwrap());
            h
        };
        assert!(learn_account(&link, &header(&pinned)).is_ok());
        assert_eq!(link.owner().as_deref(), Some("ada"));
        let other = crate::assertion::RelayKey::generate();
        let err = learn_account(&link, &header(&other)).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
        assert!(learn_account(&link, &http::HeaderMap::new()).is_err());
        // Unpinned: the handshake key is learned.
        let open = AccountLink::default();
        learn_account(&open, &header(&other)).unwrap();
        assert_eq!(open.keys.len(), 1);
    }

    #[test]
    fn machine_id_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spacesd/id");
        let first = load_or_create_machine_id(&path).unwrap();
        assert!(crate::valid_machine_id(&first));
        assert_eq!(load_or_create_machine_id(&path).unwrap(), first);
    }
}
