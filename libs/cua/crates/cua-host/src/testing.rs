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
//! grace period. Requests to `/m/<id>/…` are recorded; a machine that
//! stopped sharing refuses them as the real relay does, gRPC-Web requests
//! to a machine given a [`FakeRelay::tunnel`] are forwarded to it, and the
//! rest are answered 502.

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
    /// Statuses the next machine registrations answer with, in order
    /// (a relay mid-deploy, a proxy in front of it).
    fail_register: std::collections::VecDeque<u16>,
    /// Statuses the next machine reads (GET) answer with, in order.
    fail_reads: std::collections::VecDeque<u16>,
    /// Every machine read answers 503, as the proxy in front of the
    /// relay does while it has no backend.
    reads_down: bool,
    /// `/m/<id>` of a machine listed offline answers 502 at once, as the
    /// relay does once past its start-up grace.
    refuse_offline: bool,
    /// A relay that predates meta patches: the key is ignored.
    ignore_meta_patches: bool,
    /// A relay (or its control plane) that refuses to change who a computer
    /// is shared with, with this code (`share_outside_team`).
    refuse_shares: Option<String>,
    /// Machine registrations received (including refused ones).
    register_calls: usize,
    /// machine id → `(path prefix, base URL)` its `/m/<id>/…` gRPC-Web
    /// requests go to (the longest matching prefix wins).
    tunnels: BTreeMap<String, Vec<(String, String)>>,
    /// What `GET /v1/usage` answers; none: 404, as a relay that does not
    /// meter.
    usage: Option<serde_json::Value>,
    /// Pairing codes by canonical code.
    pairing: BTreeMap<String, FakePairing>,
}

#[derive(Clone, Debug)]
struct FakePairing {
    id: String,
    owner: Identity,
    name: Option<String>,
    team: Option<String>,
    created_at: u64,
    expires_at: u64,
    used_at: Option<u64>,
    machine: Option<String>,
    revoked: bool,
}

impl FakePairing {
    fn state(&self) -> &'static str {
        if self.used_at.is_some() {
            "used"
        } else if self.revoked {
            "revoked"
        } else if now_secs() >= self.expires_at {
            "expired"
        } else {
            "active"
        }
    }

    fn view(&self, code: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "id": self.id, "code": code.map(crate::pairing::display_pairing_code),
            "state": self.state(), "name": self.name, "team": self.team,
            "created_at": self.created_at, "expires_at": self.expires_at,
            "used_at": self.used_at, "machine": self.machine,
        })
    }
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
    st.register_calls += 1;
    if let Some(code) = st.fail_register.pop_front() {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        return err(status, "the fake relay failed this registration");
    }
    let id = body["id"].as_str().unwrap_or_default().to_string();
    // A pairing code registers as the account that minted it, once.
    let paired = bearer(&headers).and_then(|t| {
        t.strip_prefix(crate::pairing::PAIRING_TOKEN_PREFIX)
            .map(str::to_string)
    });
    let mut team = None;
    let mut code_name = None;
    let who = match paired {
        Some(code) => {
            let Some(p) = crate::pairing::normalize_pairing_code(&code)
                .and_then(|c| st.pairing.get_mut(&c))
                .filter(|p| p.state() == "active")
            else {
                return err(StatusCode::UNAUTHORIZED, "this pairing code is not valid");
            };
            if id.len() < 8 {
                return err(StatusCode::BAD_REQUEST, "invalid computer id");
            }
            p.used_at = Some(now_secs());
            p.machine = Some(id.clone());
            team = p.team.clone();
            code_name = p.name.clone();
            p.owner.clone()
        }
        None => match caller(&st, &headers) {
            Some(Caller::Account(who)) => who,
            _ => return err(StatusCode::UNAUTHORIZED, "account token required"),
        },
    };
    if id.len() < 8 {
        return err(StatusCode::BAD_REQUEST, "invalid computer id");
    }
    if let Some(existing) = st.machines.get(&id)
        && existing.machine.owner.id != who.id
    {
        return err(StatusCode::CONFLICT, "computer id owned by another account");
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
        name: code_name.unwrap_or_else(|| body["name"].as_str().unwrap_or_default().to_string()),
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
        // Like the real relay once devices are required: only a
        // registration carrying an enrolled device's session is proven.
        // Otherwise (devices not required) there is nothing to vouch for
        // either way, so it is confirmed, as before S5 added the field.
        confirmed: !st.require_devices || st.device_of(&headers, &who).is_some(),
        team: None,
        meta: body["meta"]
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .chain(team.map(|t| ("cua.team".to_string(), t)))
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

async fn pairing_mint(
    State(s): State<S>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    if let Err(r) = st.gate(&headers) {
        return r;
    }
    let Some(Caller::Account(who)) = caller(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "account token required");
    };
    let code: String = (0..12)
        .map(|_| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"[(rand::random::<u32>() % 32) as usize] as char)
        .collect();
    let now = now_secs();
    let p = FakePairing {
        id: format!("pc_{:016x}", rand::random::<u64>()),
        owner: who,
        name: body["name"].as_str().map(str::to_string),
        team: body["team"].as_str().map(str::to_string),
        created_at: now,
        expires_at: now + body["ttl_secs"].as_u64().unwrap_or(1800).clamp(60, 86_400),
        used_at: None,
        machine: None,
        revoked: false,
    };
    let view = p.view(Some(&code));
    st.pairing.insert(code, p);
    (StatusCode::CREATED, Json(view)).into_response()
}

async fn pairing_list(State(s): State<S>, headers: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    if let Err(r) = st.gate(&headers) {
        return r;
    }
    let Some(Caller::Account(who)) = caller(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "account token required");
    };
    let mut codes: Vec<&FakePairing> = st
        .pairing
        .values()
        .filter(|p| p.owner.id == who.id)
        .collect();
    codes.sort_by_key(|p| std::cmp::Reverse(p.created_at));
    let codes: Vec<_> = codes.into_iter().map(|p| p.view(None)).collect();
    Json(serde_json::json!({ "codes": codes })).into_response()
}

async fn pairing_revoke(
    State(s): State<S>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut st = s.lock().unwrap();
    if let Err(r) = st.gate(&headers) {
        return r;
    }
    let Some(Caller::Account(who)) = caller(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "account token required");
    };
    match st
        .pairing
        .values_mut()
        .find(|p| p.id == id && p.owner.id == who.id)
    {
        Some(p) => {
            if p.state() == "active" {
                p.revoked = true;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        None => err(StatusCode::NOT_FOUND, "no such pairing code"),
    }
}

/// A machine read the fake relay was told to fail (`fail_reads`,
/// `set_reads_down`), as the proxy in front of a relay answers.
fn failed_read(st: &mut State_) -> Option<Response> {
    let code = st.fail_reads.pop_front().or(st.reads_down.then_some(503))?;
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    Some((status, "no available server").into_response())
}

async fn list(State(s): State<S>, headers: HeaderMap) -> Response {
    let mut st = s.lock().unwrap();
    if let Some(r) = failed_read(&mut st) {
        return r;
    }
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
        return Err(err(StatusCode::NOT_FOUND, "no such computer"));
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
            None => Err(err(StatusCode::NOT_FOUND, "no such computer")),
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
    let mut st = s.lock().unwrap();
    if let Some(r) = failed_read(&mut st) {
        return r;
    }
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
    if let Some(code) = st.refuse_shares.clone()
        && (body["allow"].is_array() || body["viewers"].is_array())
    {
        return err(StatusCode::FORBIDDEN, &code);
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
    let ignore_meta = st.ignore_meta_patches;
    let rec = st.machines.get_mut(&id).unwrap();
    if let Some(meta) = body["meta"].as_object().filter(|_| !ignore_meta) {
        for (k, v) in meta {
            match v.as_str() {
                Some("") | None => {
                    rec.machine.meta.remove(k);
                }
                Some(v) => {
                    rec.machine.meta.insert(k.clone(), v.to_string());
                }
            }
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
            // As cua-relay's default policy: a fresh sign-in enrolls any
            // device of the account (verified email or not); the grace
            // period only the first.
            fresh || (!has_enrolled && !st.require_devices)
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
                    "this device key was replaced by {by} on the same computer; create a new device key to enroll again"
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
    if device.state == crate::relay::DeviceState::Revoked {
        return err(StatusCode::FORBIDDEN, "this device was revoked");
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

async fn usage(State(s): State<S>, headers: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    let Some(usage) = st.usage.clone() else {
        return err(StatusCode::NOT_FOUND, "not found");
    };
    match caller(&st, &headers) {
        Some(Caller::Account(_)) => Json(usage).into_response(),
        Some(Caller::Machine(_)) => err(StatusCode::FORBIDDEN, "account token required"),
        None => err(StatusCode::UNAUTHORIZED, "account token required"),
    }
}

async fn proxied(
    State(s): State<S>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    payload: axum::body::Bytes,
) -> Response {
    let (target, refused) = proxied_record(&s, &uri, &headers);
    if let Some(r) = refused {
        return r;
    }
    {
        let st = s.lock().unwrap();
        let offline = uri
            .path()
            .strip_prefix("/m/")
            .and_then(|rest| rest.split('/').next())
            .and_then(|id| st.machines.get(id))
            .is_some_and(|r| !r.machine.online);
        if st.refuse_offline && offline {
            return err(StatusCode::BAD_GATEWAY, "the computer is not connected");
        }
    }
    if let Some(target) = target
        && headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/grpc-web"))
    {
        return forward(&target, method, &uri, &headers, payload).await;
    }
    err(StatusCode::BAD_GATEWAY, "the fake relay does not tunnel")
}

/// Records a `/m/…` request; the machine's tunnel target, or the relay's
/// refusal.
fn proxied_record(
    s: &S,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
) -> (Option<String>, Option<Response>) {
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
    if let (Some(machine), Some(Caller::Account(who))) = (machine, caller(&st, headers)) {
        let device = st.device_of(headers, &who);
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
    let id = uri
        .path()
        .strip_prefix("/m/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_string();
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_health = uri.path().ends_with("/health");
    // As the real relay: a machine that stopped sharing refuses everything
    // but its health check (a gRPC caller gets a gRPC status).
    if st.machines.get(&id).is_some_and(|r| !r.machine.sharing) && !is_health {
        let message = "the host stopped sharing this computer";
        if content_type.starts_with("application/grpc") {
            let r = Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, content_type)
                .header("grpc-status", "7")
                .header("grpc-message", message)
                .body(axum::body::Body::empty())
                .expect("refusal");
            return (None, Some(r));
        }
        return (None, Some((StatusCode::FORBIDDEN, message).into_response()));
    }
    let rest = uri.path().strip_prefix(&format!("/m/{id}")).unwrap_or("");
    let target = st.tunnels.get(&id).and_then(|routes| {
        routes
            .iter()
            .filter(|(prefix, _)| rest.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, target)| target.clone())
    });
    (target, None)
}

/// One gRPC-Web request passed through to `target`, without the
/// `/m/<id>` prefix.
async fn forward(
    target: &str,
    method: axum::http::Method,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    payload: axum::body::Bytes,
) -> Response {
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let path = path
        .strip_prefix("/m/")
        .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or(path);
    let url = format!("{}{path}", target.trim_end_matches('/'));
    // One client for every forward: a client per request read the system
    // proxy settings each time, which serialized a test's many connects.
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let http = HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("fake relay client")
    });
    let mut req = http.request(method, url).body(payload);
    for (k, v) in headers {
        if k != axum::http::header::HOST && k != axum::http::header::CONTENT_LENGTH {
            req = req.header(k, v);
        }
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    let mut out = Response::builder().status(resp.status());
    for (k, v) in resp.headers() {
        if k != axum::http::header::CONTENT_LENGTH && k != axum::http::header::TRANSFER_ENCODING {
            out = out.header(k, v);
        }
    }
    let bytes = resp.bytes().await.unwrap_or_default();
    out.body(axum::body::Body::from(bytes))
        .unwrap_or_else(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))
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
            .route("/v1/usage", get(usage))
            .route("/v1/pairing-codes", get(pairing_list).post(pairing_mint))
            .route(
                "/v1/pairing-codes/{id}",
                axum::routing::delete(pairing_revoke),
            )
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

    /// Sets a machine's session: online since `connected_at` (Unix
    /// seconds), or offline (a new driver session gets a new time).
    pub fn set_session(&self, id: &str, online: bool, connected_at: Option<u64>) {
        if let Some(r) = self.state.lock().unwrap().machines.get_mut(id) {
            r.machine.online = online;
            r.machine.connected_at = if online { connected_at } else { None };
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

    /// The next machine registrations answer with `statuses`, in order,
    /// before the relay registers normally again.
    pub fn fail_registrations(&self, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .fail_register
            .extend(statuses.iter().copied());
    }

    /// The next machine reads (the list, or one machine) answer with
    /// `statuses`, in order, before the relay answers normally again.
    pub fn fail_reads(&self, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .fail_reads
            .extend(statuses.iter().copied());
    }

    /// While `on`, a request to a computer listed offline answers 502 at
    /// once, as the relay does past its start-up grace.
    pub fn set_refuse_offline(&self, on: bool) {
        self.state.lock().unwrap().refuse_offline = on;
    }

    /// While `down`, every machine read answers 503 "no available server"
    /// (a relay deploy, as its proxy answers).
    pub fn set_reads_down(&self, down: bool) {
        self.state.lock().unwrap().reads_down = down;
    }

    /// While set, a PATCH that changes who a computer is shared with is
    /// refused with `code` (`share_outside_team`), as the relay does for
    /// someone outside the owner's team.
    pub fn refuse_shares(&self, code: Option<&str>) {
        self.state.lock().unwrap().refuse_shares = code.map(str::to_string);
    }

    /// While `on`, a PATCH with `meta` is accepted but its keys are
    /// dropped, as a relay that predates them does.
    pub fn set_ignore_meta_patches(&self, on: bool) {
        self.state.lock().unwrap().ignore_meta_patches = on;
    }

    /// Machine registrations received so far (refused ones too).
    pub fn register_calls(&self) -> usize {
        self.state.lock().unwrap().register_calls
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

    /// Forwards the gRPC-Web requests to `/m/<id>/…` to `target` (a mock
    /// cua-spacesd), without the `/m/<id>` prefix, as if the machine's
    /// driver were connected.
    pub fn tunnel(&self, id: &str, target: &str) {
        self.tunnel_service(id, "", target);
    }

    /// As [`FakeRelay::tunnel`], for the requests whose path (after
    /// `/m/<id>`) starts with `prefix` only, such as
    /// `/cua.env.v1.HostSpacesService/`.
    pub fn tunnel_service(&self, id: &str, prefix: &str, target: &str) {
        self.state
            .lock()
            .unwrap()
            .tunnels
            .entry(id.to_string())
            .or_default()
            .push((prefix.to_string(), target.to_string()));
    }

    /// Answers `GET /v1/usage` with `usage` (the relay's JSON) for every
    /// account; none: 404, as a relay that does not meter.
    pub fn set_usage(&self, usage: Option<serde_json::Value>) {
        self.state.lock().unwrap().usage = usage;
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

// ---- the Spaces API (Teams are Keycloak organizations) ----

#[derive(Clone, Debug, Default)]
struct FakeOrg {
    id: String,
    alias: String,
    name: String,
    features: Vec<String>,
    /// sub → role
    members: BTreeMap<String, String>,
    /// (id, email, role)
    invites: Vec<(String, String, String)>,
    machines: Vec<crate::spaces_api::ApiOrgMachine>,
    approvals: Vec<crate::spaces_api::ApiDeviceApproval>,
}

#[derive(Default)]
struct ApiState {
    /// token → (sub, email)
    accounts: BTreeMap<String, (String, String)>,
    orgs: BTreeMap<String, FakeOrg>,
    /// `POST /v1/orgs/{org}/machines` answers `402 feature_required` this
    /// many more times (a plan bought a moment ago).
    feature_lag: usize,
    entitlement_reads: usize,
    /// Every org route answers 410, as a server without Teams.
    gone: bool,
    /// (owner sub, removal) not yet acknowledged.
    removals: Vec<(String, crate::spaces_api::ApiShareRemoval)>,
}

type A = Arc<Mutex<ApiState>>;

/// An in-process fake of the Spaces API's Teams routes (`/v1/me`,
/// `/v1/orgs/…`, `/v1/me/entitlements`): orgs with roles, machines and
/// device approvals, the `402 feature_required` of a plan that has not
/// arrived yet ([`FakeSpacesApi::feature_lag`]) and a server without
/// Teams ([`FakeSpacesApi::without_teams`]).
pub struct FakeSpacesApi {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    state: A,
}

fn api_fail(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": code, "detail": detail })),
    )
        .into_response()
}

type ApiCaller = Result<(String, String), Response>;

#[allow(clippy::result_large_err)]
fn api_caller(st: &ApiState, h: &HeaderMap) -> ApiCaller {
    let tok = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    st.accounts
        .get(tok)
        .cloned()
        .ok_or_else(|| api_fail(StatusCode::UNAUTHORIZED, "unauthorized", ""))
}

/// The org `key` (id or alias) and the caller's role in it, when at least
/// `min`.
#[allow(clippy::result_large_err)]
fn api_org(st: &ApiState, key: &str, sub: &str, min: &str) -> Result<(String, String), Response> {
    if st.gone {
        return Err(api_fail(StatusCode::GONE, "gone", "retired"));
    }
    let rank = |r: &str| match r {
        "owner" => 3,
        "admin" => 2,
        "member" => 1,
        _ => 0,
    };
    let Some(o) = st.orgs.values().find(|o| o.id == key || o.alias == key) else {
        return Err(api_fail(StatusCode::NOT_FOUND, "org_not_found", ""));
    };
    let Some(role) = o.members.get(sub) else {
        return Err(api_fail(StatusCode::NOT_FOUND, "org_not_found", ""));
    };
    if rank(role) < rank(min) {
        return Err(api_fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            &format!("needs the {min} role"),
        ));
    }
    Ok((o.id.clone(), role.clone()))
}

fn api_now() -> String {
    now_secs().to_string()
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

impl FakeSpacesApi {
    /// Starts on an ephemeral loopback port.
    pub async fn start() -> Self {
        let state: A = Arc::default();
        let app = Router::new()
            .route("/v1/me", get(api_me))
            .route("/v1/me/entitlements", get(api_entitlements))
            .route("/v1/orgs", post(api_create_org))
            .route("/v1/orgs/{org}", get(api_get_org).delete(api_delete_org))
            .route("/v1/orgs/{org}/members", get(api_members))
            .route(
                "/v1/orgs/{org}/members/{sub}",
                axum::routing::patch(api_set_role).delete(api_remove_member),
            )
            .route("/v1/orgs/{org}/leave", post(api_leave))
            .route("/v1/orgs/{org}/invites", get(api_invites).post(api_invite))
            .route(
                "/v1/orgs/{org}/machines",
                get(api_machines).post(api_add_machine),
            )
            .route(
                "/v1/orgs/{org}/machines/{id}",
                axum::routing::delete(api_remove_machine).patch(api_set_machine_role),
            )
            .route("/v1/me/sharing", get(api_sharing))
            .route("/v1/me/share-removals", get(api_share_removals))
            .route("/v1/me/share-removals/ack", post(api_ack_share_removals))
            .route("/v1/orgs/{org}/devices", get(api_devices))
            .route("/v1/orgs/{org}/devices/request", post(api_request_device))
            .route(
                "/v1/orgs/{org}/devices/{id}/{decision}",
                post(api_decide_device),
            )
            .fallback(|| async { api_fail(StatusCode::GONE, "gone", "retired") })
            .with_state(state.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        Self {
            url: format!("http://{addr}"),
            state,
        }
    }

    /// Accepts `token` as the account `sub`.
    pub fn add_account(&self, token: &str, sub: &str, email: &str) {
        self.state
            .lock()
            .unwrap()
            .accounts
            .insert(token.into(), (sub.into(), email.into()));
    }

    /// Adds an org with `features`, `owner` its owner; returns its id.
    pub fn add_org(&self, alias: &str, name: &str, owner: &str, features: &[&str]) -> String {
        let id = format!(
            "{:08x}-0000-4000-8000-{:012x}",
            rand::random::<u32>(),
            rand::random::<u64>() & 0xffff_ffff_ffff
        );
        let mut members = BTreeMap::new();
        members.insert(owner.to_string(), "owner".to_string());
        self.state.lock().unwrap().orgs.insert(
            id.clone(),
            FakeOrg {
                id: id.clone(),
                alias: alias.into(),
                name: name.into(),
                features: features.iter().map(|f| f.to_string()).collect(),
                members,
                ..Default::default()
            },
        );
        id
    }

    /// Makes `sub` a `role` of `org` (as accepting an invitation does).
    pub fn add_member(&self, org: &str, sub: &str, role: &str) {
        if let Some(o) = self.state.lock().unwrap().orgs.get_mut(org) {
            o.members.insert(sub.into(), role.into());
        }
    }

    /// The next `n` machine additions answer `402 feature_required`.
    pub fn feature_lag(&self, n: usize) {
        self.state.lock().unwrap().feature_lag = n;
    }

    /// Every org route answers 410 Gone, as a server without Teams.
    pub fn without_teams(&self) {
        self.state.lock().unwrap().gone = true;
    }

    /// How often `GET /v1/me/entitlements` was read.
    pub fn entitlement_reads(&self) -> usize {
        self.state.lock().unwrap().entitlement_reads
    }

    /// Records that `who` lost access to a computer of `owner` (a sub).
    pub fn add_share_removal(&self, owner: &str, machine_name: &str, who: &str) {
        self.state.lock().unwrap().removals.push((
            owner.into(),
            crate::spaces_api::ApiShareRemoval {
                machine_id: "m1".into(),
                machine_name: machine_name.into(),
                who: who.into(),
                removed_at: api_now(),
            },
        ));
    }

    /// The role of `machine` on `org` (`editor` or `viewer`).
    pub fn machine_role(&self, org: &str, machine: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .orgs
            .get(org)?
            .machines
            .iter()
            .find(|m| m.machine_id == machine)
            .map(|m| m.role.clone())
    }

    /// The machine ids bound to `org`.
    pub fn machines(&self, org: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .orgs
            .get(org)
            .map(|o| o.machines.iter().map(|m| m.machine_id.clone()).collect())
            .unwrap_or_default()
    }

    /// `(device_id, state)` of every approval request in `org`.
    pub fn approvals(&self, org: &str) -> Vec<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .orgs
            .get(org)
            .map(|o| {
                o.approvals
                    .iter()
                    .map(|a| (a.device_id.clone(), a.state.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn api_me(State(s): State<A>, h: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    let (sub, email) = tri!(api_caller(&st, &h));
    if st.gone {
        return api_fail(StatusCode::GONE, "gone", "retired");
    }
    let orgs: Vec<_> = st
        .orgs
        .values()
        .filter_map(|o| {
            let role = o.members.get(&sub)?;
            Some(serde_json::json!({
                "id": o.id, "alias": o.alias, "name": o.name, "role": role,
                "plan": if o.features.is_empty() { "free" } else { "teams" },
                "status": "active", "seats": 3, "features": o.features,
            }))
        })
        .collect();
    Json(serde_json::json!({
        "sub": sub, "email": email, "orgs": orgs,
        "entitlements": {"sub": sub, "plan": "free", "status": "none", "warnings": []},
    }))
    .into_response()
}

async fn api_entitlements(State(s): State<A>, h: HeaderMap) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    st.entitlement_reads += 1;
    Json(serde_json::json!({"sub": sub, "plan": "free"})).into_response()
}

async fn api_create_org(
    State(s): State<A>,
    h: HeaderMap,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    if st.gone {
        return api_fail(StatusCode::GONE, "gone", "retired");
    }
    let name = b["name"].as_str().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return api_fail(StatusCode::UNPROCESSABLE_ENTITY, "bad_name", "");
    }
    let alias: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let id = format!(
        "{:08x}-0000-4000-8000-{:012x}",
        rand::random::<u32>(),
        rand::random::<u64>() & 0xffff_ffff_ffff
    );
    let mut members = BTreeMap::new();
    members.insert(sub, "owner".to_string());
    st.orgs.insert(
        id.clone(),
        FakeOrg {
            id: id.clone(),
            alias: alias.clone(),
            name: name.clone(),
            members,
            ..Default::default()
        },
    );
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"id": id, "alias": alias, "name": name, "role": "owner", "status": "active"})),
    )
        .into_response()
}

async fn api_get_org(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, role) = tri!(api_org(&st, &org, &sub, "member"));
    let o = &st.orgs[&id];
    Json(serde_json::json!({
        "id": o.id, "alias": o.alias, "name": o.name, "role": role,
        "plan": if o.features.is_empty() { "free" } else { "teams" }, "status": "active",
        "features": o.features, "seats_paid": 3, "seats_used": o.members.len() + o.invites.len(),
    }))
    .into_response()
}

async fn api_delete_org(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "owner"));
    st.orgs.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}

async fn api_members(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "member"));
    let emails: BTreeMap<_, _> = st.accounts.values().cloned().collect();
    let members: Vec<_> = st.orgs[&id]
        .members
        .iter()
        .map(|(s, r)| serde_json::json!({"sub": s, "email": emails.get(s).cloned().unwrap_or_default(), "role": r}))
        .collect();
    Json(serde_json::json!({ "members": members })).into_response()
}

async fn api_set_role(
    State(s): State<A>,
    h: HeaderMap,
    Path((org, target)): Path<(String, String)>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "owner"));
    let role = b["role"].as_str().unwrap_or("").to_string();
    let o = st.orgs.get_mut(&id).unwrap();
    let Some(r) = o.members.get_mut(&target) else {
        return api_fail(StatusCode::NOT_FOUND, "member_not_found", "");
    };
    *r = role.clone();
    Json(serde_json::json!({"sub": target, "role": role})).into_response()
}

async fn api_remove_member(
    State(s): State<A>,
    h: HeaderMap,
    Path((org, target)): Path<(String, String)>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    if st
        .orgs
        .get_mut(&id)
        .unwrap()
        .members
        .remove(&target)
        .is_none()
    {
        return api_fail(StatusCode::NOT_FOUND, "member_not_found", "");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn api_leave(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, role) = tri!(api_org(&st, &org, &sub, "member"));
    if role == "owner" {
        return api_fail(
            StatusCode::CONFLICT,
            "owner_cannot_leave",
            "delete the org instead",
        );
    }
    st.orgs.get_mut(&id).unwrap().members.remove(&sub);
    StatusCode::NO_CONTENT.into_response()
}

async fn api_invites(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    let invites: Vec<_> = st.orgs[&id]
        .invites
        .iter()
        .map(|(i, e, _)| serde_json::json!({"id": i, "email": e}))
        .collect();
    Json(serde_json::json!({ "invites": invites })).into_response()
}

async fn api_invite(
    State(s): State<A>,
    h: HeaderMap,
    Path(org): Path<String>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    let email = b["email"].as_str().unwrap_or("").to_lowercase();
    let role = b["role"].as_str().unwrap_or("member").to_string();
    if !email.contains('@') {
        return api_fail(StatusCode::UNPROCESSABLE_ENTITY, "bad_invite", "");
    }
    let n = format!("inv-{}", rand::random::<u32>());
    st.orgs
        .get_mut(&id)
        .unwrap()
        .invites
        .push((n, email.clone(), role.clone()));
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"email": email, "role": role})),
    )
        .into_response()
}

async fn api_machines(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "member"));
    Json(serde_json::json!({ "machines": st.orgs[&id].machines })).into_response()
}

async fn api_add_machine(
    State(s): State<A>,
    h: HeaderMap,
    Path(org): Path<String>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    let machine = b["machine_id"].as_str().unwrap_or("").to_string();
    if machine.is_empty() {
        return api_fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_machine",
            "machine_id is required",
        );
    }
    if st.feature_lag > 0 {
        st.feature_lag -= 1;
        return api_fail(
            StatusCode::PAYMENT_REQUIRED,
            "feature_required",
            "team_machines",
        );
    }
    let o = st.orgs.get_mut(&id).unwrap();
    if !o.features.iter().any(|f| f == "team_machines") {
        return api_fail(
            StatusCode::PAYMENT_REQUIRED,
            "feature_required",
            "team_machines",
        );
    }
    if o.machines.iter().any(|m| m.machine_id == machine) {
        return api_fail(StatusCode::CONFLICT, "already_added", "");
    }
    o.machines.push(crate::spaces_api::ApiOrgMachine {
        machine_id: machine.clone(),
        platform: b["platform"].as_str().unwrap_or("").into(),
        label: b["label"].as_str().unwrap_or("").into(),
        added_by: sub,
        role: b["role"].as_str().unwrap_or("editor").into(),
        added_at: api_now(),
    });
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "machine_id": machine })),
    )
        .into_response()
}

async fn api_set_machine_role(
    State(s): State<A>,
    h: HeaderMap,
    Path((org, machine)): Path<(String, String)>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    let role = b["role"].as_str().unwrap_or("");
    if role != "viewer" && role != "editor" {
        return api_fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_role",
            "role is viewer or editor",
        );
    }
    let o = st.orgs.get_mut(&id).unwrap();
    let Some(m) = o.machines.iter_mut().find(|m| m.machine_id == machine) else {
        return api_fail(StatusCode::NOT_FOUND, "machine_not_found", "");
    };
    m.role = role.into();
    Json(serde_json::json!({ "machine_id": machine, "role": role })).into_response()
}

/// `GET /v1/me/sharing`: can share when in an org with `space_sharing`.
async fn api_sharing(State(s): State<A>, h: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    if st.gone {
        return api_fail(StatusCode::GONE, "gone", "retired");
    }
    let teams: Vec<_> = st
        .orgs
        .values()
        .filter(|o| o.members.contains_key(&sub) && o.features.iter().any(|f| f == "space_sharing"))
        .map(|o| {
            let role = o.members[&sub].as_str();
            serde_json::json!({
                "id": o.id, "alias": o.alias, "name": o.name,
                "can_add": role == "owner" || role == "admin",
            })
        })
        .collect();
    let mut out = serde_json::json!({ "can_share": !teams.is_empty(), "teams": teams });
    if teams.is_empty() {
        out["reason"] = "no_team".into();
    }
    Json(out).into_response()
}

async fn api_share_removals(State(s): State<A>, h: HeaderMap) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let rows: Vec<_> = st
        .removals
        .iter()
        .filter(|(owner, _)| *owner == sub)
        .map(|(_, r)| r.clone())
        .collect();
    Json(serde_json::json!({ "removals": rows })).into_response()
}

async fn api_ack_share_removals(State(s): State<A>, h: HeaderMap) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    st.removals.retain(|(owner, _)| *owner != sub);
    StatusCode::NO_CONTENT.into_response()
}

async fn api_remove_machine(
    State(s): State<A>,
    h: HeaderMap,
    Path((org, machine)): Path<(String, String)>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    let o = st.orgs.get_mut(&id).unwrap();
    let before = o.machines.len();
    o.machines.retain(|m| m.machine_id != machine);
    if o.machines.len() == before {
        return api_fail(StatusCode::NOT_FOUND, "machine_not_found", "");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn api_devices(State(s): State<A>, h: HeaderMap, Path(org): Path<String>) -> Response {
    let st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, role) = tri!(api_org(&st, &org, &sub, "member"));
    let devices: Vec<_> = st.orgs[&id]
        .approvals
        .iter()
        .filter(|a| role != "member" || a.sub == sub)
        .collect();
    Json(serde_json::json!({ "devices": devices })).into_response()
}

async fn api_request_device(
    State(s): State<A>,
    h: HeaderMap,
    Path(org): Path<String>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "member"));
    let device = b["device_id"].as_str().unwrap_or("").to_string();
    if device.is_empty() {
        return api_fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_device",
            "device_id is required",
        );
    }
    let o = st.orgs.get_mut(&id).unwrap();
    o.approvals
        .retain(|a| !(a.sub == sub && a.device_id == device));
    let a = crate::spaces_api::ApiDeviceApproval {
        sub,
        device_id: device,
        state: "pending".into(),
        requested_at: api_now(),
        ..Default::default()
    };
    o.approvals.push(a.clone());
    Json(a).into_response()
}

async fn api_decide_device(
    State(s): State<A>,
    h: HeaderMap,
    Path((org, device, decision)): Path<(String, String, String)>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let mut st = s.lock().unwrap();
    let (sub, _) = tri!(api_caller(&st, &h));
    let (id, _) = tri!(api_org(&st, &org, &sub, "admin"));
    let member = b["sub"].as_str().unwrap_or("").to_string();
    if member.is_empty() {
        return api_fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_request",
            "sub is required",
        );
    }
    let state = match decision.as_str() {
        "approve" => "approved",
        "deny" => "denied",
        _ => return api_fail(StatusCode::NOT_FOUND, "gone", ""),
    };
    let o = st.orgs.get_mut(&id).unwrap();
    let Some(a) = o
        .approvals
        .iter_mut()
        .find(|a| a.sub == member && a.device_id == device)
    else {
        return api_fail(StatusCode::NOT_FOUND, "request_not_found", "");
    };
    a.state = state.into();
    a.decided_by = Some(sub);
    a.decided_at = Some(api_now());
    Json(a.clone()).into_response()
}
