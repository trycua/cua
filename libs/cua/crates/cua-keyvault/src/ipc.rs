// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault socket: framing, server and client.
//!
//! - One Unix socket, `$CUA_HOME/keyvault.sock`, mode 0600, bound inside a
//!   private 0700 directory and renamed into place (no chmod race). Never
//!   TCP, HTTP, gRPC-Web, WebSocket or MCP: nothing a browser can reach.
//! - Frames: a 4-byte big-endian length, then JSON. At most 1 MiB. One
//!   response per request.
//! - Every connection is identified by the kernel before its first request
//!   ([`crate::caller::identify_peer`]); a peer that cannot be identified is
//!   refused and the refusal is audited.
//! - Clients verify the server the same way (a same-user process could bind
//!   the path first and phish consent).

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroize;

use crate::audit::{AuditEntry, Verification};
#[cfg(unix)]
use crate::broker::StageSink;
use crate::broker::{
    AccessRequest, ApproveOptions, Broker, Decision, ImportReport, ImportSpec, InitRequest,
    Inventory, ItemPage, LockOutcome, LoginOutcome, LoginRequest, PasswordImportSpec, PendingView,
    RuleSpec, SiteIcon, Status, TeleportOutcome, TeleportRequest, TeleportStage, UnlockRequest,
};
use crate::caller::{CallerIdentity, TrustPolicy};
use crate::model::{Delivery, Grant, ItemPolicy, UnattendedRule, UnlockPolicy};
use crate::{Error, Result};

/// Largest frame accepted either way.
pub const MAX_FRAME: usize = 1024 * 1024;
/// Idle time before the server drops a connection.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Requests per connection.
pub const MAX_REQUESTS_PER_CONNECTION: usize = 10_000;

/// A request. `op` selects the operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Status (any caller).
    Status,
    /// Create the vault.
    Init(InitRequest),
    /// Unlock.
    Unlock(UnlockRequest),
    /// Lock.
    Lock,
    /// The kill switch.
    SetDisabled {
        /// On (true) or off.
        disabled: bool,
    },
    /// Auto-wipe of delivered copies (off: they stay until wiped).
    SetAutoWipe {
        /// On (true) or off.
        on: bool,
    },
    /// Unlock policy.
    SetUnlockPolicy {
        /// Policy.
        policy: UnlockPolicy,
        /// Idle minutes.
        #[serde(default)]
        auto_lock_minutes: Option<u32>,
    },
    /// One page of items. Names (domains and keys) only inside the browse
    /// window; outside it: app, type, lock state and times.
    ListItems {
        /// Items to skip.
        #[serde(default)]
        offset: usize,
        /// Page size (default and most: [`crate::broker::MAX_PAGE`]).
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Site icons (empty while the browse window is closed).
    ListFavicons,
    /// Open the browse window (needs presence): item names become visible
    /// for a few minutes.
    Browse,
    /// Close the browse window.
    EndBrowse,
    /// A host app's per-site inventory.
    Inventory {
        /// Provider id.
        app: String,
        /// Profile.
        #[serde(default)]
        profile: Option<String>,
    },
    /// Import into the vault.
    Import(ImportSpec),
    /// Import a browser's saved passwords (sealed; used only to sign in).
    ImportPasswords(PasswordImportSpec),
    /// Sign in to a site in a target Space with a saved password.
    Login(LoginRequest),
    /// Delete items, wiping every live copy of them in Spaces.
    DeleteItems {
        /// Item ids.
        ids: Vec<String>,
    },
    /// Lock or unlock items together. Unlocking allows unattended access
    /// and asks for presence once for the batch.
    SetLocked {
        /// Item ids.
        ids: Vec<String>,
        /// Lock (true) or unlock.
        locked: bool,
    },
    /// "Never ask again" on the unlock prompt.
    SetSkipUnlockPrompt {
        /// On (true) or off.
        on: bool,
    },
    /// Set an item's policy.
    SetItemPolicy {
        /// Item id.
        id: String,
        /// Policy.
        policy: ItemPolicy,
    },
    /// Ask for access (any caller).
    RequestAccess(AccessRequest),
    /// Wait for the answer (the requester only).
    AwaitDecision {
        /// Request id.
        request_id: String,
        /// Wait bound (max 120 s).
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Pending requests.
    ListPending,
    /// Approve.
    Approve {
        /// Request id.
        request_id: String,
        /// Narrowing.
        #[serde(default)]
        options: ApproveOptions,
    },
    /// Deny.
    Deny {
        /// Request id.
        request_id: String,
    },
    /// Grants.
    ListGrants,
    /// Revoke a grant (`*`: all).
    RevokeGrant {
        /// Grant id.
        id: String,
    },
    /// Rules.
    ListRules,
    /// Add a rule.
    AddRule(RuleSpec),
    /// Remove a rule.
    RemoveRule {
        /// Rule id.
        id: String,
    },
    /// Teleport items to a target.
    Teleport(TeleportRequest),
    /// Import and immediately teleport, as one call with one presence
    /// confirmation (`Broker::import_and_teleport`): what a direct
    /// (non-MCP) teleport uses instead of exporting and uploading on its
    /// own.
    ImportAndTeleport {
        /// What to capture.
        spec: ImportSpec,
        /// Target Space.
        target: String,
        /// Keep the captured item(s) in the vault afterward.
        #[serde(default)]
        save: bool,
        /// Stream [`TeleportStage`] frames (`Response::stage`) before the
        /// reply. Off for older clients, which read one reply per request.
        #[serde(default)]
        progress: bool,
        /// Open the app in the Space after importing it. On unless a client
        /// says otherwise (older clients never sent it).
        #[serde(default = "default_true")]
        launch: bool,
    },
    /// Wipe deliveries on a target.
    Release {
        /// Target.
        target: String,
    },
    /// Deliveries.
    ListDeliveries,
    /// Audit tail.
    Audit {
        /// Entries.
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Verify the audit chain.
    VerifyAudit,
}

/// Error codes on the wire.
pub fn error_code(e: &Error) -> &'static str {
    match e {
        Error::Locked => "locked",
        Error::NoVault(_) => "no_vault",
        Error::Disabled => "disabled",
        // Distinguishable from a plain "forbidden" (S1): the caller can
        // retry with relay_plaintext_ack instead of treating this as a
        // dead end, the way a UI shows an acknowledgement prompt rather
        // than a raw error.
        Error::Forbidden(m) if m.contains("relay_plaintext_ack") => "relay_plaintext_unsealed",
        Error::Forbidden(_) => "forbidden",
        Error::PresenceFailed(_) => "presence_failed",
        Error::Denied(_) => "denied",
        Error::Capability(m) if m.contains("replay") => "replay",
        Error::Capability(m) if m.contains("expired") => "expired",
        Error::Capability(_) => "forbidden",
        Error::NotFound(_) => "not_found",
        Error::Invalid(_) | Error::WrongCredential => "invalid",
        Error::Unsupported(_) => "unsupported",
        Error::RateLimited(_) => "rate_limited",
        Error::HostEffectsRefused(_) => "host_effects_refused",
        _ => "internal",
    }
}

/// A wire error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    /// Code (see [`error_code`]).
    pub code: String,
    /// Message (never contains secrets).
    pub message: String,
}

/// A response frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    /// Success.
    pub ok: bool,
    /// Result (when `ok`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (when not `ok`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
    /// A progress frame before the reply (only to a request that asked
    /// for them); the reply itself never has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<TeleportStage>,
}

impl Response {
    fn ok(v: impl Serialize) -> Self {
        Self {
            ok: true,
            result: Some(serde_json::to_value(v).unwrap_or(Value::Null)),
            error: None,
            stage: None,
        }
    }

    fn err(e: &Error) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(WireError {
                code: error_code(e).into(),
                message: e.to_string(),
            }),
            stage: None,
        }
    }

    // Stage frames stream over the Unix socket only.
    #[cfg(unix)]
    fn stage(s: TeleportStage) -> Self {
        Self {
            ok: true,
            result: None,
            error: None,
            stage: Some(s),
        }
    }
}

/// Most stage frames one request streams (each upload chunk is one).
pub const MAX_STAGE_FRAMES: usize = 100_000;

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame of {n} bytes exceeds {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "response too large",
        ));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(bytes).await?;
    w.flush().await
}

/// Dispatches one request for a verified caller.
pub async fn dispatch(broker: &Broker, caller: &CallerIdentity, req: Request) -> Response {
    macro_rules! reply {
        ($e:expr) => {
            match $e {
                Ok(v) => Response::ok(v),
                Err(e) => Response::err(&e),
            }
        };
    }
    match req {
        Request::Status => Response::ok(broker.status(caller).await),
        Request::Init(r) => reply!(
            broker
                .init(caller, r)
                .await
                .map(|k| serde_json::json!({ "recovery_key": k }))
        ),
        Request::Unlock(r) => reply!(broker.unlock(caller, r).await),
        Request::Lock => reply!(broker.lock(caller).await),
        Request::SetDisabled { disabled } => reply!(broker.set_disabled(caller, disabled).await),
        Request::SetAutoWipe { on } => reply!(broker.set_auto_wipe(caller, on).await),
        Request::SetUnlockPolicy {
            policy,
            auto_lock_minutes,
        } => reply!(
            broker
                .set_unlock_policy(caller, policy, auto_lock_minutes)
                .await
        ),
        Request::ListItems { offset, limit } => reply!(
            broker
                .list_items(caller, offset, limit.unwrap_or(crate::broker::MAX_PAGE))
                .await
        ),
        Request::ListFavicons => reply!(broker.list_favicons(caller).await),
        Request::Browse => reply!(
            broker
                .browse(caller)
                .await
                .map(|until| serde_json::json!({ "browse_until_ms": until }))
        ),
        Request::EndBrowse => reply!(broker.end_browse(caller).await),
        Request::Inventory { app, profile } => {
            reply!(broker.inventory(caller, &app, profile.as_deref()).await)
        }
        Request::Import(spec) => reply!(broker.import(caller, spec).await),
        Request::ImportPasswords(spec) => reply!(broker.import_passwords(caller, spec).await),
        Request::Login(r) => reply!(broker.login(caller, r).await),
        Request::DeleteItems { ids } => reply!(broker.delete_items(caller, ids).await),
        Request::SetLocked { ids, locked } => reply!(broker.set_locked(caller, ids, locked).await),
        Request::SetSkipUnlockPrompt { on } => {
            reply!(broker.set_skip_unlock_prompt(caller, on).await)
        }
        Request::SetItemPolicy { id, policy } => {
            reply!(broker.set_item_policy(caller, &id, policy).await)
        }
        Request::RequestAccess(r) => reply!(broker.request_access(caller, r).await),
        Request::AwaitDecision {
            request_id,
            timeout_ms,
        } => reply!(
            broker
                .await_decision(
                    caller,
                    &request_id,
                    Duration::from_millis(timeout_ms.unwrap_or(0).min(120_000))
                )
                .await
        ),
        Request::ListPending => reply!(broker.list_pending(caller).await),
        Request::Approve {
            request_id,
            options,
        } => reply!(broker.approve(caller, &request_id, options).await),
        Request::Deny { request_id } => reply!(broker.deny(caller, &request_id).await),
        Request::ListGrants => reply!(broker.list_grants(caller).await),
        Request::RevokeGrant { id } => reply!(broker.revoke_grant(caller, &id).await),
        Request::ListRules => reply!(broker.list_rules(caller).await),
        Request::AddRule(spec) => reply!(broker.add_rule(caller, spec).await),
        Request::RemoveRule { id } => reply!(broker.remove_rule(caller, &id).await),
        Request::Teleport(r) => reply!(broker.teleport(caller, r).await),
        Request::ImportAndTeleport {
            spec,
            target,
            save,
            launch,
            ..
        } => {
            reply!(
                broker
                    .import_and_teleport_launching(caller, spec, target, save, launch, None)
                    .await
            )
        }
        Request::Release { target } => reply!(broker.release(caller, &target).await),
        Request::ListDeliveries => reply!(broker.list_deliveries(caller).await),
        Request::Audit { limit } => reply!(broker.audit_tail(caller, limit.unwrap_or(200)).await),
        Request::VerifyAudit => reply!(
            broker
                .verify_audit(caller)
                .await
                .map(VerificationView::from)
        ),
    }
}

/// Serializable audit verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationView {
    /// Intact.
    pub ok: bool,
    /// Entries.
    pub entries: u64,
    /// Entries written while locked (not MAC'd).
    pub unauthenticated: u64,
    /// First bad line.
    #[serde(default)]
    pub tampered_line: Option<u64>,
    /// Why.
    #[serde(default)]
    pub reason: Option<String>,
}

impl From<Verification> for VerificationView {
    fn from(v: Verification) -> Self {
        Self {
            ok: v.ok(),
            entries: v.entries,
            unauthenticated: v.unauthenticated,
            tampered_line: v.tamper.as_ref().map(|t| t.line),
            reason: v.tamper.map(|t| t.reason),
        }
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Binds the keyvault socket at `path`: inside a private directory first,
/// then renamed into place, so it is never reachable with looser modes.
/// Refuses when a live server already answers there.
#[cfg(unix)]
pub async fn bind(path: &Path) -> Result<tokio::net::UnixListener> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("socket path has no parent".into()))?;
    crate::store::private_dir(parent)?;
    // The socket must live in a 0700 directory this user owns, so no other user
    // (and no attacker via a redirected $CUA_HOME) can reach or replace it
    // (red-team F16).
    crate::validate_socket_dir(parent)?;
    if path.exists() {
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            return Err(Error::Invalid(format!(
                "another Keyvault is already listening at {}",
                path.display()
            )));
        }
        std::fs::remove_file(path)?;
    }
    let staging = parent.join(format!(".kv-bind-{}", crate::crypto::random_id()?));
    crate::store::private_dir(&staging)?;
    let tmp = staging.join("s");
    let listener = tokio::net::UnixListener::bind(&tmp)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    let res = std::fs::rename(&tmp, path);
    let _ = std::fs::remove_dir(&staging);
    res?;
    Ok(listener)
}

/// Serves until the listener fails. Each connection runs in its own task.
#[cfg(unix)]
pub async fn serve(listener: tokio::net::UnixListener, broker: Arc<Broker>, policy: TrustPolicy) {
    let policy = Arc::new(policy);
    // Bounded by the process lifetime; each iteration handles one accept.
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "keyvault accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let broker = broker.clone();
        let policy = policy.clone();
        tokio::spawn(async move {
            handle(stream, broker, &policy).await;
        });
    }
}

#[cfg(unix)]
async fn handle(mut stream: tokio::net::UnixStream, broker: Arc<Broker>, policy: &TrustPolicy) {
    use std::os::fd::AsRawFd;
    let caller = match crate::caller::identify_peer(stream.as_raw_fd(), policy) {
        Ok(c) => c,
        Err(e) => {
            broker.record_rejected(&e.to_string()).await;
            let resp = Response::err(&Error::Forbidden(format!("caller not identified: {e}")));
            if let Ok(b) = serde_json::to_vec(&resp) {
                let _ = write_frame(&mut stream, &b).await;
            }
            return;
        }
    };
    broker.remember_caller(&caller).await;
    for _ in 0..MAX_REQUESTS_PER_CONNECTION {
        let mut frame = match tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut stream)).await {
            Ok(Ok(Some(f))) => f,
            _ => return,
        };
        let parsed = serde_json::from_slice::<Request>(&frame);
        // An `init` or `unlock` frame carries a passphrase: wipe the raw
        // bytes as soon as they are parsed.
        frame.zeroize();
        let resp = match parsed {
            Ok(Request::ImportAndTeleport {
                spec,
                target,
                save,
                progress: true,
                launch,
            }) => {
                match teleport_streaming(&mut stream, &broker, &caller, spec, target, save, launch)
                    .await
                {
                    Some(r) => r,
                    None => return,
                }
            }
            Ok(req) => dispatch(&broker, &caller, req).await,
            Err(e) => Response::err(&Error::Invalid(format!("bad request: {e}"))),
        };
        let Ok(mut bytes) = serde_json::to_vec(&resp) else {
            return;
        };
        drop(resp);
        // An `init` reply carries the recovery key.
        let written = write_frame(&mut stream, &bytes).await;
        bytes.zeroize();
        if written.is_err() {
            return;
        }
    }
}

/// `import_and_teleport` with its stages written as frames while it runs;
/// the reply is returned for the caller to write. None when the client went
/// away (the teleport still finishes).
#[cfg(unix)]
async fn teleport_streaming(
    stream: &mut tokio::net::UnixStream,
    broker: &Broker,
    caller: &CallerIdentity,
    spec: ImportSpec,
    target: String,
    save: bool,
    launch: bool,
) -> Option<Response> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TeleportStage>();
    let sink: StageSink = Arc::new(move |s| {
        let _ = tx.send(s);
    });
    let work = broker.import_and_teleport_launching(caller, spec, target, save, launch, Some(sink));
    tokio::pin!(work);
    let mut connected = true;
    let mut sent = 0usize;
    // Bounded: the teleport future ends, and at most MAX_STAGE_FRAMES go out.
    let result = loop {
        tokio::select! {
            r = &mut work => break r,
            Some(s) = rx.recv() => {
                if connected && sent < MAX_STAGE_FRAMES {
                    sent += 1;
                    let frame = serde_json::to_vec(&Response::stage(s)).unwrap_or_default();
                    connected = write_frame(stream, &frame).await.is_ok();
                }
            }
        }
    };
    if !connected {
        return None;
    }
    while let Ok(s) = rx.try_recv() {
        if sent >= MAX_STAGE_FRAMES {
            break;
        }
        sent += 1;
        let frame = serde_json::to_vec(&Response::stage(s)).unwrap_or_default();
        write_frame(stream, &frame).await.ok()?;
    }
    Some(match result {
        Ok(v) => Response::ok(v),
        Err(e) => Response::err(&e),
    })
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// How a client checks the server.
#[derive(Clone, Debug)]
pub enum ServerCheck {
    /// The server must satisfy this policy's first-party requirement.
    Require(TrustPolicy),
    /// No check (tests, and debug builds with
    /// `CUA_KEYVAULT_ALLOW_UNVERIFIED_DAEMON=1`).
    Unverified,
}

impl ServerCheck {
    /// Production in release builds. Debug builds may opt out with
    /// `CUA_KEYVAULT_ALLOW_UNVERIFIED_DAEMON=1` (unsigned dev daemons).
    pub fn default_for_build() -> Self {
        if cfg!(debug_assertions)
            && std::env::var("CUA_KEYVAULT_ALLOW_UNVERIFIED_DAEMON").as_deref() == Ok("1")
        {
            ServerCheck::Unverified
        } else {
            ServerCheck::Require(TrustPolicy::production())
        }
    }
}

/// The client's connection: the Keyvault socket. Peer verification
/// ([`crate::caller::identify_peer`]) exists only for Unix sockets, so other
/// OSes have no connection type and [`KeyvaultClient::connect`] refuses.
#[cfg(unix)]
type ClientStream = tokio::net::UnixStream;
#[cfg(not(unix))]
type ClientStream = NoConnection;

/// Uninhabited: no Keyvault connection exists on this OS.
#[cfg(not(unix))]
pub enum NoConnection {}

#[cfg(not(unix))]
impl tokio::io::AsyncRead for NoConnection {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match *self {}
    }
}

#[cfg(not(unix))]
impl tokio::io::AsyncWrite for NoConnection {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match *self {}
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match *self {}
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match *self {}
    }
}

/// A Keyvault client.
pub struct KeyvaultClient {
    stream: ClientStream,
    /// The server as verified at connect.
    pub server: Option<CallerIdentity>,
}

/// Why connecting failed.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Nothing is listening: Cua is not installed or not running.
    #[error("the Cua Keyvault is not running at {0}")]
    NotRunning(PathBuf),
    /// The listener is not the Cua daemon.
    #[error("the process at {path} is not the Cua daemon ({who}); refusing to talk to it")]
    Impostor {
        /// Socket path.
        path: PathBuf,
        /// What it is.
        who: String,
    },
    /// Other failure.
    #[error("keyvault connect: {0}")]
    Other(String),
}

impl KeyvaultClient {
    /// Connects to `path` and verifies the server per `check`.
    #[cfg(unix)]
    pub async fn connect(
        path: &Path,
        check: ServerCheck,
    ) -> std::result::Result<Self, ConnectError> {
        use std::os::fd::AsRawFd;
        // The socket's directory must be a 0700 dir this user owns before we
        // even connect: on Linux the server's signature is unverifiable, so an
        // unsafe directory (for example one a malicious $CUA_HOME points at) is
        // the phishing seam (red-team F16). No socket (or no directory) just
        // means Cua is not running: say so, since there is nothing to trust or
        // refuse yet (the daemon makes the directory 0700 when it binds).
        if std::fs::symlink_metadata(path).is_err() {
            return Err(ConnectError::NotRunning(path.to_path_buf()));
        }
        if let Some(parent) = path.parent() {
            crate::validate_socket_dir(parent).map_err(|e| ConnectError::Other(e.to_string()))?;
        }
        let stream = tokio::net::UnixStream::connect(path).await.map_err(|e| {
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                ConnectError::NotRunning(path.to_path_buf())
            } else {
                ConnectError::Other(e.to_string())
            }
        })?;
        let server = match check {
            ServerCheck::Unverified => None,
            ServerCheck::Require(policy) => {
                let id = crate::caller::identify_peer(stream.as_raw_fd(), &policy)
                    .map_err(|e| ConnectError::Other(e.to_string()))?;
                if !id.first_party {
                    return Err(ConnectError::Impostor {
                        path: path.to_path_buf(),
                        who: id.display(),
                    });
                }
                Some(id)
            }
        };
        Ok(Self { stream, server })
    }

    /// Connects to `path`: the Keyvault IPC needs a kernel-verified Unix
    /// socket peer, so it is not available on this OS yet.
    #[cfg(not(unix))]
    pub async fn connect(
        path: &Path,
        check: ServerCheck,
    ) -> std::result::Result<Self, ConnectError> {
        let _ = (path, check);
        Err(ConnectError::Other(
            "the Cua Keyvault is not available on this OS yet".into(),
        ))
    }

    /// Connects to the default socket.
    pub async fn connect_default() -> std::result::Result<Self, ConnectError> {
        let path = crate::default_socket()
            .ok_or_else(|| ConnectError::Other("HOME and CUA_HOME are unset".into()))?;
        Self::connect(&path, ServerCheck::default_for_build()).await
    }

    /// Sends one request and returns the raw result.
    pub async fn call(&mut self, req: &Request) -> Result<Value> {
        self.call_with_stages(req, &mut |_| {}).await
    }

    /// [`Self::call`], handing each stage frame before the reply to `on`.
    async fn call_with_stages(
        &mut self,
        req: &Request,
        on: &mut (dyn FnMut(TeleportStage) + Send),
    ) -> Result<Value> {
        let mut bytes = serde_json::to_vec(req)?;
        let written = write_frame(&mut self.stream, &bytes).await;
        // `init` and `unlock` frames carry a passphrase.
        bytes.zeroize();
        written?;
        // Bounded: the server sends at most MAX_STAGE_FRAMES before the reply.
        let mut resp = None;
        for _ in 0..=MAX_STAGE_FRAMES {
            let mut frame = read_frame(&mut self.stream)
                .await?
                .ok_or_else(|| Error::Backend("the Keyvault closed the connection".into()))?;
            let parsed = serde_json::from_slice::<Response>(&frame);
            frame.zeroize();
            let r = parsed?;
            match r.stage {
                Some(s) => on(s),
                None => {
                    resp = Some(r);
                    break;
                }
            }
        }
        let resp = resp.ok_or_else(|| Error::Backend("too many progress frames".into()))?;
        if resp.ok {
            Ok(resp.result.unwrap_or(Value::Null))
        } else {
            let e = resp.error.unwrap_or(WireError {
                code: "internal".into(),
                message: "unknown error".into(),
            });
            Err(wire_to_error(e))
        }
    }

    async fn typed<T: serde::de::DeserializeOwned>(&mut self, req: &Request) -> Result<T> {
        Ok(serde_json::from_value(self.call(req).await?)?)
    }

    /// Status.
    pub async fn status(&mut self) -> Result<Status> {
        self.typed(&Request::Status).await
    }
    /// Creates the vault (first party; the daemon asks for presence).
    /// Returns the recovery key when one was asked for: show it once.
    pub async fn init(&mut self, req: InitRequest) -> Result<Option<String>> {
        let v = self.call(&Request::Init(req)).await?;
        Ok(v.get("recovery_key")
            .and_then(Value::as_str)
            .map(str::to_string))
    }
    /// Unlocks with the OS key store (no credential), a passphrase or the
    /// recovery key (first party).
    pub async fn unlock(&mut self, req: UnlockRequest) -> Result<()> {
        self.call(&Request::Unlock(req)).await.map(|_| ())
    }
    /// Locks (first party).
    pub async fn lock(&mut self) -> Result<()> {
        self.call(&Request::Lock).await.map(|_| ())
    }
    /// Every item, page by page. Names are present only inside the browse
    /// window ([`Self::browse`]).
    pub async fn list_items(&mut self) -> Result<ItemPage> {
        let mut all = ItemPage::default();
        loop {
            let page: ItemPage = self
                .typed(&Request::ListItems {
                    offset: all.items.len(),
                    limit: None,
                })
                .await?;
            let empty = page.items.is_empty();
            all.total = page.total;
            all.names_visible = page.names_visible;
            all.items.extend(page.items);
            if empty || all.items.len() >= all.total {
                return Ok(all);
            }
        }
    }
    /// Site icons, empty while the browse window is closed.
    pub async fn list_favicons(&mut self) -> Result<Vec<SiteIcon>> {
        let v = self.call(&Request::ListFavicons).await?;
        serde_json::from_value(v).map_err(|e| Error::Invalid(e.to_string()))
    }
    /// Opens the browse window (the daemon asks for presence). Returns when
    /// it closes, Unix ms.
    pub async fn browse(&mut self) -> Result<u64> {
        let v = self.call(&Request::Browse).await?;
        Ok(v.get("browse_until_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0))
    }
    /// Closes the browse window.
    pub async fn end_browse(&mut self) -> Result<()> {
        self.call(&Request::EndBrowse).await.map(|_| ())
    }
    /// Deletes items and wipes their live copies. Returns the wiped imports.
    pub async fn delete_items(&mut self, ids: Vec<String>) -> Result<Vec<String>> {
        self.typed(&Request::DeleteItems { ids }).await
    }
    /// Locks or unlocks items together (unlocking asks for presence once).
    pub async fn set_locked(&mut self, ids: Vec<String>, locked: bool) -> Result<LockOutcome> {
        self.typed(&Request::SetLocked { ids, locked }).await
    }
    /// "Never ask again" on the unlock prompt.
    pub async fn set_skip_unlock_prompt(&mut self, on: bool) -> Result<()> {
        self.call(&Request::SetSkipUnlockPrompt { on })
            .await
            .map(|_| ())
    }
    /// Inventory.
    pub async fn inventory(&mut self, app: &str, profile: Option<&str>) -> Result<Inventory> {
        self.typed(&Request::Inventory {
            app: app.into(),
            profile: profile.map(Into::into),
        })
        .await
    }
    /// Import.
    pub async fn import(&mut self, spec: ImportSpec) -> Result<ImportReport> {
        self.typed(&Request::Import(spec)).await
    }
    /// Import a browser's saved passwords (first party; presence).
    pub async fn import_passwords(&mut self, spec: PasswordImportSpec) -> Result<ImportReport> {
        self.typed(&Request::ImportPasswords(spec)).await
    }
    /// Sign in to a site in a Space with a saved password.
    pub async fn login(&mut self, req: LoginRequest) -> Result<LoginOutcome> {
        self.typed(&Request::Login(req)).await
    }
    /// Request access.
    pub async fn request_access(&mut self, req: AccessRequest) -> Result<PendingView> {
        self.typed(&Request::RequestAccess(req)).await
    }
    /// Await a decision.
    pub async fn await_decision(
        &mut self,
        request_id: &str,
        timeout: Duration,
    ) -> Result<Decision> {
        self.typed(&Request::AwaitDecision {
            request_id: request_id.into(),
            timeout_ms: Some(timeout.as_millis() as u64),
        })
        .await
    }
    /// Pending.
    pub async fn list_pending(&mut self) -> Result<Vec<PendingView>> {
        self.typed(&Request::ListPending).await
    }
    /// Approve.
    pub async fn approve(&mut self, request_id: &str, options: ApproveOptions) -> Result<Grant> {
        self.typed(&Request::Approve {
            request_id: request_id.into(),
            options,
        })
        .await
    }
    /// Deny.
    pub async fn deny(&mut self, request_id: &str) -> Result<()> {
        self.typed(&Request::Deny {
            request_id: request_id.into(),
        })
        .await
    }
    /// Teleport.
    pub async fn teleport(&mut self, req: TeleportRequest) -> Result<TeleportOutcome> {
        self.typed(&Request::Teleport(req)).await
    }
    /// Import and immediately teleport in one call, one presence
    /// confirmation (first party). `save`: keep the captured item(s) in the
    /// vault afterward, or forget them once delivery completes.
    pub async fn import_and_teleport(
        &mut self,
        spec: ImportSpec,
        target: String,
        save: bool,
    ) -> Result<TeleportOutcome> {
        self.typed(&Request::ImportAndTeleport {
            spec,
            target,
            save,
            progress: false,
            launch: true,
        })
        .await
    }
    /// [`Self::import_and_teleport`], telling `on` each [`TeleportStage`]
    /// as the daemon reaches it.
    pub async fn import_and_teleport_with_progress(
        &mut self,
        spec: ImportSpec,
        target: String,
        save: bool,
        on: impl FnMut(TeleportStage) + Send,
    ) -> Result<TeleportOutcome> {
        self.import_and_teleport_launching(spec, target, save, true, on)
            .await
    }
    /// [`Self::import_and_teleport_with_progress`] with an explicit `launch`
    /// (whether the Space opens the app once it is imported).
    pub async fn import_and_teleport_launching(
        &mut self,
        spec: ImportSpec,
        target: String,
        save: bool,
        launch: bool,
        mut on: impl FnMut(TeleportStage) + Send,
    ) -> Result<TeleportOutcome> {
        let v = self
            .call_with_stages(
                &Request::ImportAndTeleport {
                    spec,
                    target,
                    save,
                    progress: true,
                    launch,
                },
                &mut on,
            )
            .await?;
        Ok(serde_json::from_value(v)?)
    }
    /// Release.
    pub async fn release(&mut self, target: &str) -> Result<Vec<String>> {
        self.typed(&Request::Release {
            target: target.into(),
        })
        .await
    }
    /// Grants.
    pub async fn list_grants(&mut self) -> Result<Vec<Grant>> {
        self.typed(&Request::ListGrants).await
    }
    /// Rules.
    pub async fn list_rules(&mut self) -> Result<Vec<UnattendedRule>> {
        self.typed(&Request::ListRules).await
    }
    /// Deliveries.
    pub async fn list_deliveries(&mut self) -> Result<Vec<Delivery>> {
        self.typed(&Request::ListDeliveries).await
    }
    /// Audit.
    pub async fn audit(&mut self, limit: usize) -> Result<Vec<AuditEntry>> {
        self.typed(&Request::Audit { limit: Some(limit) }).await
    }
    /// Verify audit.
    pub async fn verify_audit(&mut self) -> Result<VerificationView> {
        self.typed(&Request::VerifyAudit).await
    }
    /// Kill switch.
    pub async fn set_disabled(&mut self, disabled: bool) -> Result<()> {
        self.typed(&Request::SetDisabled { disabled }).await
    }
    /// Auto-wipe of delivered copies.
    pub async fn set_auto_wipe(&mut self, on: bool) -> Result<()> {
        self.typed(&Request::SetAutoWipe { on }).await
    }
}

/// Maps a wire error back to [`Error`].
pub fn wire_to_error(e: WireError) -> Error {
    match e.code.as_str() {
        "locked" => Error::Locked,
        "no_vault" => Error::NoVault(e.message),
        "disabled" => Error::Disabled,
        "forbidden" => Error::Forbidden(e.message),
        "presence_failed" => Error::PresenceFailed(e.message),
        "denied" => Error::Denied(e.message),
        "replay" | "expired" => Error::Capability(e.message),
        "not_found" => Error::NotFound(e.message),
        "invalid" => Error::Invalid(e.message),
        "unsupported" => Error::Unsupported(e.message),
        "rate_limited" => Error::RateLimited(e.message),
        "host_effects_refused" => Error::HostEffectsRefused(e.message),
        _ => Error::Backend(e.message),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Red-team F16: the socket-directory guard accepts a 0700 dir this user
    /// owns and refuses one reachable by other users.
    #[test]
    fn socket_dir_guard_requires_a_private_owned_dir() {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(crate::validate_socket_dir(d.path()).is_ok());
        // Loosen to 0755: refused.
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            crate::validate_socket_dir(d.path()),
            Err(Error::Forbidden(_))
        ));
        // Group-readable is also refused.
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(matches!(
            crate::validate_socket_dir(d.path()),
            Err(Error::Forbidden(_))
        ));
        // Restore so TempDir cleanup can remove it.
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// A loose `$CUA_HOME` with no socket in it is "not running", not a
    /// refusal: there is nothing to connect to (a user's existing 0755
    /// `~/.cua` before the daemon first binds and tightens it).
    #[tokio::test]
    async fn no_socket_in_a_loose_dir_is_not_running() {
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = d.path().join("keyvault.sock");
        let err = KeyvaultClient::connect(&path, ServerCheck::Unverified)
            .await
            .err()
            .expect("no server");
        assert!(
            matches!(&err, ConnectError::NotRunning(p) if *p == path),
            "{err:?}"
        );
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// Red-team F16: the daemon binds only inside a private directory (it forces
    /// 0700), and a client refuses to connect through a directory reachable by
    /// other users (the Linux phishing seam), rather than talking to whatever
    /// is there. Uses a short `/tmp` base so the socket path stays under
    /// `SUN_LEN`.
    #[tokio::test]
    async fn bind_is_private_and_connect_refuses_an_unsafe_dir() {
        let d = tempfile::Builder::new()
            .prefix("kv")
            .tempdir_in("/tmp")
            .unwrap();
        let sock = d.path().join("s.sock");

        // Binding succeeds and the directory ends up 0700 regardless of umask.
        let listener = bind(&sock).await.unwrap();
        drop(listener);
        std::fs::remove_file(&sock).ok();
        let mode = std::fs::metadata(d.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "bind must leave the socket dir private");

        // Loosen the directory and place a file where the socket would be: a
        // client refuses to connect through it (never reaches the syscall).
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&sock, b"").ok();
        let cerr = KeyvaultClient::connect(&sock, ServerCheck::Unverified)
            .await
            .err();
        assert!(
            matches!(cerr, Some(ConnectError::Other(_))),
            "an unsafe socket dir must be refused at connect: {cerr:?}"
        );
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// A streaming `import_and_teleport`: stage frames, then the reply; the
    /// client hands each stage over in order and returns the outcome. The
    /// request opts in, so an older daemon that ignores the flag (one reply,
    /// no stages) still works.
    #[tokio::test]
    async fn teleport_stages_stream_before_the_reply() {
        let (client_end, mut server_end) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            let req = read_frame(&mut server_end).await.unwrap().unwrap();
            let req: Request = serde_json::from_slice(&req).unwrap();
            assert!(matches!(
                req,
                Request::ImportAndTeleport { progress: true, .. }
            ));
            for st in [
                TeleportStage::Reading,
                TeleportStage::Packing,
                TeleportStage::Uploading { done: 5, total: 10 },
                TeleportStage::Importing,
            ] {
                let f = serde_json::to_vec(&Response::stage(st)).unwrap();
                write_frame(&mut server_end, &f).await.unwrap();
            }
            let done = Response::ok(TeleportOutcome {
                authority: "interactive".into(),
                ..Default::default()
            });
            write_frame(&mut server_end, &serde_json::to_vec(&done).unwrap())
                .await
                .unwrap();
        });
        let mut client = KeyvaultClient {
            stream: client_end,
            server: None,
        };
        let mut seen = Vec::new();
        let out = client
            .import_and_teleport_with_progress(ImportSpec::default(), "dev-1".into(), false, |st| {
                seen.push(st)
            })
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(out.authority, "interactive");
        assert_eq!(
            seen,
            [
                TeleportStage::Reading,
                TeleportStage::Packing,
                TeleportStage::Uploading { done: 5, total: 10 },
                TeleportStage::Importing,
            ]
        );
        // On the wire a stage is tagged and the reply carries none.
        let wire = serde_json::to_value(Response::stage(TeleportStage::Uploading {
            done: 1,
            total: 2,
        }))
        .unwrap();
        assert_eq!(wire["stage"]["stage"], "uploading");
        assert!(
            serde_json::to_value(Response::ok(1))
                .unwrap()
                .get("stage")
                .is_none()
        );
        // An older client's request (no flag) asks for no stages.
        let old: Request = serde_json::from_str(
            r#"{"op":"import_and_teleport","spec":{"app":"chrome"},"target":"t"}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            Request::ImportAndTeleport {
                progress: false,
                launch: true,
                ..
            }
        ));
    }

    #[test]
    fn cua_home_override_outside_the_real_home_is_untrusted() {
        // No override -> trusted (~/.cua).
        temp_env(&[("CUA_HOME", None)], || {
            assert!(crate::cua_home_within_real_home());
        });
        // Override inside HOME -> trusted; outside -> untrusted.
        let home = tempfile::tempdir().unwrap();
        let inside = home.path().join(".cua");
        std::fs::create_dir_all(&inside).unwrap();
        let outside = tempfile::tempdir().unwrap();
        temp_env(
            &[
                ("HOME", Some(home.path().to_str().unwrap())),
                ("CUA_HOME", Some(inside.to_str().unwrap())),
            ],
            || assert!(crate::cua_home_within_real_home()),
        );
        temp_env(
            &[
                ("HOME", Some(home.path().to_str().unwrap())),
                ("CUA_HOME", Some(outside.path().to_str().unwrap())),
            ],
            || assert!(!crate::cua_home_within_real_home()),
        );
    }

    /// Sets env vars for the closure, restoring them after. Serialized by a
    /// mutex so concurrent env tests do not race.
    fn temp_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(String, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var_os(k)))
            .collect();
        for (k, v) in vars {
            match v {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        f();
        for (k, v) in saved {
            match v {
                Some(v) => unsafe { std::env::set_var(&k, v) },
                None => unsafe { std::env::remove_var(&k) },
            }
        }
    }
}

fn default_true() -> bool {
    true
}
