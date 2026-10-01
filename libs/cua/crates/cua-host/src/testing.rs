//! An in-process fake of the cua-relay machine directory (feature
//! `testing`): the §1 HTTP API with fixed account tokens, for tests of the
//! host flow, the Spaces relay provider and the CLI. It does not tunnel.
//!
//! Client devices follow the relay's protocol (signed registration and
//! session proofs, enrollment by a fresh sign-in, one-time codes, approval
//! from an enrolled device, a new key of the same machine replacing the
//! old record; [`FakeRelay::legacy_enrollment`] plays a relay from before
//! sign-in enrollment for every device and re-keying); with
//! [`FakeRelay::require_devices`] the machine API refuses account calls
//! without an enrolled device's session, as the real relay does after its
//! grace period. Requests to `/m/<id>/…` are recorded and answered 502.

use crate::relay::{ConnectedClient, Identity, Machine};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Record {
    machine: Machine,
    token: String,
}

#[derive(Clone, Debug)]
struct FakeDevice {
    account: String,
    public_key: Vec<u8>,
    name: String,
    state: crate::relay::DeviceState,
    code: Option<String>,
    platform: String,
    created_at: u64,
    enrolled_until: Option<u64>,
    last_seen: Option<u64>,
    machine: Option<String>,
    superseded_by: Option<String>,
}

/// Machine access is audited at most this often per device and machine.
const ACCESS_AUDIT_SECS: u64 = 600;

/// How long the fake relay enrolls a device (the real relay's default).
const DEVICE_TTL_SECS: u64 = 30 * 86_400;

#[derive(Default)]
struct State_ {
    /// account token → identity
    accounts: BTreeMap<String, Identity>,
    machines: BTreeMap<String, Record>,
    public_url: String,
    devices: BTreeMap<String, FakeDevice>,
    /// session token → (account, device)
    sessions: BTreeMap<String, (String, String)>,
    require_devices: bool,
    /// Accounts whose token counts as a fresh interactive sign-in.
    fresh: std::collections::BTreeSet<String>,
    audit: Vec<(String, serde_json::Value)>,
    /// `(path, device session header)` of every `/m/…` request.
    proxied: Vec<(String, Option<String>)>,
    /// End of the grace period the device list reports.
    enforce_after: u64,
    /// Enrollment rules of a relay before sign-in enrollment for every
    /// device and re-keying.
    legacy: bool,
}

impl State_ {
    /// The enrolled device behind the request's session, if any.
    fn device_of(&self, headers: &HeaderMap, who: &Identity) -> Option<String> {
        let token = headers
            .get(crate::relay::DEVICE_SESSION_HEADER)
            .and_then(|v| v.to_str().ok())?;
        let (account, device) = self.sessions.get(token)?;
        (account == &who.id
            && self.devices.get(device).map(|d| d.state)
                == Some(crate::relay::DeviceState::Enrolled))
        .then(|| device.clone())
    }

    /// Refuses an account call without an enrolled device when required.
    #[allow(clippy::result_large_err)]
    fn gate(&self, headers: &HeaderMap) -> Result<(), Response> {
        if !self.require_devices {
            return Ok(());
        }
        match caller(self, headers) {
            Some(Caller::Account(who)) if self.device_of(headers, &who).is_none() => Err(err(
                StatusCode::FORBIDDEN,
                "this device is not enrolled for your cua.ai account",
            )),
            _ => Ok(()),
        }
    }
}

fn verify_proof(public_key: &[u8], message: &str, sig: &str) -> bool {
    use base64::Engine as _;
    let Ok(sig) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig) else {
        return false;
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, public_key)
        .verify(message.as_bytes(), &sig)
        .is_ok()
}

fn device_view(id: &str, d: &FakeDevice, current: Option<&str>) -> crate::relay::DeviceView {
    crate::relay::DeviceView {
        id: id.into(),
        name: d.name.clone(),
        state: d.state,
        current: current == Some(id),
        platform: d.platform.clone(),
        created_at: d.created_at,
        enrolled_until: d.enrolled_until,
        last_seen: d.last_seen,
        ..Default::default()
    }
}

/// Enrolls `d` now (bootstrap or approval).
fn enroll_now(d: &mut FakeDevice) {
    d.state = crate::relay::DeviceState::Enrolled;
    d.code = None;
    d.enrolled_until = Some(now_secs() + DEVICE_TTL_SECS);
}

/// After `id` enrolled: revokes `account`'s other live devices of the same
/// machine as superseded (not on a legacy relay), ending their sessions.
fn supersede(st: &mut State_, account: &str, id: &str) -> Vec<String> {
    let machine = st.devices.get(id).and_then(|d| d.machine.clone());
    let Some(machine) = machine.filter(|_| !st.legacy) else {
        return Vec::new();
    };
    let mut replaced = Vec::new();
    for (other, d) in st.devices.iter_mut() {
        if other != id
            && d.account == account
            && d.state != crate::relay::DeviceState::Revoked
            && d.machine.as_deref() == Some(machine.as_str())
        {
            d.state = crate::relay::DeviceState::Revoked;
            d.code = None;
            d.superseded_by = Some(id.to_string());
            replaced.push(other.clone());
        }
    }
    st.sessions.retain(|_, (_, dev)| !replaced.contains(dev));
    for old in &replaced {
        st.audit.push((
            account.to_string(),
            serde_json::json!({"ts": now_secs(), "kind": "device_rekeyed", "device": id, "subject": old}),
        ));
    }
    replaced
}

/// A running fake relay.
#[derive(Clone)]
pub struct FakeRelay {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    state: Arc<Mutex<State_>>,
}

fn err(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-cua-relay-authorization")
        .or_else(|| headers.get("authorization"))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

enum Caller {
    Account(Identity),
    Machine(String),
}

fn caller(state: &State_, headers: &HeaderMap) -> Option<Caller> {
    let token = bearer(headers)?;
    if let Some(id) = state.accounts.get(&token) {
        return Some(Caller::Account(id.clone()));
    }
    state
        .machines
        .values()
        .find(|r| r.token == token)
        .map(|r| Caller::Machine(r.machine.id.clone()))
}

fn view(record: &Record, who: &Identity) -> Option<Machine> {
    let mut m = record.machine.clone();
    if m.owner.id == who.id {
        m.role = "owner".into();
        return Some(m);
    }
    let listed = |list: &[String]| {
        list.iter().any(|a| {
            a == &who.id
                || who
                    .email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(a))
        })
    };
    let role = if listed(&m.viewers) {
        "viewer"
    } else if listed(&m.allow) {
        "shared"
    } else {
        return None;
    };
    m.role = role.into();
    m.allow = vec![];
    m.viewers = vec![];
    Some(m)
}

type S = Arc<Mutex<State_>>;

async fn register(
    State(s): State<S>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let Some(Caller::Account(who)) = caller(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "account token required");
    };
    let id = body["id"].as_str().unwrap_or_default().to_string();
    if id.len() < 8 {
        return err(StatusCode::BAD_REQUEST, "invalid machine id");
    }
    if let Some(existing) = st.machines.get(&id)
        && existing.machine.owner.id != who.id
    {
        return err(StatusCode::CONFLICT, "machine id owned by another account");
    }
    let allow: Vec<String> = body["allow"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // A Space a host provides names its host: the caller must be able to
    // use that host (owner or editor), as on the real relay.
    let host = body["host"].as_str().map(str::to_string);
    if let Some(h) = &host {
        match st.machines.get(h).and_then(|r| view(r, &who)) {
            Some(v) if v.role == "owner" || v.role == "shared" => {}
            _ => {
                return err(
                    StatusCode::FORBIDDEN,
                    "not allowed to add Spaces to that host",
                );
            }
        }
    }
    let token = format!("cmt_{:032x}", rand::random::<u128>());
    let machine = Machine {
        id: id.clone(),
        name: body["name"].as_str().unwrap_or_default().to_string(),
        owner: who.clone(),
        role: "owner".into(),
        online: false,
        sharing: true,
        version: String::new(),
        connected_at: None,
        url: format!("{}/m/{id}", st.public_url),
        allow,
        viewers: vec![],
        clients: vec![],
        host,
        // This mock never asks for a device signature or MFA, so a
        // registration here has nothing to vouch for either way; default to
        // confirmed, matching behavior before S5 added the field.
        confirmed: true,
        meta: body["meta"]
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
    };
    st.machines.insert(
        id,
        Record {
            machine: machine.clone(),
            token: token.clone(),
        },
    );
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "machine": machine,
            "machine_token": token,
            "jwks": fake_jwks(),
        })),
    )
        .into_response()
}

async fn list(State(s): State<S>, headers: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    if let Err(r) = st.gate(&headers) {
        return r;
    }
    let Some(Caller::Account(who)) = caller(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "account token required");
    };
    let machines: Vec<Machine> = st.machines.values().filter_map(|r| view(r, &who)).collect();
    Json(serde_json::json!({ "machines": machines })).into_response()
}

#[allow(clippy::result_large_err)]
fn authorize<'a>(
    st: &'a State_,
    headers: &HeaderMap,
    id: &str,
    owner_only: bool,
) -> Result<(Machine, &'a Record), Response> {
    st.gate(headers)?;
    let Some(record) = st.machines.get(id) else {
        return Err(err(StatusCode::NOT_FOUND, "no such machine"));
    };
    match caller(st, headers) {
        Some(Caller::Machine(m)) if m == id => {
            let mut v = record.machine.clone();
            v.role = "owner".into();
            Ok((v, record))
        }
        Some(Caller::Account(who)) => match view(record, &who) {
            Some(v) if !owner_only || v.role == "owner" => Ok((v, record)),
            Some(_) => Err(err(StatusCode::FORBIDDEN, "owner only")),
            None => Err(err(StatusCode::NOT_FOUND, "no such machine")),
        },
        _ => Err(err(StatusCode::UNAUTHORIZED, "credential required")),
    }
}

/// The fake relay's signing keys: what a registration returns and
/// `/.well-known/jwks.json` serves.
fn fake_jwks() -> serde_json::Value {
    serde_json::json!({"keys": [{"kty": "OKP", "crv": "Ed25519", "x": "AA", "kid": "fake", "alg": "EdDSA", "use": "sig"}]})
}

async fn get_one(State(s): State<S>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let st = s.lock().unwrap();
    match authorize(&st, &headers, &id, false) {
        Ok((m, _)) => Json(m).into_response(),
        Err(r) => r,
    }
}

async fn patch(
    State(s): State<S>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    if let Err(r) = authorize(&st, &headers, &id, true) {
        return r;
    }
    if matches!(caller(&st, &headers), Some(Caller::Machine(_))) {
        return err(StatusCode::FORBIDDEN, "owner only");
    }
    let rec = st.machines.get_mut(&id).unwrap();
    if let Some(n) = body["name"].as_str() {
        rec.machine.name = n.into();
    }
    if let Some(a) = body["allow"].as_array() {
        rec.machine.allow = a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    if let Some(a) = body["viewers"].as_array() {
        rec.machine.viewers = a
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_ascii_lowercase()))
            .collect();
    }
    if let Some(sh) = body["sharing"].as_bool() {
        rec.machine.sharing = sh;
        if !sh {
            rec.machine.clients.clear();
        }
    }
    Json(rec.machine.clone()).into_response()
}

async fn delete(State(s): State<S>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let mut st = s.lock().unwrap();
    if let Err(r) = authorize(&st, &headers, &id, true) {
        return r;
    }
    st.machines.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}

async fn sharing(s: S, headers: HeaderMap, id: String, on: bool) -> Response {
    let mut st = s.lock().unwrap();
    if let Err(r) = authorize(&st, &headers, &id, true) {
        return r;
    }
    let rec = st.machines.get_mut(&id).unwrap();
    rec.machine.sharing = on;
    if !on {
        rec.machine.clients.clear();
    }
    let mut m = rec.machine.clone();
    m.role = "owner".into();
    Json(m).into_response()
}

async fn stop(State(s): State<S>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    sharing(s, headers, id, false).await
}

async fn start(State(s): State<S>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    sharing(s, headers, id, true).await
}

#[allow(clippy::result_large_err)]
fn account_of(st: &State_, headers: &HeaderMap) -> Result<Identity, Response> {
    match caller(st, headers) {
        Some(Caller::Account(who)) => Ok(who),
        _ => Err(err(StatusCode::UNAUTHORIZED, "account token required")),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn device_register(
    State(s): State<S>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    use crate::relay::DeviceState;
    use base64::Engine as _;
    let mut st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Ok(public_key) = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body["public_key"].as_str().unwrap_or_default())
    else {
        return err(StatusCode::BAD_REQUEST, "public_key");
    };
    let digest = ring::digest::digest(&ring::digest::SHA256, &public_key);
    let id = format!("dev_{}", &hex::encode(digest.as_ref())[..24]);
    let ts = body["ts"].as_u64().unwrap_or_default();
    if ts.abs_diff(now_secs()) > 120
        || !verify_proof(
            &public_key,
            &crate::device::register_message(&id, ts),
            body["sig"].as_str().unwrap_or_default(),
        )
    {
        return err(StatusCode::UNAUTHORIZED, "invalid device signature");
    }
    let has_enrolled = st
        .devices
        .values()
        .any(|d| d.account == who.id && d.state == DeviceState::Enrolled);
    let fresh = st.fresh.contains(&who.id);
    let asked = body["bootstrap"].as_bool().unwrap_or(false);
    let bootstrap = asked
        && if st.legacy {
            !has_enrolled && (fresh || !st.require_devices)
        } else {
            // A fresh sign-in enrolls any device of an account with a
            // (verified) email; the grace period only the first.
            (fresh && (who.email.is_some() || !has_enrolled))
                || (!has_enrolled && !st.require_devices)
        };
    let machine = body["machine_id"]
        .as_str()
        .filter(|m| !m.trim().is_empty() && !st.legacy)
        .map(|m| m.trim().to_string());
    let name = body["name"].as_str().unwrap_or_default().to_string();
    let legacy = st.legacy;
    let entry = st.devices.entry(id.clone()).or_insert(FakeDevice {
        account: who.id.clone(),
        public_key,
        name: if name.is_empty() {
            "Unnamed device".into()
        } else {
            name.clone()
        },
        state: DeviceState::Pending,
        code: None,
        platform: body["platform"].as_str().unwrap_or_default().to_string(),
        created_at: now_secs(),
        enrolled_until: None,
        last_seen: None,
        machine: None,
        superseded_by: None,
    });
    if entry.account != who.id {
        return err(StatusCode::CONFLICT, "device key of another account");
    }
    if entry.state == DeviceState::Revoked {
        return err(
            StatusCode::FORBIDDEN,
            &match &entry.superseded_by {
                Some(by) => format!(
                    "this device key was replaced by {by} on the same machine; create a new device key to enroll again"
                ),
                None => "this device was revoked; create a new device key to enroll again".into(),
            },
        );
    }
    if !name.is_empty() {
        entry.name = name;
    }
    if machine.is_some() {
        entry.machine = machine;
    }
    let mut code = None;
    let mut enrolled_now = false;
    if entry.state != DeviceState::Enrolled || (bootstrap && fresh && !legacy) {
        if bootstrap {
            enroll_now(entry);
            enrolled_now = true;
        } else {
            let c = format!(
                "{:04X}-{:04X}",
                rand::random::<u16>(),
                rand::random::<u16>()
            );
            entry.code = Some(c.clone());
            code = Some(c);
        }
    }
    let device = device_view(&id, entry, Some(&id));
    let enrolled = device.state == DeviceState::Enrolled;
    st.audit.push((
        who.id.clone(),
        serde_json::json!({"ts": now_secs(), "kind": if enrolled {"device_enrolled"} else {"device_registered"}, "device": id}),
    ));
    let superseded = if enrolled_now {
        supersede(&mut st, &who.id, &id)
    } else {
        Vec::new()
    };
    let mut reply = serde_json::json!({
        "device": device, "code": code, "enforce_after": 0, "ttl_secs": 30 * 86_400,
    });
    if !st.legacy {
        reply["superseded"] = serde_json::json!(superseded);
    }
    Json(reply).into_response()
}

async fn device_session(
    State(s): State<S>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let id = body["device_id"].as_str().unwrap_or_default().to_string();
    let Some(device) = st.devices.get(&id).filter(|d| d.account == who.id).cloned() else {
        return err(StatusCode::NOT_FOUND, "no such device");
    };
    let ts = body["ts"].as_u64().unwrap_or_default();
    if ts.abs_diff(now_secs()) > 120
        || !verify_proof(
            &device.public_key,
            &crate::device::session_message(&id, ts),
            body["sig"].as_str().unwrap_or_default(),
        )
    {
        return err(StatusCode::UNAUTHORIZED, "invalid device signature");
    }
    if device.state != crate::relay::DeviceState::Enrolled {
        return err(
            StatusCode::FORBIDDEN,
            "this device is waiting for approval from an enrolled device",
        );
    }
    let token = format!("cds_{:032x}", rand::random::<u128>());
    if let Some(d) = st.devices.get_mut(&id) {
        d.last_seen = Some(now_secs());
    }
    st.sessions.insert(token.clone(), (who.id.clone(), id));
    Json(serde_json::json!({"session": token, "expires_at": now_secs() + 3600})).into_response()
}

async fn device_approve(
    State(s): State<S>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Some(approver) = st.device_of(&headers, &who) else {
        return err(StatusCode::FORBIDDEN, "do this from an enrolled device");
    };
    let mut code = body["code"].as_str().map(|c| c.trim().to_ascii_uppercase());
    let mut device_id = body["device_id"].as_str().map(str::to_string);
    // A `dev_…` code is a device id (not on a legacy relay).
    if !st.legacy
        && device_id.is_none()
        && let Some(id) = body["code"]
            .as_str()
            .map(str::trim)
            .filter(|c| c.starts_with(crate::relay::DEVICE_ID_PREFIX))
    {
        device_id = Some(id.to_string());
        code = None;
    }
    let target = st
        .devices
        .iter()
        .find(|(id, d)| {
            d.account == who.id
                && (code.is_some() && d.code == code || device_id.as_deref() == Some(id.as_str()))
        })
        .map(|(id, _)| id.clone());
    let Some(target) = target.filter(|t| t != &approver) else {
        return err(
            StatusCode::NOT_FOUND,
            if code.is_some() && !st.legacy {
                "no device is waiting with this code (codes expire after 10 minutes); approve by id instead"
            } else {
                "no such device"
            },
        );
    };
    let d = st.devices.get_mut(&target).unwrap();
    enroll_now(d);
    let view = device_view(&target, d, Some(&approver));
    st.audit.push((
        who.id.clone(),
        serde_json::json!({"ts": now_secs(), "kind": "device_enrolled", "device": target, "subject": approver}),
    ));
    supersede(&mut st, &who.id, &target);
    Json(serde_json::json!({ "device": view })).into_response()
}

async fn device_list(State(s): State<S>, headers: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let current = st.device_of(&headers, &who);
    let devices: Vec<_> = st
        .devices
        .iter()
        .filter(|(_, d)| d.account == who.id && d.superseded_by.is_none())
        .map(|(id, d)| device_view(id, d, current.as_deref()))
        .collect();
    Json(serde_json::json!({ "devices": devices, "enforce_after": st.enforce_after }))
        .into_response()
}

async fn device_rename(
    State(s): State<S>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Some(actor) = st.device_of(&headers, &who) else {
        return err(StatusCode::FORBIDDEN, "do this from an enrolled device");
    };
    let Some(d) = st.devices.get_mut(&id).filter(|d| d.account == who.id) else {
        return err(StatusCode::NOT_FOUND, "no such device");
    };
    d.name = body["name"].as_str().unwrap_or_default().to_string();
    Json(device_view(&id, d, Some(&actor))).into_response()
}

async fn device_revoke(State(s): State<S>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let mut st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Some(actor) = st.device_of(&headers, &who) else {
        return err(StatusCode::FORBIDDEN, "do this from an enrolled device");
    };
    let Some(d) = st.devices.get_mut(&id).filter(|d| d.account == who.id) else {
        return err(StatusCode::NOT_FOUND, "no such device");
    };
    d.state = crate::relay::DeviceState::Revoked;
    let view = device_view(&id, d, Some(&actor));
    st.sessions.retain(|_, (_, dev)| dev != &id);
    st.audit.push((
        who.id.clone(),
        serde_json::json!({"ts": now_secs(), "kind": "device_revoked", "device": actor, "subject": id}),
    ));
    Json(view).into_response()
}

async fn audit(State(s): State<S>, headers: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    let who = match account_of(&st, &headers) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let events: Vec<_> = st
        .audit
        .iter()
        .filter(|(a, _)| a == &who.id)
        .map(|(_, e)| e.clone())
        .collect();
    Json(serde_json::json!({ "events": events })).into_response()
}

async fn proxied(State(s): State<S>, uri: axum::http::Uri, headers: HeaderMap) -> Response {
    let session = headers
        .get(crate::relay::DEVICE_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut st = s.lock().unwrap();
    // The account's access log, as the real relay keeps it.
    let machine = uri
        .path()
        .strip_prefix("/m/")
        .and_then(|rest| rest.split('/').next())
        .filter(|id| st.machines.contains_key(*id))
        .map(str::to_string);
    if let (Some(machine), Some(Caller::Account(who))) = (machine, caller(&st, &headers)) {
        let device = st.device_of(&headers, &who);
        let event = match &device {
            Some(d) => serde_json::json!({
                "ts": now_secs(), "kind": "machine_access", "device": d, "machine": machine, "detail": "owner",
            }),
            None => serde_json::json!({
                "ts": now_secs(), "kind": "machine_access", "machine": machine, "subject": "unenrolled device",
            }),
        };
        // Throttled per account, device and machine, as the real relay does.
        let repeat = st.audit.iter().rev().take(50).any(|(a, e)| {
            a == &who.id
                && e["kind"] == "machine_access"
                && e["machine"] == event["machine"]
                && e["device"] == event["device"]
                && now_secs().saturating_sub(e["ts"].as_u64().unwrap_or(0)) < ACCESS_AUDIT_SECS
        });
        if !repeat {
            st.audit.push((who.id.clone(), event));
        }
    }
    st.proxied.push((uri.path().to_string(), session));
    err(StatusCode::BAD_GATEWAY, "the fake relay does not tunnel")
}

impl FakeRelay {
    /// Starts on an ephemeral loopback port.
    pub async fn start() -> Self {
        Self::start_on(0).await
    }

    /// Starts on loopback `port` (0 = ephemeral).
    pub async fn start_on(port: u16) -> Self {
        let state: S = Arc::default();
        let app = Router::new()
            .route(
                "/v1/info",
                get(|State(s): State<S>| async move {
                    Json(serde_json::json!({
                        "public_url": s.lock().unwrap().public_url,
                        "issuer": "fake-relay",
                        "account_auth": true,
                        "static_tokens": false,
                    }))
                }),
            )
            .route(
                "/.well-known/jwks.json",
                get(|| async { Json(fake_jwks()) }),
            )
            .route("/v1/machines", get(list).post(register))
            .route(
                "/v1/machines/{id}",
                get(get_one).patch(patch).delete(delete),
            )
            .route("/v1/machines/{id}/stop-sharing", post(stop))
            .route("/v1/machines/{id}/start-sharing", post(start))
            .route("/v1/devices", get(device_list).post(device_register))
            .route("/v1/devices/session", post(device_session))
            .route("/v1/devices/approve", post(device_approve))
            .route(
                "/v1/devices/{id}",
                axum::routing::patch(device_rename).delete(device_revoke),
            )
            .route("/v1/audit", get(audit))
            .fallback(proxied)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("bind fake relay");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        state.lock().unwrap().public_url = url.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { url, state }
    }

    /// Accepts `token` as the account `id` (with `email`).
    pub fn add_account(&self, token: &str, id: &str, email: Option<&str>) {
        self.state.lock().unwrap().accounts.insert(
            token.into(),
            Identity {
                id: id.into(),
                email: email.map(str::to_string),
                name: None,
            },
        );
    }

    /// Forgets every open device session, as a real relay restart does:
    /// sessions live only in the running process (see
    /// `cua_relay::devices::DeviceStore`), so a redeploy or crash drops
    /// every one of them at once while devices, machines and audit history
    /// (all persisted) are unaffected. Lets a test simulate a device whose
    /// session was opened moments before a restart and still looks locally
    /// unexpired to [`crate::device::DeviceAuth`].
    pub fn forget_sessions(&self) {
        self.state.lock().unwrap().sessions.clear();
    }

    /// Marks a machine online (as if its driver joined).
    pub fn set_online(&self, id: &str, online: bool, version: &str) {
        if let Some(r) = self.state.lock().unwrap().machines.get_mut(id) {
            r.machine.online = online;
            r.machine.version = version.into();
            r.machine.connected_at = online.then_some(1_700_000_000);
        }
    }

    /// Adds a connected client to a machine.
    pub fn add_client(&self, id: &str, client: ConnectedClient) {
        if let Some(r) = self.state.lock().unwrap().machines.get_mut(id) {
            r.machine.clients.push(client);
        }
    }

    /// The machine as the relay stores it.
    pub fn machine(&self, id: &str) -> Option<Machine> {
        self.state
            .lock()
            .unwrap()
            .machines
            .get(id)
            .map(|r| r.machine.clone())
    }

    /// Refuses account calls to the machine API without an enrolled
    /// device's session (the real relay after its grace period).
    pub fn require_devices(&self, on: bool) {
        self.state.lock().unwrap().require_devices = on;
    }

    /// Sets the end of the grace period the device list reports.
    pub fn set_enforce_after(&self, at: u64) {
        self.state.lock().unwrap().enforce_after = at;
    }

    /// Treats the account `id`'s tokens as a fresh interactive sign-in (a
    /// device registering then enrolls without an approval).
    pub fn fresh_sign_in(&self, id: &str) {
        self.state.lock().unwrap().fresh.insert(id.into());
    }

    /// Treats the account `id`'s tokens as a long-lived session again.
    pub fn stale_sign_in(&self, id: &str) {
        self.state.lock().unwrap().fresh.remove(id);
    }

    /// Plays a relay from before sign-in enrollment for every device and
    /// re-keying: a fresh sign-in enrolls only an account's first device,
    /// machine ids are ignored, and a `dev_…` code is just an unknown code.
    pub fn legacy_enrollment(&self, on: bool) {
        self.state.lock().unwrap().legacy = on;
    }

    /// The device that replaced device `id`, if one did.
    pub fn superseded_by(&self, id: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .devices
            .get(id)
            .and_then(|d| d.superseded_by.clone())
    }

    /// The state of device `id`.
    pub fn device_state(&self, id: &str) -> Option<crate::relay::DeviceState> {
        self.state.lock().unwrap().devices.get(id).map(|d| d.state)
    }

    /// Number of devices registered for the account `id`.
    pub fn device_count(&self, account: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .devices
            .values()
            .filter(|d| d.account == account)
            .count()
    }

    /// `(path, device session)` of the `/m/…` requests received so far.
    pub fn proxied(&self) -> Vec<(String, Option<String>)> {
        self.state.lock().unwrap().proxied.clone()
    }

    /// The current machine token of `id`.
    pub fn machine_token(&self, id: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .machines
            .get(id)
            .map(|r| r.token.clone())
    }
}
