//! Pooled CDP browser-endpoint WebSocket client.
//!
//! v2 demultiplexer: a dedicated reader task owns the receive half of
//! each connection and routes frames by `id` to per-call oneshot
//! channels, while unsolicited events fan out to [`CdpConnection::subscribe`]
//! subscribers in arrival order. This replaces v1's hold-the-socket
//! serialization and is what makes `Target.*` auto-attach (OOPIF child
//! sessions) usable: events emitted before a command's reply are
//! guaranteed to be queued on a subscriber created before the call by
//! the time that call returns.
//!
//! Only loopback `ws://` URLs are accepted: the endpoint is a local
//! browser the platform adapter proved we own, never a remote service.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Validate that `url` is a plain-`ws` loopback WebSocket URL.
/// Anything else — `wss`, remote hosts, hostnames that merely *resolve*
/// to loopback — is rejected; the endpoint contract is a literal
/// loopback address handed over by the platform adapter.
pub fn validate_loopback_ws_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("endpoint URL must use the ws:// scheme: {url}"))?;
    let authority = rest.split('/').next().unwrap_or("");
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        // [::1]:9222 form.
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    match host {
        "127.0.0.1" | "::1" | "localhost" => Ok(()),
        other => Err(format!("endpoint host {other:?} is not loopback: {url}")),
    }
}

/// An unsolicited CDP event as delivered by the browser endpoint.
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    /// The (parent) session the event is scoped to under flattened
    /// sessions, when present.
    pub session_id: Option<String>,
    pub params: Value,
}

/// Bounded state for one page-owned JavaScript dialog. Message text and URLs
/// are deliberately not retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpDialogState {
    pub generation: u64,
    pub kind: String,
    pub session_id: String,
}

/// What the reader routed back for one in-flight call.
enum CallOutcome {
    Result(Value),
    Error { code: Option<i64>, message: String },
}

/// State shared between the caller side and the reader task.
struct Demux {
    pending: StdMutex<HashMap<u64, oneshot::Sender<CallOutcome>>>,
    subscribers: StdMutex<Vec<mpsc::UnboundedSender<CdpEvent>>>,
    session_targets: StdMutex<HashMap<String, String>>,
    dialogs: StdMutex<HashMap<String, CdpDialogState>>,
    next_dialog_generation: AtomicU64,
    closed: AtomicBool,
}

impl Demux {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // Dropping the senders wakes every pending caller with a recv
        // error, which `call` reports as a closed socket.
        self.pending.lock().unwrap().clear();
        self.subscribers.lock().unwrap().clear();
        self.session_targets.lock().unwrap().clear();
        self.dialogs.lock().unwrap().clear();
    }
}

/// The reader task: routes replies by id, fans events out to
/// subscribers, and fails everything on socket close.
async fn read_loop(mut read: SplitStream<WsStream>, demux: Arc<Demux>) {
    loop {
        let frame = match read.next().await {
            Some(Ok(frame)) => frame,
            Some(Err(_)) | None => break,
        };
        let text = match frame {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let v: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            let Some(tx) = demux.pending.lock().unwrap().remove(&id) else {
                continue; // reply to a timed-out or unknown call
            };
            let outcome = match v.get("error") {
                Some(err) => CallOutcome::Error {
                    code: err.get("code").and_then(Value::as_i64),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown CDP error")
                        .to_owned(),
                },
                None => CallOutcome::Result(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(outcome);
        } else if let Some(method) = v.get("method").and_then(Value::as_str) {
            let event = CdpEvent {
                method: method.to_owned(),
                session_id: v
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                params: v.get("params").cloned().unwrap_or(Value::Null),
            };
            if let Some(session_id) = event.session_id.as_deref() {
                let target_id = demux
                    .session_targets
                    .lock()
                    .unwrap()
                    .get(session_id)
                    .cloned();
                match event.method.as_str() {
                    "Page.javascriptDialogOpening" => {
                        let kind = match event.params.get("type").and_then(Value::as_str) {
                            Some("alert") => "alert",
                            Some("confirm") => "confirm",
                            Some("prompt") => "prompt",
                            Some("beforeunload") => "beforeunload",
                            _ => "other",
                        };
                        let generation =
                            demux.next_dialog_generation.fetch_add(1, Ordering::Relaxed);
                        if let Some(target_id) = target_id {
                            demux.dialogs.lock().unwrap().insert(
                                target_id,
                                CdpDialogState {
                                    generation,
                                    kind: kind.to_owned(),
                                    session_id: session_id.to_owned(),
                                },
                            );
                        }
                    }
                    "Page.javascriptDialogClosed" => {
                        if let Some(target_id) = target_id {
                            demux.dialogs.lock().unwrap().remove(&target_id);
                        }
                    }
                    _ => {}
                }
            }
            demux
                .subscribers
                .lock()
                .unwrap()
                .retain(|s| s.send(event.clone()).is_ok());
        }
    }
    demux.close();
}

/// A single browser-endpoint connection.
pub struct CdpConnection {
    writer: Mutex<SplitSink<WsStream, Message>>,
    demux: Arc<Demux>,
    next_id: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for CdpConnection {
    fn drop(&mut self) {
        self.reader.abort();
        self.demux.close();
    }
}

impl CdpConnection {
    pub async fn connect(ws_url: &str) -> anyhow::Result<Self> {
        validate_loopback_ws_url(ws_url).map_err(|e| anyhow::anyhow!(e))?;
        let (ws, _resp) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(ws_url))
                .await
                .map_err(|_| anyhow::anyhow!("CDP connect to {ws_url} timed out"))??;
        let (write, read) = ws.split();
        let demux = Arc::new(Demux {
            pending: StdMutex::new(HashMap::new()),
            subscribers: StdMutex::new(Vec::new()),
            session_targets: StdMutex::new(HashMap::new()),
            dialogs: StdMutex::new(HashMap::new()),
            next_dialog_generation: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });
        let reader = tokio::spawn(read_loop(read, demux.clone()));
        Ok(Self {
            writer: Mutex::new(write),
            demux,
            next_id: AtomicU64::new(1),
            reader,
        })
    }

    /// Whether the reader observed the socket close. A closed connection
    /// never recovers; the pool redials.
    pub fn is_closed(&self) -> bool {
        self.demux.closed.load(Ordering::SeqCst)
    }

    /// Subscribe to unsolicited CDP events. Delivery is in socket
    /// arrival order, and any event the endpoint emitted before a
    /// command's reply is already queued here by the time [`Self::call`]
    /// for that command returns — callers can drain with `try_recv`
    /// deterministically (the Target.setAutoAttach pattern).
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<CdpEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.demux.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Associate the one Page-enabled dialog session with its exact target.
    /// Later calls may use fresh attachment sessions, but dialog events keep
    /// arriving on this bounded, persistent event session.
    pub fn register_dialog_session(&self, session_id: &str, target_id: &str) {
        let mut sessions = self.demux.session_targets.lock().unwrap();
        sessions.retain(|_, mapped_target| mapped_target != target_id);
        sessions.insert(session_id.to_owned(), target_id.to_owned());
    }

    pub fn unregister_dialog_session(&self, session_id: &str, target_id: &str) {
        let mut sessions = self.demux.session_targets.lock().unwrap();
        if sessions.get(session_id).map(String::as_str) == Some(target_id) {
            sessions.remove(session_id);
        }
    }

    pub fn has_dialog_session(&self, target_id: &str) -> bool {
        self.demux
            .session_targets
            .lock()
            .unwrap()
            .values()
            .any(|mapped_target| mapped_target == target_id)
    }

    pub fn dialog_state(&self, target_id: &str) -> Option<CdpDialogState> {
        self.demux.dialogs.lock().unwrap().get(target_id).cloned()
    }

    pub fn clear_dialog_state(&self, target_id: &str, generation: u64) {
        let mut dialogs = self.demux.dialogs.lock().unwrap();
        if dialogs.get(target_id).map(|state| state.generation) == Some(generation) {
            dialogs.remove(target_id);
        }
    }

    /// Issue one CDP command and await its `id`-matched reply.
    /// `session_id` targets an attached (flattened) target session.
    /// Returns the `result` object, or an error carrying the CDP
    /// `error.code` (as `({code})`) and `error.message`.
    pub async fn call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        if self.is_closed() {
            anyhow::bail!("CDP socket closed before {method}");
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.demux.pending.lock().unwrap().insert(id, tx);
        // Close can race the initial check. If the reader cleared the
        // pending map just before this insertion, remove the orphaned
        // sender now instead of waiting for the call timeout.
        if self.is_closed() {
            self.demux.pending.lock().unwrap().remove(&id);
            anyhow::bail!("CDP socket closed before {method}");
        }

        let mut msg = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            msg["sessionId"] = Value::String(sid.to_owned());
        }
        let sent = {
            let mut writer = self.writer.lock().await;
            writer.send(Message::Text(msg.to_string())).await
        };
        if let Err(e) = sent {
            self.demux.pending.lock().unwrap().remove(&id);
            anyhow::bail!("CDP send failed during {method}: {e}");
        }

        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Err(_) => {
                self.demux.pending.lock().unwrap().remove(&id);
                anyhow::bail!("CDP {method} timed out after {CALL_TIMEOUT:?}")
            }
            Ok(Err(_)) => anyhow::bail!("CDP socket closed during {method}"),
            Ok(Ok(CallOutcome::Result(v))) => Ok(v),
            Ok(Ok(CallOutcome::Error {
                code: Some(code),
                message,
            })) => anyhow::bail!("CDP {method} failed ({code}): {message}"),
            Ok(Ok(CallOutcome::Error {
                code: None,
                message,
            })) => anyhow::bail!("CDP {method} failed: {message}"),
        }
    }
}

#[derive(Clone)]
struct PoolEntry {
    conn: Arc<CdpConnection>,
    generation: Option<u64>,
}

fn claimed_ports() -> &'static StdMutex<HashMap<u16, usize>> {
    static CLAIMED: OnceLock<StdMutex<HashMap<u16, usize>>> = OnceLock::new();
    CLAIMED.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn loopback_port(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("ws://")?;
    let authority = rest.split('/').next()?;
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split_once("]:")?.1.parse().ok();
    }
    authority.rsplit_once(':')?.1.parse().ok()
}

/// Whether an existing-profile grant owns the DevTools listener used by this
/// URL. The legacy page route consults this before opening its own page socket.
pub fn endpoint_port_is_grant_owned(url: &str) -> bool {
    loopback_port(url).is_some_and(|port| claimed_ports().lock().unwrap().contains_key(&port))
}

/// One pooled browser-level connection per endpoint URL. Existing-profile
/// entries are additionally owned by one explicit connection generation.
pub struct CdpPool {
    conns: Mutex<HashMap<String, PoolEntry>>,
    claimed_loopback_ports: StdMutex<HashSet<u16>>,
}

impl CdpPool {
    pub fn new() -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
            claimed_loopback_ports: StdMutex::new(HashSet::new()),
        }
    }

    /// Get the pooled connection for `ws_url`, dialing if needed.
    /// A connection whose socket closed is replaced transparently.
    pub async fn get(&self, ws_url: &str) -> anyhow::Result<Arc<CdpConnection>> {
        if endpoint_port_is_grant_owned(ws_url) {
            anyhow::bail!(
                "this DevTools endpoint is owned by a first-class existing-profile attachment"
            );
        }
        let mut conns = self.conns.lock().await;
        if let Some(existing) = conns.get(ws_url) {
            if existing.generation.is_some() {
                anyhow::bail!("this DevTools endpoint is owned by an attachment generation");
            }
            if !existing.conn.is_closed() {
                return Ok(existing.conn.clone());
            }
            conns.remove(ws_url);
        }
        let conn = Arc::new(CdpConnection::connect(ws_url).await?);
        conns.insert(
            ws_url.to_owned(),
            PoolEntry {
                conn: conn.clone(),
                generation: None,
            },
        );
        Ok(conn)
    }

    /// Convert the one live browser-level socket into grant-owned state. This
    /// does not redial and therefore cannot create a second consent prompt.
    pub async fn claim_existing(
        &self,
        ws_url: &str,
        generation: u64,
    ) -> anyhow::Result<Arc<CdpConnection>> {
        let port = loopback_port(ws_url)
            .ok_or_else(|| anyhow::anyhow!("existing-profile endpoint has no loopback port"))?;
        let mut conns = self.conns.lock().await;
        let conn = match conns.get(ws_url) {
            Some(entry) if !entry.conn.is_closed() => entry.conn.clone(),
            Some(_) => anyhow::bail!("the approved browser socket closed before it was claimed"),
            None => Arc::new(CdpConnection::connect(ws_url).await?),
        };
        conns.insert(
            ws_url.to_owned(),
            PoolEntry {
                conn: conn.clone(),
                generation: Some(generation),
            },
        );
        if self.claimed_loopback_ports.lock().unwrap().insert(port) {
            *claimed_ports().lock().unwrap().entry(port).or_default() += 1;
        }
        Ok(conn)
    }

    /// Reuse only the socket belonging to the exact live grant generation.
    /// Closed sockets are not replaced here; reconnect is an explicit state
    /// transition owned by the browser engine.
    pub async fn get_existing(
        &self,
        ws_url: &str,
        generation: u64,
    ) -> anyhow::Result<Arc<CdpConnection>> {
        let conns = self.conns.lock().await;
        let entry = conns
            .get(ws_url)
            .ok_or_else(|| anyhow::anyhow!("the grant-owned browser socket is missing"))?;
        if entry.generation != Some(generation) {
            anyhow::bail!("the browser socket belongs to a different connection generation");
        }
        if entry.conn.is_closed() {
            anyhow::bail!("the grant-owned browser socket is closed");
        }
        Ok(entry.conn.clone())
    }

    /// Replace one dead grant-owned socket with exactly one new generation.
    pub async fn reconnect_existing(
        &self,
        ws_url: &str,
        old_generation: u64,
        new_generation: u64,
    ) -> anyhow::Result<Arc<CdpConnection>> {
        {
            let conns = self.conns.lock().await;
            if let Some(entry) = conns.get(ws_url) {
                if entry
                    .generation
                    .is_some_and(|generation| generation > old_generation)
                {
                    anyhow::bail!("the reconnect source generation is no longer current");
                }
                if entry.generation == Some(old_generation) && !entry.conn.is_closed() {
                    return Ok(entry.conn.clone());
                }
            }
        }

        // A WebSocket handshake can wait for browser-owned consent UI. Never
        // hold the pool mutex across that wait: grant revocation must remain
        // able to remove the old generation when consent is refused.
        let conn = Arc::new(CdpConnection::connect(ws_url).await?);
        let mut conns = self.conns.lock().await;
        if let Some(entry) = conns.get(ws_url) {
            if entry
                .generation
                .is_some_and(|generation| generation > old_generation)
            {
                if entry.generation == Some(new_generation) && !entry.conn.is_closed() {
                    return Ok(entry.conn.clone());
                }
                anyhow::bail!("the reconnect source generation is no longer current");
            }
        }
        conns.insert(
            ws_url.to_owned(),
            PoolEntry {
                conn: conn.clone(),
                generation: Some(new_generation),
            },
        );
        Ok(conn)
    }

    /// Drop a (likely dead) connection so the next call redials.
    pub async fn evict(&self, ws_url: &str) {
        self.conns.lock().await.remove(ws_url);
    }

    pub fn release_claim_marker(&self, ws_url: &str) {
        if let Some(port) = loopback_port(ws_url) {
            if self.claimed_loopback_ports.lock().unwrap().remove(&port) {
                release_claimed_port(port);
            }
        }
    }

    pub async fn release_existing(&self, ws_url: &str, generation: u64) {
        let mut conns = self.conns.lock().await;
        if conns
            .get(ws_url)
            .is_some_and(|entry| entry.generation == Some(generation))
        {
            conns.remove(ws_url);
        }
        drop(conns);
        self.release_claim_marker(ws_url);
    }
}

impl Default for CdpPool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CdpPool {
    fn drop(&mut self) {
        let claimed_loopback_ports = self.claimed_loopback_ports.get_mut().unwrap();
        if claimed_loopback_ports.is_empty() {
            return;
        }
        let mut global = claimed_ports().lock().unwrap();
        for port in claimed_loopback_ports.drain() {
            if let Some(count) = global.get_mut(&port) {
                *count -= 1;
                if *count == 0 {
                    global.remove(&port);
                }
            }
        }
    }
}

fn release_claimed_port(port: u16) {
    let mut global = claimed_ports().lock().unwrap();
    if let Some(count) = global.get_mut(&port) {
        *count -= 1;
        if *count == 0 {
            global.remove(&port);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::mock_cdp::{MockCdpServer, MockEvent, MockReply};
    use serde_json::json;
    use std::sync::Arc as StdArc;

    #[test]
    fn loopback_urls_are_accepted() {
        for ok in [
            "ws://127.0.0.1:9222/devtools/browser/abc",
            "ws://localhost:9222/devtools/browser/abc",
            "ws://[::1]:9222/devtools/browser/abc",
            "ws://127.0.0.1/devtools/browser/abc",
        ] {
            assert!(validate_loopback_ws_url(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn non_loopback_and_non_ws_urls_are_rejected() {
        for bad in [
            "wss://127.0.0.1:9222/devtools/browser/abc",
            "http://127.0.0.1:9222/json",
            "ws://10.0.0.5:9222/devtools/browser/abc",
            "ws://example.com:9222/devtools/browser/abc",
            "ws://127.0.0.1.evil.test:9222/x",
            "ws://[fe80::1]:9222/x",
            "",
        ] {
            assert!(validate_loopback_ws_url(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn call_returns_the_id_matched_result() {
        let server = MockCdpServer::start(StdArc::new(|call| {
            assert_eq!(call.method, "Echo.params");
            MockReply::ok(json!({ "echoed": call.params.clone() }))
        }))
        .await;
        let conn = CdpConnection::connect(&server.ws_url()).await.unwrap();
        let result = conn
            .call(None, "Echo.params", json!({ "x": 7 }))
            .await
            .unwrap();
        assert_eq!(result["echoed"]["x"], 7);
    }

    #[tokio::test]
    async fn claimed_existing_profile_socket_is_generation_owned() {
        let server = MockCdpServer::start(StdArc::new(|_| MockReply::ok(json!({})))).await;
        let url = server.ws_url();
        let pool = CdpPool::new();
        let initial = pool.get(&url).await.unwrap();
        let claimed = pool.claim_existing(&url, 1).await.unwrap();
        assert!(Arc::ptr_eq(&initial, &claimed), "claim must not redial");
        assert!(pool.get(&url).await.is_err(), "legacy access must refuse");
        assert!(pool.get_existing(&url, 2).await.is_err());
        let reused = pool.get_existing(&url, 1).await.unwrap();
        assert!(Arc::ptr_eq(&claimed, &reused));
        pool.release_existing(&url, 1).await;
        assert!(
            pool.get(&url).await.is_ok(),
            "session cleanup releases ownership"
        );
    }

    #[tokio::test]
    async fn dropping_a_pool_releases_its_claim_marker() {
        let server = MockCdpServer::start(StdArc::new(|_| MockReply::ok(json!({})))).await;
        let url = server.ws_url();
        {
            let pool = CdpPool::new();
            pool.claim_existing(&url, 1).await.unwrap();
            assert!(endpoint_port_is_grant_owned(&url));
        }
        assert!(!endpoint_port_is_grant_owned(&url));
    }

    #[tokio::test]
    async fn claim_marker_is_reference_counted_across_pools() {
        let server = MockCdpServer::start(StdArc::new(|_| MockReply::ok(json!({})))).await;
        let url = server.ws_url();
        let first = CdpPool::new();
        let second = CdpPool::new();
        first.claim_existing(&url, 1).await.unwrap();
        second.claim_existing(&url, 2).await.unwrap();

        drop(first);
        assert!(endpoint_port_is_grant_owned(&url));
        drop(second);
        assert!(!endpoint_port_is_grant_owned(&url));
    }

    #[tokio::test]
    async fn stalled_reconnect_does_not_block_generation_release() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (first, _) = listener.accept().await.unwrap();
            let _first_ws = tokio_tungstenite::accept_async(first).await.unwrap();
            let (_stalled_reconnect, _) = listener.accept().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });
        let url = format!("ws://127.0.0.1:{port}/devtools/browser/reconnect");
        let pool = CdpPool::new();
        let claimed = pool.claim_existing(&url, 1).await.unwrap();
        claimed.demux.close();

        let mut reconnect = Box::pin(pool.reconnect_existing(&url, 1, 2));
        tokio::select! {
            _ = &mut reconnect => panic!("reconnect unexpectedly completed"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        }
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            pool.release_existing(&url, 1),
        )
        .await
        .expect("grant release must not wait for the reconnect handshake");
        drop(reconnect);
        server.abort();
    }

    #[tokio::test]
    async fn cdp_error_codes_are_preserved_in_the_message() {
        let server = MockCdpServer::start(StdArc::new(|_| {
            MockReply::method_not_found("Target.setAutoAttach")
        }))
        .await;
        let conn = CdpConnection::connect(&server.ws_url()).await.unwrap();
        let err = conn
            .call(Some("sess-1"), "Target.setAutoAttach", json!({}))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("(-32601)"), "{msg}");
        assert!(msg.contains("Target.setAutoAttach"), "{msg}");
    }

    #[tokio::test]
    async fn events_emitted_before_a_reply_are_drainable_after_the_call() {
        let server = MockCdpServer::start(StdArc::new(|call| {
            if call.method == "Emitter.go" {
                MockReply::ok(json!({})).with_events(vec![
                    MockEvent {
                        method: "Thing.happened".into(),
                        session_id: None,
                        params: json!({ "n": 1 }),
                    },
                    MockEvent {
                        method: "Thing.happened".into(),
                        session_id: Some("child-sess".into()),
                        params: json!({ "n": 2 }),
                    },
                ])
            } else {
                MockReply::ok(json!({}))
            }
        }))
        .await;
        let conn = CdpConnection::connect(&server.ws_url()).await.unwrap();
        let mut events = conn.subscribe();
        conn.call(None, "Emitter.go", json!({})).await.unwrap();

        // The demux guarantees pre-reply events are queued: no awaiting.
        let first = events.try_recv().expect("first event queued");
        assert_eq!(first.method, "Thing.happened");
        assert_eq!(first.session_id, None);
        assert_eq!(first.params["n"], 1);
        let second = events.try_recv().expect("second event queued");
        assert_eq!(second.session_id.as_deref(), Some("child-sess"));
        assert_eq!(second.params["n"], 2);
        assert!(events.try_recv().is_err(), "no phantom events");
    }

    #[tokio::test]
    async fn javascript_dialog_journal_is_bounded_and_generation_checked() {
        let server = MockCdpServer::start(StdArc::new(|call| match call.method.as_str() {
            "Dialog.open" => MockReply::ok(json!({})).with_events(vec![MockEvent {
                method: "Page.javascriptDialogOpening".into(),
                session_id: call.session_id.clone(),
                params: json!({
                    "type": "prompt",
                    "message": "private dialog text",
                    "url": "https://private.example"
                }),
            }]),
            "Dialog.close" => MockReply::ok(json!({})).with_events(vec![MockEvent {
                method: "Page.javascriptDialogClosed".into(),
                session_id: call.session_id.clone(),
                params: json!({}),
            }]),
            _ => MockReply::ok(json!({})),
        }))
        .await;
        let conn = CdpConnection::connect(&server.ws_url()).await.unwrap();
        conn.register_dialog_session("tab-session-a", "page-target");
        conn.call(Some("tab-session-a"), "Dialog.open", json!({}))
            .await
            .unwrap();
        let dialog = conn.dialog_state("page-target").expect("dialog journaled");
        assert_eq!(dialog.kind, "prompt");
        assert_eq!(dialog.session_id, "tab-session-a");
        assert!(!format!("{dialog:?}").contains("private"));
        conn.call(Some("tab-session-a"), "Dialog.close", json!({}))
            .await
            .unwrap();
        assert!(conn.dialog_state("page-target").is_none());
    }

    #[tokio::test]
    async fn out_of_order_replies_route_to_the_right_callers() {
        // Raw server: read two requests, reply to the SECOND first.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut ids = Vec::new();
            while ids.len() < 2 {
                if let Some(Ok(Message::Text(text))) = ws.next().await {
                    let v: Value = serde_json::from_str(&text).unwrap();
                    ids.push(v["id"].as_u64().unwrap());
                }
            }
            for id in ids.iter().rev() {
                let reply = json!({ "id": id, "result": { "for_id": id } });
                ws.send(Message::Text(reply.to_string())).await.unwrap();
            }
        });

        let conn = CdpConnection::connect(&format!("ws://127.0.0.1:{port}/devtools/browser/x"))
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            conn.call(None, "First.call", json!({})),
            conn.call(None, "Second.call", json!({})),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_ne!(a["for_id"], b["for_id"]);
        assert_eq!(a["for_id"], 1, "first call gets the id-1 payload");
        assert_eq!(b["for_id"], 2, "second call gets the id-2 payload");
    }

    #[tokio::test]
    async fn socket_close_fails_pending_calls_and_marks_the_connection() {
        // Raw server: accept, read one request, close without replying.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = ws.next().await;
            let _ = ws.close(None).await;
        });

        let conn = CdpConnection::connect(&format!("ws://127.0.0.1:{port}/devtools/browser/x"))
            .await
            .unwrap();
        let err = conn
            .call(None, "Never.replies", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("closed"), "{err}");
        assert!(conn.is_closed());
        let err = conn.call(None, "After.close", json!({})).await.unwrap_err();
        assert!(err.to_string().contains("closed"), "{err}");
    }

    #[tokio::test]
    async fn pool_replaces_closed_connections() {
        let server = MockCdpServer::start(StdArc::new(|_| MockReply::ok(json!({})))).await;
        let url = server.ws_url();
        let pool = CdpPool::new();
        let first = pool.get(&url).await.unwrap();
        first.demux.close(); // simulate a dead socket
        let second = pool.get(&url).await.unwrap();
        assert!(
            !StdArc::ptr_eq(&first, &second),
            "pool must redial a closed connection"
        );
        second.call(None, "Ping.pong", json!({})).await.unwrap();
    }
}
