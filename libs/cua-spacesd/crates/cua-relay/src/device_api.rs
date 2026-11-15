// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Device enrollment API (account mode), see [`crate::devices`]:
//!
//! - `POST   /v1/devices`          register this device (proof of its key);
//!   enrolls it right after a fresh sign-in, else returns a one-time code.
//!   An enrollment replaces older devices of the same machine.
//! - `POST   /v1/devices/session`  signed timestamp -> device session token.
//! - `POST   /v1/devices/approve`  from an enrolled device: approve a
//!   pending or expired device by code or id (a `dev_…` code is an id).
//! - `GET    /v1/devices`          the account's devices.
//! - `PATCH  /v1/devices/{id}`     rename (enrolled device).
//! - `DELETE /v1/devices/{id}`     revoke (enrolled device).
//! - `GET    /v1/audit`            the account's audit log.
//!
//! Every call carries the account token; machine tokens are refused.

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::assertion::now_secs;
use crate::auth_log::{self, Reason, Route, TokenFacts};
use crate::devices::{
    AuditEvent, DeviceError, DeviceRecord, DeviceState, Registration, DEVICE_SESSION_HEADER,
    ENROLLMENT_HEADER,
};
use crate::directory::{MachineRecord, Role};
use crate::oidc::Identity;
use crate::server::{account_token, bearer, Relay};

/// Whether a registration is a phone's: it says so, or its token was issued
/// to the phone app's client. Only while the relay has phone support on (a
/// relay without it records nothing about phones).
fn is_phone_registration(relay: &Relay, who: &Identity, platform: &str) -> bool {
    let Some(push) = &relay.config.push else {
        return crate::phone::platform_is_phone(platform);
    };
    crate::phone::platform_is_phone(platform)
        || who
            .azp
            .as_deref()
            .is_some_and(|a| push.phone_clients().iter().any(|c| c == a))
}

/// How an account request's device stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// An enrolled device (its id) holds a live session.
    Device(String),
    /// No enrolled device, let through during the migration grace period.
    Grace,
    /// Device enrollment is off on this relay.
    Off,
}

impl Gate {
    /// The enrolled device, if any.
    pub fn device(&self) -> Option<&str> {
        match self {
            Gate::Device(id) => Some(id),
            Gate::Grace | Gate::Off => None,
        }
    }
}

/// What a phone is told by a relay without phone support.
pub const PHONE_SUPPORT_OFF: &str = "this relay does not serve phones";

/// The error an unenrolled device gets once the grace period is over.
pub const NOT_ENROLLED: &str =
    "this device is not enrolled for your cua.ai account: sign in again (`cua auth login`) to enroll it, run `cua devices enroll`, or approve it from the Cua Spaces app on an enrolled device";

/// Checks the device behind an account request: an enrolled device's live
/// session, else (during the grace period) a flagged pass, else an error.
pub fn gate(relay: &Relay, session: Option<&str>, who: &Identity) -> Result<Gate, String> {
    if !relay.config.device_enrollment || relay.config.oidc.is_none() {
        return Ok(Gate::Off);
    }
    if let Some(device) = session.and_then(|t| relay.devices.session_device(&who.account, t)) {
        // A phone is served only by a relay that has phone support on.
        if !relay.config.push.as_ref().is_some_and(|p| p.phones())
            && relay.devices.is_phone(&device)
        {
            return Err(PHONE_SUPPORT_OFF.into());
        }
        return Ok(Gate::Device(device));
    }
    if relay.devices.in_grace(now_secs()) {
        relay.devices.audit_throttled(
            &format!("unenrolled:{}:{}", who.account, who.user),
            &who.account,
            AuditEvent::new("unenrolled_access").subject(subject_of(who)),
        );
        return Ok(Gate::Grace);
    }
    Err(NOT_ENROLLED.into())
}

/// The device session a request carries.
pub fn session_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(DEVICE_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Tells a client let through by the grace period that enrollment is
/// required, and from when.
pub fn flag_grace(relay: &Relay, headers: &mut HeaderMap) {
    if let Ok(v) = HeaderValue::from_str(&format!(
        "required; enforce-after={}",
        relay.devices.enforce_after()
    )) {
        headers.insert(ENROLLMENT_HEADER, v);
    }
}

fn subject_of(who: &Identity) -> String {
    who.email.clone().unwrap_or_else(|| who.user.clone())
}

/// Records a client reaching `machine` in the accessor's audit log and, for
/// a shared machine, in the owner's (throttled per device and machine).
pub fn record_access(
    relay: &Relay,
    gate: &Gate,
    who: &Identity,
    machine: &MachineRecord,
    role: Role,
) {
    let actor = gate.device().unwrap_or(&who.user).to_owned();
    let mut event = AuditEvent::new("machine_access")
        .device(gate.device())
        .machine(&machine.id)
        .detail(role.as_str());
    if matches!(gate, Gate::Grace) {
        event = event.subject("unenrolled device");
    }
    relay.devices.audit_throttled(
        &format!("access:{}:{actor}:{}", who.account, machine.id),
        &who.account,
        event,
    );
    if matches!(role, Role::Shared | Role::Viewer) {
        relay.devices.audit_throttled(
            &format!("shared:{}:{actor}:{}", machine.owner.id, machine.id),
            &machine.owner.id,
            AuditEvent::new("shared_access")
                .machine(&machine.id)
                .subject(subject_of(who)),
        );
    }
}

/// A device as the API shows it (no key material or code hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceView {
    /// `dev_…`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `pending`, `enrolled`, `expired` or `revoked`.
    pub state: DeviceState,
    /// Registration time.
    pub created_at: u64,
    /// Last enrollment.
    pub enrolled_at: Option<u64>,
    /// Re-verification due at.
    pub enrolled_until: Option<u64>,
    /// Approving device, or the bootstrap kind.
    pub enrolled_by: Option<String>,
    /// Last session.
    pub last_seen: Option<u64>,
    /// Revocation time.
    pub revoked_at: Option<u64>,
    /// When a pending code expires.
    pub code_expires: Option<u64>,
    /// The caller's own device.
    #[serde(default)]
    pub current: bool,
    /// Operating system the device reported.
    #[serde(default)]
    pub platform: String,
    /// A phone (a smaller kind of device, see [`crate::phone`]).
    #[serde(default)]
    pub phone: bool,
    /// What the device states it can host for the pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<crate::phone::Capabilities>,
}

fn view(d: &DeviceRecord, current: Option<&str>) -> DeviceView {
    DeviceView {
        id: d.id.clone(),
        name: d.name.clone(),
        state: d.state(now_secs()),
        created_at: d.created_at,
        enrolled_at: d.enrolled_at,
        enrolled_until: d.enrolled_until,
        enrolled_by: d.enrolled_by.clone(),
        last_seen: d.last_seen,
        revoked_at: d.revoked_at,
        code_expires: d.code_expires.filter(|e| *e > now_secs()),
        current: current == Some(d.id.as_str()),
        platform: d.platform.clone(),
        phone: d.is_phone(),
        capabilities: d.capabilities.clone(),
    }
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/devices", get(list).post(register))
        .route("/v1/devices/session", post(session))
        .route("/v1/devices/approve", post(approve))
        .route("/v1/devices/revoke-phones", post(revoke_phones))
        .route("/v1/devices/{id}", patch(rename).delete(revoke))
        .route("/v1/audit", get(audit))
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({"error": message.into(), "code": code})),
    )
        .into_response()
}

/// Whether the token comes from a sign-in made in the last ten minutes: the
/// proof a person, not just a stolen session, is behind a sensitive call from
/// a phone.
pub(crate) fn stepped_up(who: &Identity) -> bool {
    const FRESH_SECS: u64 = 600;
    who.auth_time
        .is_some_and(|t| now_secs().saturating_sub(t) <= FRESH_SECS && t <= now_secs() + 60)
}

/// What a phone is told when it changes who a computer is shared with, removes or
/// hides one, or adds one, on a sign-in that is not recent.
pub const PHONE_NEEDS_SIGN_IN: &str =
    "changing your computers from a phone needs you to sign in again just now";

/// The account's control plane from a phone: a phone whose sign-in is not recent
/// (a stolen phone with a refreshed token has none) may not reshare, stop sharing,
/// remove or confirm a computer, or mint a pairing code. The same rule as approving
/// or revoking a device. `Ok` for a computer, and for a phone that just signed in.
pub(crate) fn phone_needs_recent_sign_in(
    relay: &Relay,
    who: &Identity,
    device: Option<&str>,
) -> Result<(), Response> {
    let is_phone = device.is_some_and(|d| relay.devices.is_phone(d));
    if is_phone && !stepped_up(who) {
        if let Some(d) = device {
            relay.devices.audit_throttled(
                &format!("mobile_denied:{d}:control_plane"),
                &who.account,
                crate::devices::AuditEvent::new("mobile_denied")
                    .device(Some(d))
                    .detail("control plane without a recent sign-in"),
            );
        }
        return Err(error(
            StatusCode::FORBIDDEN,
            "recent_sign_in_needed",
            PHONE_NEEDS_SIGN_IN,
        ));
    }
    Ok(())
}

/// With phone support on, one account may make only so many enrollment calls
/// a minute (a phone that retries in a loop, or a stolen token trying keys), counted
/// apart for a fresh sign-in and an old one.
fn too_many_calls(relay: &Relay, who: &Identity) -> Option<Response> {
    relay.config.push.as_ref()?;
    // Calls made on a stale sign-in (what a stolen token has) count apart from those on a fresh one,
    // so a thief's flood cannot use up the owner's allowance.
    let wait = relay
        .viewer_limits()
        .device_call(&format!("{}|{}", who.account, stepped_up(who)))
        .err()?;
    let mut r = error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "too many requests from this account; try again in a minute",
    );
    if let Ok(v) = HeaderValue::from_str(&wait.as_secs().saturating_add(1).to_string()) {
        r.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
    }
    Some(r)
}

fn device_error(e: DeviceError) -> Response {
    let (status, code) = match &e {
        DeviceError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid"),
        DeviceError::Proof(_) => (StatusCode::UNAUTHORIZED, "device_proof"),
        DeviceError::Forbidden(_) => (StatusCode::FORBIDDEN, "device_forbidden"),
        DeviceError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        DeviceError::CodeNotFound => (StatusCode::NOT_FOUND, "code_not_found"),
        DeviceError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
        DeviceError::Storage(_) => (StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    };
    error(status, code, e.to_string())
}

/// Logs a device-proof 401 (other device errors are not 401s). `ts` is the
/// proof's timestamp, for the clock skew of a stale proof.
fn log_proof_failure(headers: &HeaderMap, e: &DeviceError, ts: u64) {
    let DeviceError::Proof(message) = e else {
        return;
    };
    let (reason, skew) = match message.as_str() {
        crate::devices::PROOF_STALE => (
            Reason::DeviceProofStale,
            Some(ts as i64 - now_secs() as i64),
        ),
        crate::devices::PROOF_REPLAYED => (Reason::DeviceProofReplayed, None),
        _ => (Reason::DeviceProofBadSignature, None),
    };
    let facts = TokenFacts {
        clock_skew_secs: skew,
        ..TokenFacts::default()
    };
    auth_log::rejected(headers, Route::Devices, reason, Some(&facts));
}

/// The account behind the request (machine tokens are refused).
pub(crate) async fn account(relay: &Relay, headers: &HeaderMap) -> Result<Identity, Response> {
    let Some(oidc) = &relay.config.oidc else {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "account mode is not enabled on this relay",
        ));
    };
    if !relay.config.device_enrollment {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "device enrollment is off on this relay",
        ));
    }
    if bearer(headers, "authorization")
        .is_some_and(|t| t.starts_with(crate::directory::MACHINE_TOKEN_PREFIX))
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "machine_token",
            "computer tokens cannot manage devices",
        ));
    }
    let Some(token) = account_token(headers) else {
        auth_log::rejected(headers, Route::Devices, Reason::MissingToken, None);
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "missing account token",
        ));
    };
    oidc.validate(token).await.map_err(|e| {
        auth_log::rejected(headers, Route::Devices, e.reason, Some(&e.facts));
        error(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            format!("invalid account token: {e}"),
        )
    })
}

/// The caller's enrolled device (required for changes).
pub(crate) fn enrolled(
    relay: &Relay,
    headers: &HeaderMap,
    who: &Identity,
) -> Result<String, Response> {
    session_header(headers)
        .and_then(|t| relay.devices.session_device(&who.account, t))
        .ok_or_else(|| {
            error(
                StatusCode::FORBIDDEN,
                "device_not_enrolled",
                "do this from an enrolled device",
            )
        })
}

#[derive(Deserialize)]
struct RegisterBody {
    public_key: String,
    #[serde(default)]
    name: String,
    /// Operating system the device reports (`macos`, `windows`, `linux`).
    #[serde(default)]
    platform: String,
    /// A stable id of the machine the device runs on (a hash of its
    /// hardware or install identity), so a new key of the same machine
    /// replaces the old record.
    #[serde(default)]
    machine_id: Option<String>,
    ts: u64,
    sig: String,
    #[serde(default)]
    bootstrap: bool,
    /// What the device can host for the pool (a phone: nothing today). Additive
    /// and versioned.
    #[serde(default)]
    capabilities: Option<crate::phone::Capabilities>,
}

async fn register(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<RegisterBody>,
) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Some(r) = too_many_calls(&relay, &who) {
        return r;
    }
    if let Some(c) = &body.capabilities {
        if let Err(message) = c.check() {
            return error(StatusCode::BAD_REQUEST, "invalid_capabilities", message);
        }
    }
    match relay.devices.register(&Registration {
        account: &who.account,
        user: &who.user,
        public_key: &body.public_key,
        name: &body.name,
        platform: &body.platform,
        machine_id: body.machine_id.as_deref(),
        ts: body.ts,
        sig: &body.sig,
        bootstrap: body.bootstrap,
        auth_time: who.auth_time,
        // The validator keeps an email only when the issuer verified it.
        email_verified: who.email.is_some(),
        mfa: who.mfa,
        phone: is_phone_registration(&relay, &who, &body.platform),
        capabilities: body.capabilities.clone(),
    }) {
        Ok(r) => {
            let state = r.device.state(now_secs());
            tracing::info!(account = %crate::ident::tag(&who.account), device = %r.device.id, ?state, superseded = ?r.superseded, "device registered");
            Json(serde_json::json!({
                "device": view(&r.device, Some(&r.device.id)),
                "code": r.code,
                "superseded": r.superseded,
                "enforce_after": relay.devices.enforce_after(),
                "ttl_secs": relay.devices.policy().ttl_secs,
            }))
            .into_response()
        }
        Err(e) => {
            log_proof_failure(&headers, &e, body.ts);
            device_error(e)
        }
    }
}

#[derive(Deserialize)]
struct SessionBody {
    device_id: String,
    ts: u64,
    sig: String,
}

async fn session(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<SessionBody>,
) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Some(r) = too_many_calls(&relay, &who) {
        return r;
    }
    // Off the async workers: opening a session appends to the fsynced
    // session journal (CUA-2323).
    let opened = {
        let relay = relay.clone();
        let account = who.account.clone();
        let (device, sig) = (body.device_id.clone(), body.sig.clone());
        tokio::task::spawn_blocking(move || {
            relay.devices.open_session(&account, &device, body.ts, &sig)
        })
        .await
    };
    let Ok(opened) = opened else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the session could not be opened",
        );
    };
    match opened {
        Ok((token, expires)) => Json(serde_json::json!({
            "session": token,
            "expires_at": expires,
            "device": relay.devices.get(&body.device_id).map(|d| view(&d, Some(&d.id))),
        }))
        .into_response(),
        Err(e) => {
            log_proof_failure(&headers, &e, body.ts);
            device_error(e)
        }
    }
}

#[derive(Deserialize)]
struct ApproveBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
}

async fn approve(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<ApproveBody>,
) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Some(r) = too_many_calls(&relay, &who) {
        return r;
    }
    let approver = match enrolled(&relay, &headers, &who) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match relay.devices.approve(
        &who.account,
        &approver,
        body.code.as_deref(),
        body.device_id.as_deref(),
        stepped_up(&who),
    ) {
        Ok(d) => {
            tracing::info!(account = %crate::ident::tag(&who.account), device = %d.id, approver = %approver, "device approved");
            Json(serde_json::json!({ "device": view(&d, Some(&approver)) })).into_response()
        }
        Err(e) => device_error(e),
    }
}

async fn list(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let current = match gate(&relay, session_header(&headers), &who) {
        Ok(g) => g.device().map(str::to_owned),
        Err(m) => return error(StatusCode::FORBIDDEN, "device_not_enrolled", m),
    };
    let devices: Vec<_> = relay
        .devices
        .list(&who.account)
        .iter()
        .map(|d| view(d, current.as_deref()))
        .collect();
    Json(serde_json::json!({
        "devices": devices,
        "enforce_after": relay.devices.enforce_after(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let actor = match enrolled(&relay, &headers, &who) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match relay.devices.rename(&who.account, &actor, &id, &body.name) {
        Ok(d) => Json(view(&d, Some(&actor))).into_response(),
        Err(e) => device_error(e),
    }
}

async fn revoke(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let actor = match enrolled(&relay, &headers, &who) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match relay
        .devices
        .revoke(&who.account, &actor, &id, stepped_up(&who))
    {
        Ok(d) => {
            // Open streams and outstanding tickets of the revoked device's
            // user end too (tickets don't say which device they were minted
            // on; the user's other devices reconnect). No one else is cut.
            if d.is_phone() {
                // Only the phones' streams: the account's computers keep theirs.
                relay.cut_phones(&d.user);
            } else {
                relay.cut_account(&d.user, None);
            }
            // A revoked phone stops getting alerts too (approval prompts
            // included).
            if let Some(push) = &relay.config.push {
                let _ = push.hub.registry.unregister(&who.account, &d.id);
            }
            tracing::info!(account = %crate::ident::tag(&who.account), device = %d.id, "device revoked");
            Json(view(&d, Some(&actor))).into_response()
        }
        Err(e) => device_error(e),
    }
}

/// The kill switch for the account's phones: every phone is revoked at once
/// (its sessions, its viewer tickets, its alerts). Asked from a computer, or
/// from a phone with a sign-in made just now, like any revoke of another
/// device.
async fn revoke_phones(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let actor = match enrolled(&relay, &headers, &who) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut revoked = Vec::new();
    for d in relay.devices.list(&who.account) {
        if !d.is_phone() || d.revoked_at.is_some() {
            continue;
        }
        match relay
            .devices
            .revoke(&who.account, &actor, &d.id, stepped_up(&who))
        {
            Ok(d) => {
                if d.is_phone() {
                    // Only the phones' streams: the account's computers keep theirs.
                    relay.cut_phones(&d.user);
                } else {
                    relay.cut_account(&d.user, None);
                }
                if let Some(push) = &relay.config.push {
                    let _ = push.hub.registry.unregister(&who.account, &d.id);
                }
                revoked.push(d.id);
            }
            // A phone that asks without a fresh sign-in stops at the first
            // refusal, with the reason; the ones already revoked stay so.
            Err(e) => return device_error(e),
        }
    }
    tracing::info!(account = %crate::ident::tag(&who.account), phones = revoked.len(), "phones revoked");
    Json(serde_json::json!({ "revoked": revoked })).into_response()
}

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default)]
    limit: Option<usize>,
    /// `jsonl` (S9): the full retained chain, one entry per line
    /// (`application/x-ndjson`), for export to a SIEM or backup instead of
    /// the default single JSON object. The default format is unchanged.
    #[serde(default)]
    format: Option<String>,
}

/// Largest export a single request returns (S9): the account's whole
/// retained chain, since [`crate::devices::AUDIT_LIMIT`] bounds how much
/// there ever is to export.
const MAX_AUDIT_EXPORT: usize = crate::devices::AUDIT_LIMIT;

async fn audit(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Err(m) = gate(&relay, session_header(&headers), &who) {
        return error(StatusCode::FORBIDDEN, "device_not_enrolled", m);
    }
    let jsonl = q.format.as_deref() == Some("jsonl");
    let limit = q
        .limit
        .unwrap_or(if jsonl { MAX_AUDIT_EXPORT } else { 200 })
        .min(MAX_AUDIT_EXPORT);
    let events = relay.devices.audit_log(&who.account, limit);
    if jsonl {
        let mut body = String::new();
        for e in &events {
            body.push_str(&serde_json::to_string(e).unwrap_or_default());
            body.push('\n');
        }
        return (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/x-ndjson; charset=utf-8",
            )],
            body,
        )
            .into_response();
    }
    Json(serde_json::json!({ "events": events })).into_response()
}
