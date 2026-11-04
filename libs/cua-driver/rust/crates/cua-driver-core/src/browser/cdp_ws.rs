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
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::{Duration, Instant};

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

/// Connection-scoped command policy. Grant-owned personal-profile sockets
/// always use the reviewed set below; callers cannot opt out or relax it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdpMethodPolicy {
    Unrestricted,
    ExistingProfile,
}

const EXISTING_PROFILE_METHODS: &[&str] = &[
    "Accessibility.getFullAXTree",
    "Browser.getWindowBounds",
    "Browser.getWindowForTarget",
    "Browser.setDownloadBehavior",
    "DOM.describeNode",
    "DOM.focus",
    "DOM.getBoxModel",
    "DOM.getDocument",
    "DOM.resolveNode",
    "DOM.scrollIntoViewIfNeeded",
    "DOM.setFileInputFiles",
    "DOMSnapshot.captureSnapshot",
    "Emulation.setFocusEmulationEnabled",
    "Input.dispatchKeyEvent",
    "Input.dispatchMouseEvent",
    "Input.insertText",
    "Page.bringToFront",
    "Page.captureScreenshot",
    "Page.enable",
    "Page.getFrameTree",
    "Page.getLayoutMetrics",
    "Page.handleJavaScriptDialog",
    "Page.navigate",
    "Runtime.callFunctionOn",
    "Runtime.evaluate",
    "Target.activateTarget",
    "Target.attachToTarget",
    "Target.detachFromTarget",
    "Target.getTargets",
    "Target.setAutoAttach",
];

impl CdpMethodPolicy {
    fn allows(self, method: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::ExistingProfile => EXISTING_PROFILE_METHODS.binary_search(&method).is_ok(),
        }
    }
}

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
    existing_profile_policy: AtomicBool,
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
            existing_profile_policy: AtomicBool::new(false),
            reader,
        })
    }

    pub fn method_policy(&self) -> CdpMethodPolicy {
        if self.existing_profile_policy.load(Ordering::SeqCst) {
            CdpMethodPolicy::ExistingProfile
        } else {
            CdpMethodPolicy::Unrestricted
        }
    }

    /// Monotonic restriction: once a socket is claimed for a personal
    /// profile it can never return to the unrestricted command surface.
    pub fn restrict_to_existing_profile(&self) {
        self.existing_profile_policy.store(true, Ordering::SeqCst);
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
        let policy = self.method_policy();
        if !policy.allows(method) {
            anyhow::bail!(
                "existing-profile CDP policy refused method {method}; the reviewed personal-profile command set cannot be bypassed"
            );
        }
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

/// How long a consented existing-profile socket may stay open with no session
/// holding it. Matches the existing-profile grant idle bound.
pub const SHARED_SOCKET_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
/// Absolute bound measured from the browser consent that opened the socket.
/// Matches the existing-profile grant absolute bound.
pub const SHARED_SOCKET_ABSOLUTE_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// How a grant gives up its hold on a shared existing-profile socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedSocketRelease {
    /// Normal session end: when this was the last holder, keep the
    /// already-consented socket open (parked) for the idle bound so the next
    /// approved session does not trigger another browser consent prompt.
    Park,
    /// Security-relevant end (identity change, remote debugging being turned
    /// back off, expiry): close the socket once no other session holds it.
    Close,
}

/// Grant-owned state of one browser-level existing-profile socket. Chrome
/// asks the person to approve every new browser-level WebSocket, but not
/// target attaches over an existing one, so every approved session for the
/// same browser instance multiplexes over this one socket through flattened
/// target sessions.
#[derive(Clone)]
struct SharedSocket {
    /// Live grant generations bound to this socket. Generations are unique
    /// per engine, so one holder can never release another holder's claim.
    holders: HashSet<u64>,
    opened_at: Instant,
    last_used_at: Instant,
}

impl SharedSocket {
    fn new(generation: u64, now: Instant) -> Self {
        Self {
            holders: HashSet::from([generation]),
            opened_at: now,
            last_used_at: now,
        }
    }

    fn expired(&self, now: Instant, ttl: &SharedSocketTtl) -> bool {
        now.duration_since(self.opened_at) > ttl.absolute
            || (self.holders.is_empty() && now.duration_since(self.last_used_at) > ttl.idle)
    }
}

#[derive(Clone)]
struct PoolEntry {
    conn: Arc<CdpConnection>,
    /// `None` for an unrestricted driver-owned endpoint.
    shared: Option<SharedSocket>,
}

#[derive(Clone, Copy)]
struct SharedSocketTtl {
    idle: Duration,
    absolute: Duration,
}

fn claimed_ports() -> &'static StdMutex<HashMap<u16, usize>> {
    static CLAIMED: OnceLock<StdMutex<HashMap<u16, usize>>> = OnceLock::new();
    CLAIMED.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn live_pools() -> &'static StdMutex<Vec<Weak<PoolInner>>> {
    static POOLS: OnceLock<StdMutex<Vec<Weak<PoolInner>>>> = OnceLock::new();
    POOLS.get_or_init(|| StdMutex::new(Vec::new()))
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

/// Close every parked (unheld) existing-profile socket in this process. Used
/// when the person revokes all driver sessions: revocation must not leave an
/// already-consented browser socket waiting for the next session.
pub async fn close_parked_browser_sockets() -> usize {
    let pools: Vec<Arc<PoolInner>> = {
        let mut pools = live_pools().lock().unwrap();
        pools.retain(|pool| pool.strong_count() > 0);
        pools.iter().filter_map(Weak::upgrade).collect()
    };
    let mut closed = 0;
    for pool in pools {
        closed += pool.close_parked().await;
    }
    closed
}

/// Per-pool record of which endpoint URLs are grant-owned, so the global
/// per-port marker is held exactly while at least one grant-owned entry for
/// that port exists in this pool.
#[derive(Default)]
struct Markers {
    urls: HashSet<String>,
    ports: HashMap<u16, usize>,
}

struct PoolInner {
    conns: Mutex<HashMap<String, PoolEntry>>,
    /// Serializes every browser-level dial for grant-owned sockets so two
    /// sessions can never open two sockets (and two consent prompts) at once.
    dial_gate: Mutex<()>,
    markers: StdMutex<Markers>,
    ttl: SharedSocketTtl,
}

impl PoolInner {
    async fn close_parked(&self) -> usize {
        let mut conns = self.conns.lock().await;
        let parked: Vec<String> = conns
            .iter()
            .filter(|(_, entry)| entry.shared.as_ref().is_some_and(|s| s.holders.is_empty()))
            .map(|(url, _)| url.clone())
            .collect();
        for url in &parked {
            self.remove_entry(&mut conns, url);
        }
        parked.len()
    }

    fn mark_shared(&self, url: &str) {
        let Some(port) = loopback_port(url) else {
            return;
        };
        let mut markers = self.markers.lock().unwrap();
        if !markers.urls.insert(url.to_owned()) {
            return;
        }
        let count = markers.ports.entry(port).or_default();
        *count += 1;
        if *count == 1 {
            *claimed_ports().lock().unwrap().entry(port).or_default() += 1;
        }
    }

    fn unmark_shared(&self, url: &str) {
        let Some(port) = loopback_port(url) else {
            return;
        };
        let mut markers = self.markers.lock().unwrap();
        if !markers.urls.remove(url) {
            return;
        }
        if let Some(count) = markers.ports.get_mut(&port) {
            *count -= 1;
            if *count == 0 {
                markers.ports.remove(&port);
                release_claimed_port(port);
            }
        }
    }

    fn insert_shared(&self, conns: &mut HashMap<String, PoolEntry>, url: &str, entry: PoolEntry) {
        conns.insert(url.to_owned(), entry);
        self.mark_shared(url);
    }

    fn remove_entry(&self, conns: &mut HashMap<String, PoolEntry>, url: &str) {
        if conns
            .remove(url)
            .is_some_and(|entry| entry.shared.is_some())
        {
            self.unmark_shared(url);
        }
    }

    /// Return the live shared socket for `url`, dropping it first when it is
    /// closed or past its bounds.
    fn live_shared<'a>(
        &self,
        conns: &'a mut HashMap<String, PoolEntry>,
        url: &str,
        now: Instant,
    ) -> Option<&'a mut PoolEntry> {
        let stale = conns.get(url).is_some_and(|entry| {
            entry
                .shared
                .as_ref()
                .is_some_and(|shared| entry.conn.is_closed() || shared.expired(now, &self.ttl))
        });
        if stale {
            self.remove_entry(conns, url);
        }
        conns.get_mut(url).filter(|entry| entry.shared.is_some())
    }
}

impl Drop for PoolInner {
    fn drop(&mut self) {
        let markers = std::mem::take(self.markers.get_mut().unwrap());
        for port in markers.ports.into_keys() {
            release_claimed_port(port);
        }
    }
}

/// One pooled browser-level connection per endpoint URL. Existing-profile
/// entries are shared by every grant for that browser instance: each grant
/// generation is a refcounted holder, and the socket outlives individual
/// sessions only within the bounded parking window.
pub struct CdpPool {
    inner: Arc<PoolInner>,
}

impl CdpPool {
    pub fn new() -> Self {
        Self::with_shared_socket_ttls(SHARED_SOCKET_IDLE_TTL, SHARED_SOCKET_ABSOLUTE_TTL)
    }

    pub(crate) fn with_shared_socket_ttls(idle: Duration, absolute: Duration) -> Self {
        let inner = Arc::new(PoolInner {
            conns: Mutex::new(HashMap::new()),
            dial_gate: Mutex::new(()),
            markers: StdMutex::new(Markers::default()),
            ttl: SharedSocketTtl { idle, absolute },
        });
        live_pools().lock().unwrap().push(Arc::downgrade(&inner));
        Self { inner }
    }

    /// Get the pooled connection for `ws_url`, dialing if needed.
    /// A connection whose socket closed is replaced transparently.
    pub async fn get(&self, ws_url: &str) -> anyhow::Result<Arc<CdpConnection>> {
        if endpoint_port_is_grant_owned(ws_url) {
            anyhow::bail!(
                "this DevTools endpoint is owned by a first-class existing-profile attachment"
            );
        }
        let mut conns = self.inner.conns.lock().await;
        if let Some(existing) = conns.get(ws_url) {
            if existing.shared.is_some() {
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
                shared: None,
            },
        );
        Ok(conn)
    }

    /// Bind grant `generation` to the browser socket for `ws_url`. A live
    /// shared socket (held by another session, or parked after one ended) is
    /// joined without dialing, so no new browser consent prompt appears. Only
    /// when no usable socket exists is exactly one new socket opened.
    pub async fn claim_existing(
        &self,
        ws_url: &str,
        generation: u64,
    ) -> anyhow::Result<Arc<CdpConnection>> {
        loopback_port(ws_url)
            .ok_or_else(|| anyhow::anyhow!("existing-profile endpoint has no loopback port"))?;
        let _dial = self.inner.dial_gate.lock().await;
        {
            let now = Instant::now();
            let mut conns = self.inner.conns.lock().await;
            if let Some(entry) = self.inner.live_shared(&mut conns, ws_url, now) {
                let shared = entry
                    .shared
                    .as_mut()
                    .expect("live_shared returns shared entries");
                shared.holders.insert(generation);
                shared.last_used_at = now;
                return Ok(entry.conn.clone());
            }
            match conns.get(ws_url) {
                Some(entry) if !entry.conn.is_closed() => {
                    // Convert the one live socket that proved the endpoint
                    // into grant-owned state in place, without redialing.
                    let conn = entry.conn.clone();
                    conn.restrict_to_existing_profile();
                    self.inner.insert_shared(
                        &mut conns,
                        ws_url,
                        PoolEntry {
                            conn: conn.clone(),
                            shared: Some(SharedSocket::new(generation, now)),
                        },
                    );
                    return Ok(conn);
                }
                Some(_) => {
                    anyhow::bail!("the approved browser socket closed before it was claimed")
                }
                None => {}
            }
        }

        // The WebSocket handshake can wait for browser-owned consent UI.
        // Never hold the entry mutex across that wait: other sessions keep
        // using their sockets and revocation stays responsive.
        let conn = Arc::new(CdpConnection::connect(ws_url).await?);
        conn.restrict_to_existing_profile();
        let mut conns = self.inner.conns.lock().await;
        self.inner.insert_shared(
            &mut conns,
            ws_url,
            PoolEntry {
                conn: conn.clone(),
                shared: Some(SharedSocket::new(generation, Instant::now())),
            },
        );
        Ok(conn)
    }

    /// Reuse the shared socket only while `generation` still holds it.
    /// Closed sockets are not replaced here; reconnect is an explicit state
    /// transition owned by the browser engine.
    pub async fn get_existing(
        &self,
        ws_url: &str,
        generation: u64,
    ) -> anyhow::Result<Arc<CdpConnection>> {
        let now = Instant::now();
        let mut conns = self.inner.conns.lock().await;
        let entry = conns
            .get_mut(ws_url)
            .ok_or_else(|| anyhow::anyhow!("the grant-owned browser socket is missing"))?;
        let shared = entry
            .shared
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the browser socket is not grant-owned"))?;
        if !shared.holders.contains(&generation) {
            anyhow::bail!("the browser socket belongs to a different connection generation");
        }
        if entry.conn.is_closed() {
            anyhow::bail!("the grant-owned browser socket is closed");
        }
        if now.duration_since(shared.opened_at) > self.inner.ttl.absolute {
            anyhow::bail!("the grant-owned browser socket reached its absolute lifetime");
        }
        shared.last_used_at = now;
        Ok(entry.conn.clone())
    }

    /// Move one grant from `old_generation` to `new_generation`. If another
    /// session already re-established a live shared socket, join it without
    /// dialing; otherwise open exactly one replacement socket (which the
    /// browser treats as a new connection and may ask the person to approve).
    pub async fn reconnect_existing(
        &self,
        ws_url: &str,
        old_generation: u64,
        new_generation: u64,
    ) -> anyhow::Result<Arc<CdpConnection>> {
        let _dial = self.inner.dial_gate.lock().await;
        {
            let now = Instant::now();
            let mut conns = self.inner.conns.lock().await;
            if let Some(entry) = self.inner.live_shared(&mut conns, ws_url, now) {
                let shared = entry
                    .shared
                    .as_mut()
                    .expect("live_shared returns shared entries");
                shared.holders.remove(&old_generation);
                shared.holders.insert(new_generation);
                shared.last_used_at = now;
                return Ok(entry.conn.clone());
            }
        }

        // See claim_existing: never hold the entry mutex across a dial that
        // can wait for browser-owned consent UI.
        let conn = Arc::new(CdpConnection::connect(ws_url).await?);
        conn.restrict_to_existing_profile();
        let mut conns = self.inner.conns.lock().await;
        // Holders of the dead socket rebind through their own reconnect,
        // which joins this socket without dialing.
        self.inner.insert_shared(
            &mut conns,
            ws_url,
            PoolEntry {
                conn: conn.clone(),
                shared: Some(SharedSocket::new(new_generation, Instant::now())),
            },
        );
        Ok(conn)
    }

    /// Drop a (likely dead) unrestricted connection so the next call
    /// redials. Grant-owned sockets are never evicted by the legacy route.
    pub async fn evict(&self, ws_url: &str) {
        let mut conns = self.inner.conns.lock().await;
        if conns
            .get(ws_url)
            .is_some_and(|entry| entry.shared.is_none())
        {
            conns.remove(ws_url);
        }
    }

    /// Release grant `generation`'s hold on the socket for `ws_url`. Only a
    /// current holder can affect the socket, and the socket is never closed
    /// while another session still holds it.
    pub async fn release_existing(&self, ws_url: &str, generation: u64, mode: SharedSocketRelease) {
        let now = Instant::now();
        let mut conns = self.inner.conns.lock().await;
        let Some(entry) = conns.get_mut(ws_url) else {
            return;
        };
        let Some(shared) = entry.shared.as_mut() else {
            return;
        };
        if !shared.holders.remove(&generation) || !shared.holders.is_empty() {
            return;
        }
        if mode == SharedSocketRelease::Park
            && !entry.conn.is_closed()
            && !shared.expired(now, &self.inner.ttl)
        {
            shared.last_used_at = now;
            drop(conns);
            self.schedule_parked_reaper(ws_url);
        } else {
            self.inner.remove_entry(&mut conns, ws_url);
        }
    }

    /// Number of sessions holding the shared socket for `ws_url`, or `None`
    /// when no grant-owned socket exists (diagnostics/tests).
    pub async fn shared_socket_holders(&self, ws_url: &str) -> Option<usize> {
        let conns = self.inner.conns.lock().await;
        conns
            .get(ws_url)
            .and_then(|entry| entry.shared.as_ref())
            .map(|shared| shared.holders.len())
    }

    fn schedule_parked_reaper(&self, ws_url: &str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        let url = ws_url.to_owned();
        let mut wait = self.inner.ttl.idle;
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(wait + Duration::from_millis(10)).await;
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let now = Instant::now();
                let mut conns = inner.conns.lock().await;
                let Some(entry) = conns.get(&url) else {
                    return;
                };
                let Some(shared) = entry.shared.as_ref() else {
                    return;
                };
                if !shared.holders.is_empty() {
                    // Held again; a later release schedules a fresh reaper.
                    return;
                }
                if entry.conn.is_closed() || shared.expired(now, &inner.ttl) {
                    inner.remove_entry(&mut conns, &url);
                    return;
                }
                wait = inner
                    .ttl
                    .idle
                    .saturating_sub(now.duration_since(shared.last_used_at));
            }
        });
    }
}

impl Default for CdpPool {
    fn default() -> Self {
        Self::new()
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
        assert_eq!(
            claimed.method_policy(),
            CdpMethodPolicy::ExistingProfile,
            "claiming a personal-profile socket must restrict it in place"
        );
        assert!(pool.get(&url).await.is_err(), "legacy access must refuse");
        assert!(pool.get_existing(&url, 2).await.is_err());
        let reused = pool.get_existing(&url, 1).await.unwrap();
        assert!(Arc::ptr_eq(&claimed, &reused));
        pool.release_existing(&url, 1, SharedSocketRelease::Close)
            .await;
        assert!(
            pool.get(&url).await.is_ok(),
            "closing the last holder releases ownership"
        );
    }

    /// Accepts browser-level sockets and counts each handshake, i.e. each
    /// time a real browser would show its remote-debugging consent prompt.
    async fn counting_browser() -> (
        String,
        StdArc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let dials = StdArc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = dials.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let counter = counter.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    counter.fetch_add(1, Ordering::SeqCst);
                    while let Some(Ok(_)) = ws.next().await {}
                });
            }
        });
        (
            format!("ws://127.0.0.1:{port}/devtools/browser/shared"),
            dials,
            task,
        )
    }

    #[tokio::test]
    async fn sessions_share_one_browser_socket_and_cannot_close_each_other() {
        let (url, dials, server) = counting_browser().await;
        let pool = CdpPool::new();
        let a = pool.claim_existing(&url, 10).await.unwrap();
        let b = pool.claim_existing(&url, 11).await.unwrap();
        assert!(Arc::ptr_eq(&a, &b), "second session must join, not redial");
        assert_eq!(
            dials.load(Ordering::SeqCst),
            1,
            "one consent for both sessions"
        );
        assert_eq!(pool.shared_socket_holders(&url).await, Some(2));

        // A security close by one session leaves the other session's socket alone.
        pool.release_existing(&url, 10, SharedSocketRelease::Close)
            .await;
        assert!(!b.is_closed());
        let still = pool.get_existing(&url, 11).await.unwrap();
        assert!(Arc::ptr_eq(&b, &still));
        assert!(
            pool.get_existing(&url, 10).await.is_err(),
            "a released generation cannot keep using the shared socket"
        );

        // Releasing a generation that never held the socket is a no-op.
        pool.release_existing(&url, 99, SharedSocketRelease::Close)
            .await;
        assert_eq!(pool.shared_socket_holders(&url).await, Some(1));
        server.abort();
    }

    #[tokio::test]
    async fn parked_socket_serves_the_next_session_without_a_new_dial() {
        let (url, dials, server) = counting_browser().await;
        let pool = CdpPool::new();
        let first = pool.claim_existing(&url, 1).await.unwrap();
        pool.release_existing(&url, 1, SharedSocketRelease::Park)
            .await;
        assert_eq!(pool.shared_socket_holders(&url).await, Some(0), "parked");
        assert!(
            endpoint_port_is_grant_owned(&url),
            "a parked personal-profile socket stays off the legacy route"
        );
        assert!(pool.get(&url).await.is_err());

        let next = pool.claim_existing(&url, 2).await.unwrap();
        assert!(Arc::ptr_eq(&first, &next));
        assert_eq!(dials.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn parked_socket_closes_after_the_idle_bound() {
        let (url, dials, server) = counting_browser().await;
        let pool = CdpPool::with_shared_socket_ttls(
            std::time::Duration::from_millis(50),
            std::time::Duration::from_secs(3600),
        );
        pool.claim_existing(&url, 1).await.unwrap();
        pool.release_existing(&url, 1, SharedSocketRelease::Park)
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(
            pool.shared_socket_holders(&url).await,
            None,
            "the reaper closes an idle parked socket"
        );
        assert!(!endpoint_port_is_grant_owned(&url));

        pool.claim_existing(&url, 2).await.unwrap();
        assert_eq!(
            dials.load(Ordering::SeqCst),
            2,
            "a later session dials again"
        );
        server.abort();
    }

    #[tokio::test]
    async fn socket_past_its_absolute_bound_is_not_joined() {
        let (url, dials, server) = counting_browser().await;
        let pool = CdpPool::with_shared_socket_ttls(
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_millis(50),
        );
        let old = pool.claim_existing(&url, 1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(pool.get_existing(&url, 1).await.is_err());
        let fresh = pool.claim_existing(&url, 2).await.unwrap();
        assert!(!Arc::ptr_eq(&old, &fresh));
        assert_eq!(dials.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn security_close_of_the_last_holder_does_not_park() {
        let (url, _dials, server) = counting_browser().await;
        let pool = CdpPool::new();
        pool.claim_existing(&url, 1).await.unwrap();
        pool.release_existing(&url, 1, SharedSocketRelease::Close)
            .await;
        assert_eq!(pool.shared_socket_holders(&url).await, None);
        assert!(!endpoint_port_is_grant_owned(&url));
        server.abort();
    }

    #[tokio::test]
    async fn reconnect_joins_a_socket_another_session_already_reopened() {
        let (url, dials, server) = counting_browser().await;
        let pool = CdpPool::new();
        let dead = pool.claim_existing(&url, 1).await.unwrap();
        pool.claim_existing(&url, 2).await.unwrap();
        dead.demux.close();

        let reopened = pool.reconnect_existing(&url, 1, 3).await.unwrap();
        let joined = pool.reconnect_existing(&url, 2, 4).await.unwrap();
        assert!(Arc::ptr_eq(&reopened, &joined));
        assert_eq!(
            dials.load(Ordering::SeqCst),
            2,
            "one reconnect dial for both sessions"
        );
        assert_eq!(pool.shared_socket_holders(&url).await, Some(2));
        server.abort();
    }

    #[tokio::test]
    async fn revoking_all_sessions_closes_parked_sockets_only() {
        let (parked_url, _d1, s1) = counting_browser().await;
        let (held_url, _d2, s2) = counting_browser().await;
        let pool = CdpPool::new();
        pool.claim_existing(&parked_url, 1).await.unwrap();
        pool.release_existing(&parked_url, 1, SharedSocketRelease::Park)
            .await;
        pool.claim_existing(&held_url, 2).await.unwrap();

        assert_eq!(pool.inner.close_parked().await, 1);
        assert_eq!(pool.shared_socket_holders(&parked_url).await, None);
        assert_eq!(pool.shared_socket_holders(&held_url).await, Some(1));
        s1.abort();
        s2.abort();
    }

    #[tokio::test]
    async fn existing_profile_policy_rejects_fingerprint_and_interception_methods() {
        let seen = StdArc::new(StdMutex::new(Vec::<String>::new()));
        let server_seen = seen.clone();
        let server = MockCdpServer::start(StdArc::new(move |call| {
            server_seen.lock().unwrap().push(call.method.clone());
            MockReply::ok(json!({}))
        }))
        .await;
        let conn = CdpConnection::connect(&server.ws_url()).await.unwrap();
        conn.restrict_to_existing_profile();

        for method in [
            "Runtime.enable",
            "Target.setDiscoverTargets",
            "Page.addScriptToEvaluateOnNewDocument",
            "Network.enable",
            "Fetch.enable",
            "Emulation.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setTimezoneOverride",
        ] {
            let error = conn.call(None, method, json!({})).await.unwrap_err();
            assert!(
                error.to_string().contains("policy refused"),
                "{method}: {error}"
            );
        }
        assert!(
            seen.lock().unwrap().is_empty(),
            "refused methods must not reach the browser socket"
        );

        conn.call(None, "Target.getTargets", json!({}))
            .await
            .expect("reviewed method remains available");
        assert_eq!(&*seen.lock().unwrap(), &["Target.getTargets"]);
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
            pool.release_existing(&url, 1, SharedSocketRelease::Close),
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

    /// Live check against a real Chrome whose remote debugging was allowed in
    /// `chrome://inspect/#remote-debugging`. Chrome shows its "Allow remote
    /// debugging?" prompt for the first socket; a person (or the driver's
    /// consent adapter) must accept it within the connect timeout. Every later
    /// session must reuse that socket without a second prompt.
    ///
    /// CUA_E2E_CHROME_DEVTOOLS_ACTIVE_PORT=<user-data-dir>/DevToolsActivePort \
    ///   cargo test -p cua-driver-core --lib real_chrome -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs a disposable Chrome with remote debugging allowed and a person to accept one prompt"]
    async fn real_chrome_sessions_share_one_consented_socket() {
        let Some(path) = std::env::var_os("CUA_E2E_CHROME_DEVTOOLS_ACTIVE_PORT") else {
            return;
        };
        let active = std::fs::read_to_string(path).unwrap();
        let mut lines = active.lines();
        let port = lines.next().unwrap().trim();
        let browser_path = lines.next().unwrap().trim();
        let url = format!("ws://127.0.0.1:{port}{browser_path}");
        let pool = CdpPool::new();
        let started = std::time::Instant::now();
        let log = |step: &str| eprintln!("[{:>7.3}s] {step}", started.elapsed().as_secs_f64());

        log("session A: claim (Chrome prompts for this first socket)");
        let a = pool.claim_existing(&url, 1).await.unwrap();
        log("session A: socket open");

        let page = |conn: Arc<CdpConnection>| async move {
            let targets = conn
                .call(None, "Target.getTargets", json!({}))
                .await
                .unwrap();
            targets["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["type"] == "page")
                .map(|t| t["targetId"].as_str().unwrap().to_owned())
                .unwrap()
        };
        let attach_and_eval = |conn: Arc<CdpConnection>, target: String| async move {
            let attached = conn
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({ "targetId": target, "flatten": true }),
                )
                .await
                .unwrap();
            let session = attached["sessionId"].as_str().unwrap().to_owned();
            let value = conn
                .call(
                    Some(&session),
                    "Runtime.evaluate",
                    json!({ "expression": "1 + 1", "returnByValue": true }),
                )
                .await
                .unwrap();
            assert_eq!(value["result"]["value"], 2);
            session
        };
        let target = page(a.clone()).await;
        let session_a = attach_and_eval(a.clone(), target.clone()).await;

        let t = std::time::Instant::now();
        let b = pool.claim_existing(&url, 2).await.unwrap();
        log("session B: claimed");
        assert!(
            Arc::ptr_eq(&a, &b),
            "session B must reuse the consented socket"
        );
        assert!(
            t.elapsed() < std::time::Duration::from_millis(500),
            "no consent wait"
        );
        let session_b = attach_and_eval(b.clone(), target.clone()).await;
        assert_ne!(
            session_a, session_b,
            "each session gets its own flattened CDP session"
        );

        pool.release_existing(&url, 1, SharedSocketRelease::Close)
            .await;
        log("session A: ended");
        let b = pool.get_existing(&url, 2).await.unwrap();
        attach_and_eval(b, target.clone()).await;
        log("session B: still working after A ended");

        pool.release_existing(&url, 2, SharedSocketRelease::Park)
            .await;
        log("session B: ended (socket parked)");
        let t = std::time::Instant::now();
        let c = pool.claim_existing(&url, 3).await.unwrap();
        assert!(
            Arc::ptr_eq(&a, &c),
            "a later session must reuse the parked socket"
        );
        assert!(
            t.elapsed() < std::time::Duration::from_millis(500),
            "no consent wait"
        );
        attach_and_eval(c, target).await;
        log("session C: working on the parked socket without a prompt");
        pool.release_existing(&url, 3, SharedSocketRelease::Close)
            .await;
        log("session C: ended; socket closed");
    }
}
