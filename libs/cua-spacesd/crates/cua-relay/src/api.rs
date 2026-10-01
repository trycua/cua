// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Machine directory API (account mode):
//!
//! - `GET  /.well-known/jwks.json`: the assertion signing key.
//! - `GET  /v1/info`
//! - `POST /v1/machines` (account): register a machine, get its token.
//! - `GET  /v1/machines` (account): owned + shared machines.
//! - `GET | PATCH | DELETE /v1/machines/{id}`
//! - `POST /v1/machines/{id}/stop-sharing` | `start-sharing` (owner or the
//!   machine itself): cut every client and refuse new ones, or resume.
//!
//! Account callers other than registration need an enrolled client device
//! (see [`crate::devices`]). A machine token only serves its own machine:
//! it reads its own record, stops sharing, undoes a stop it made itself and
//! unregisters; it cannot rename, change the allowlist, list or reach other
//! machines, or register.

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::device_api::{flag_grace, gate, session_header, Gate};
use crate::devices::AuditEvent;
use crate::directory::{
    DirectoryError, MachinePatch, MachineRecord, Owner, Role, MACHINE_TOKEN_PREFIX,
};
use crate::oidc::Identity;
use crate::server::{account_token, bearer, Registrant, Relay};

/// A connected client in a machine's presence list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientView {
    /// User id.
    pub id: String,
    /// Email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Open streams.
    pub streams: u32,
    /// First seen (Unix seconds).
    pub since: u64,
}

/// A machine as the directory API shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineView {
    /// Machine id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Owner.
    pub owner: Owner,
    /// `owner` or `shared` (the caller's role; `owner` for the machine itself).
    pub role: String,
    /// Connected to the relay now.
    pub online: bool,
    /// False after stop-sharing.
    pub sharing: bool,
    /// spacesd version while online.
    pub version: String,
    /// Connection time while online.
    pub connected_at: Option<u64>,
    /// Client URL: `<public_url>/m/<id>`.
    pub url: String,
    /// Allowlist (owner view only).
    pub allow: Vec<String>,
    /// View-only accounts (owner view only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewers: Vec<String>,
    /// Account users connected now.
    pub clients: Vec<ClientView>,
    /// For a Space a host provides: the host's machine id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// [`MachineRecord::confirmed`]: false shows as "new" until an enrolled
    /// device confirms it (`POST /v1/machines/{id}/confirm`, owner view
    /// only -- a shared viewer/editor does not need to know).
    pub confirmed: bool,
    /// [`MachineRecord::meta`] (what the registering client says about the
    /// machine; no secrets).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RegisterBody {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    allow: Option<Vec<String>>,
    /// A Space a host provides: that host's machine id. The caller must be
    /// able to use the host (its owner, or an editor) from an enrolled
    /// device.
    #[serde(default)]
    host: Option<String>,
    /// [`MachineRecord::meta`]; replaces the machine's when given.
    #[serde(default)]
    meta: Option<BTreeMap<String, String>>,
}

#[derive(Serialize)]
struct RegisterReply {
    machine: MachineView,
    machine_token: String,
    jwks: serde_json::Value,
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/.well-known/jwks.json", get(jwks))
        .route("/v1/info", get(info))
        .route("/v1/machines", get(list).post(register))
        .route(
            "/v1/machines/{id}",
            get(get_one).patch(patch_one).delete(delete_one),
        )
        .route("/v1/machines/{id}/stop-sharing", post(stop_sharing))
        .route("/v1/machines/{id}/start-sharing", post(start_sharing))
        .route("/v1/machines/{id}/confirm", post(confirm))
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({"error": message.into()}))).into_response()
}

fn directory_error(e: DirectoryError) -> Response {
    let status = match &e {
        DirectoryError::Invalid(_) => StatusCode::BAD_REQUEST,
        DirectoryError::Forbidden(_) => StatusCode::FORBIDDEN,
        DirectoryError::NotFound => StatusCode::NOT_FOUND,
        DirectoryError::Conflict(_) => StatusCode::CONFLICT,
        DirectoryError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, e.to_string())
}

/// Header a host re-running setup sends its current machine token in, so
/// rotating an existing machine's token needs the machine (or an enrolled
/// device), not just an account session.
pub const MACHINE_AUTHORIZATION_HEADER: &str = "x-cua-machine-authorization";

enum Caller {
    /// An account user and how their device stands.
    Account(Identity, Result<Gate, String>),
    Machine(MachineRecord),
}

impl Caller {
    /// Fails for an account caller whose device is not enrolled (after the
    /// grace period).
    fn require_device(&self) -> Result<(), Response> {
        match self {
            Caller::Account(_, Err(m)) => Err(error(StatusCode::FORBIDDEN, m.clone())),
            _ => Ok(()),
        }
    }

    /// The acting device (enrolled account device) or `machine`.
    fn actor(&self) -> Option<String> {
        match self {
            Caller::Account(_, Ok(g)) => g.device().map(str::to_owned),
            Caller::Account(_, Err(_)) => None,
            Caller::Machine(_) => Some("machine".into()),
        }
    }

    fn in_grace(&self) -> bool {
        matches!(self, Caller::Account(_, Ok(Gate::Grace)))
    }
}

fn finish(relay: &Relay, caller: &Caller, mut response: Response) -> Response {
    if caller.in_grace() {
        flag_grace(relay, response.headers_mut());
    }
    response
}

async fn caller(relay: &Relay, headers: &HeaderMap) -> Result<Caller, Response> {
    let machine_token = bearer(headers, header::AUTHORIZATION.as_str())
        .filter(|t| t.starts_with(MACHINE_TOKEN_PREFIX));
    if let Some(token) = machine_token {
        return relay
            .directory
            .machine_for_token(token)
            .map(Caller::Machine)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "invalid machine token"));
    }
    let Some(oidc) = &relay.config.oidc else {
        return Err(error(
            StatusCode::NOT_FOUND,
            "account mode is not enabled on this relay",
        ));
    };
    let Some(token) = account_token(headers) else {
        return Err(error(StatusCode::UNAUTHORIZED, "missing account token"));
    };
    oidc.validate(token)
        .await
        .map(|who| {
            let g = gate(relay, session_header(headers), &who);
            Caller::Account(who, g)
        })
        .map_err(|e| {
            error(
                StatusCode::UNAUTHORIZED,
                format!("invalid account token: {e}"),
            )
        })
}

fn view(relay: &Relay, headers: &HeaderMap, record: &MachineRecord, role: Role) -> MachineView {
    let live = relay.machine(&record.id).filter(
        |m| matches!(&m.registrant, Registrant::Account(owner) if owner == &record.owner.id),
    );
    let clients = live
        .as_ref()
        .map(|m| {
            m.presence()
                .into_iter()
                .map(|(id, c)| ClientView {
                    id,
                    email: c.email,
                    name: c.name,
                    streams: c.streams,
                    since: c.since,
                })
                .collect()
        })
        .unwrap_or_default();
    MachineView {
        id: record.id.clone(),
        name: record.name.clone(),
        owner: record.owner.clone(),
        role: role.as_str().into(),
        online: live.is_some(),
        sharing: record.sharing,
        version: live.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        connected_at: live.as_ref().map(|m| {
            m.connected_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }),
        url: relay.machine_url(headers, &record.id),
        allow: if role == Role::Owner {
            record.allow.clone()
        } else {
            Vec::new()
        },
        viewers: if role == Role::Owner {
            record.viewers.clone()
        } else {
            Vec::new()
        },
        clients: if role == Role::Owner {
            clients
        } else {
            Vec::new()
        },
        host: record.host.clone(),
        confirmed: record.confirmed,
        meta: record.meta.clone(),
    }
}

/// The record `id` and the caller's role; the machine itself acts as owner.
fn authorize(caller: &Caller, relay: &Relay, id: &str) -> Result<(MachineRecord, Role), Response> {
    match caller {
        Caller::Machine(record) if record.id == id => Ok((record.clone(), Role::Owner)),
        Caller::Machine(_) => Err(error(
            StatusCode::FORBIDDEN,
            "machine token for another machine",
        )),
        Caller::Account(who, _) => {
            let record = relay
                .directory
                .get(id)
                .ok_or_else(|| directory_error(DirectoryError::NotFound))?;
            // Machines not shared with the caller are indistinguishable from
            // missing ones.
            let role = record
                .role_of(who)
                .ok_or_else(|| directory_error(DirectoryError::NotFound))?;
            Ok((record, role))
        }
    }
}

fn require_owner(role: Role) -> Result<(), Response> {
    (role == Role::Owner)
        .then_some(())
        .ok_or_else(|| error(StatusCode::FORBIDDEN, "only the owner can do this"))
}

async fn jwks(State(relay): State<Relay>) -> Response {
    Json(relay.key.jwks()).into_response()
}

async fn info(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    Json(serde_json::json!({
        "public_url": relay.public_url(&headers),
        "issuer": relay.config.oidc.as_ref().map(|o| o.config().issuer.clone()),
        "account_auth": relay.config.oidc.is_some(),
        "static_tokens": !relay.config.tokens.is_empty(),
    }))
    .into_response()
}

async fn register(
    State(relay): State<Relay>,
    headers: HeaderMap,
    body: Json<RegisterBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(Caller::Account(who, device)) => (who, device),
        Ok(Caller::Machine(_)) => {
            return error(StatusCode::FORBIDDEN, "register with an account token")
        }
        Err(r) => return r,
    };
    // An id currently connected with a static relay token belongs to that
    // (self-hosted) machine: an account cannot take it over.
    if relay
        .machine(&body.id)
        .is_some_and(|m| matches!(m.registrant, Registrant::Static(_)))
    {
        return error(
            StatusCode::CONFLICT,
            "machine id is connected with a relay token",
        );
    }
    let existing = relay.directory.get(&body.id);
    let existed = existing.is_some();
    // Registering a new machine needs only the account (hosting never
    // enrolls the host as a client). Re-registering an existing one rotates
    // its token and can change its allowlist, so it also needs the current
    // machine token (the host re-running setup) or an enrolled device.
    if let Some(record) = &existing {
        let by_machine = bearer(&headers, MACHINE_AUTHORIZATION_HEADER)
            .and_then(|t| relay.directory.machine_for_token(t))
            .is_some_and(|m| m.id == record.id);
        if !by_machine && record.owner.id == who.account {
            if let Err(m) = &device {
                return error(
                    StatusCode::FORBIDDEN,
                    format!("re-registering this machine needs its machine token or an enrolled device ({m})"),
                );
            }
        }
    }
    // A Space on someone's host: only an account that can use that host
    // (its owner or an editor, never a viewer), from an enrolled device.
    if let Some(host) = body.host.as_deref() {
        if let Err(m) = &device {
            return error(
                StatusCode::FORBIDDEN,
                format!("adding a Space to a host needs an enrolled device ({m})"),
            );
        }
        match relay.directory.get(host).and_then(|h| h.role_of(&who)) {
            Some(Role::Owner | Role::Shared) => {}
            Some(Role::Viewer) => {
                return error(
                    StatusCode::FORBIDDEN,
                    "that host is shared with you to watch only",
                )
            }
            None => return error(StatusCode::NOT_FOUND, "no such host machine"),
        }
    }
    let actor = device
        .as_ref()
        .ok()
        .and_then(|g| g.device().map(str::to_owned));
    let old_allow = existing.map(|r| r.allow).unwrap_or_default();
    // A brand-new machine still needs only the account token (hosting never
    // enrolls the host as a client), but one that arrives with neither an
    // enrolled device's signature (the device session already verified
    // above) nor a sign-in whose token proves MFA is not proven the way a
    // registration this relay can vouch for is: it starts unconfirmed, so a
    // rogue machine a stolen session registered stands out in the apps
    // instead of blending in (S5). Device enrollment being off on this
    // relay (`Gate::Off`) has no such signal to withhold, so it does not
    // flag anything.
    let proven = matches!(
        device,
        Ok(crate::device_api::Gate::Device(_)) | Ok(crate::device_api::Gate::Off)
    ) || who.mfa;
    // Metadata is checked before anything is registered.
    if let Some(meta) = body.meta.clone() {
        if let Err(e) = crate::directory::clean_meta(meta) {
            return directory_error(e);
        }
    }
    match relay.directory.register(
        &who,
        &body.id,
        body.name.as_deref(),
        body.allow.as_deref(),
        proven,
    ) {
        Ok((record, token)) => {
            let record = match body.host.as_deref() {
                Some(host) => match relay.directory.set_host(&record.id, Some(host)) {
                    Ok(r) => r,
                    Err(e) => return directory_error(e),
                },
                None => record,
            };
            let record = match body.meta.clone() {
                Some(meta) => match relay.directory.set_meta(&record.id, meta) {
                    Ok(r) => r,
                    Err(e) => return directory_error(e),
                },
                None => record,
            };
            // A rotated token invalidates the running session's credential.
            if existed {
                relay.drop_machine(&record.id);
            }
            tracing::info!(machine = %record.id, account = %who.account, confirmed = %record.confirmed, "machine registered");
            relay.devices.audit(
                &who.account,
                AuditEvent::new(if existed {
                    "machine_reregistered"
                } else {
                    "machine_registered"
                })
                .device(actor.as_deref())
                .machine(&record.id)
                .detail(match &record.host {
                    Some(host) => format!("{} (a Space on host {host})", record.name),
                    None => record.name.clone(),
                }),
            );
            if !existed && !record.confirmed {
                relay.devices.audit(
                    &who.account,
                    AuditEvent::new("machine_unconfirmed")
                        .device(actor.as_deref())
                        .machine(&record.id)
                        .detail("registered with an account token only; confirm it from an enrolled device if you recognize it"),
                );
            }
            audit_allow_changes(&relay, &record, &old_allow, actor.as_deref());
            let reply = RegisterReply {
                machine: view(&relay, &headers, &record, Role::Owner),
                machine_token: token,
                jwks: relay.key.jwks(),
            };
            let status = if existed {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (status, Json(reply)).into_response()
        }
        Err(e) => directory_error(e),
    }
}

async fn list(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let caller = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = caller.require_device() {
        return r;
    }
    let machines = match &caller {
        Caller::Account(who, _) => relay
            .directory
            .visible_to(who)
            .into_iter()
            .map(|(r, role)| view(&relay, &headers, &r, role))
            .collect::<Vec<_>>(),
        Caller::Machine(record) => vec![view(&relay, &headers, record, Role::Owner)],
    };
    finish(
        &relay,
        &caller,
        Json(serde_json::json!({ "machines": machines })).into_response(),
    )
}

async fn get_one(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = caller.require_device() {
        return r;
    }
    match authorize(&caller, &relay, &id) {
        Ok((record, role)) => finish(
            &relay,
            &caller,
            Json(view(&relay, &headers, &record, role)).into_response(),
        ),
        Err(r) => r,
    }
}

fn apply_sharing(relay: &Relay, record: &MachineRecord) {
    if !record.sharing {
        if let Some(machine) = relay.machine(&record.id) {
            machine.cut_clients();
        }
        tracing::info!(machine = %record.id, "sharing stopped; clients cut");
    }
}

async fn patch_one(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(patch): Json<MachinePatch>,
) -> Response {
    let caller = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Caller::Machine(_) = caller {
        return error(
            StatusCode::FORBIDDEN,
            "a machine token cannot rename the machine or change who it is shared with; use your account",
        );
    }
    if let Err(r) = caller.require_device() {
        return r;
    }
    let (before, role) = match authorize(&caller, &relay, &id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_owner(role) {
        return r;
    }
    let actor = caller.actor();
    match relay.directory.update(&id, &patch) {
        Ok(record) => {
            apply_sharing(&relay, &record);
            audit_allow_changes(&relay, &record, &before.allow, actor.as_deref());
            audit_viewer_changes(&relay, &record, &before.viewers, actor.as_deref());
            if before.sharing != record.sharing {
                audit_sharing(&relay, &record, actor.as_deref());
            }
            if patch.allow.is_some() || patch.viewers.is_some() {
                // Revoked accounts lose their open streams too, and so does
                // an account whose role changed (an editor turned viewer
                // must not keep an input stream it opened as an editor).
                if let Some(machine) = relay.machine(&id) {
                    let changed = machine.presence().into_iter().any(|(user, c)| {
                        let who = Identity {
                            user: user.clone(),
                            account: user,
                            email: c.email,
                            name: None,
                            auth_time: None,
                            mfa: false,
                        };
                        let now = record.role_of(&who);
                        now.is_none() || now != before.role_of(&who)
                    });
                    if changed {
                        machine.cut_clients();
                    }
                }
            }
            Json(view(&relay, &headers, &record, Role::Owner)).into_response()
        }
        Err(e) => directory_error(e),
    }
}

async fn delete_one(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = caller.require_device() {
        return r;
    }
    let (_, role) = match authorize(&caller, &relay, &id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_owner(role) {
        return r;
    }
    match relay.directory.remove(&id) {
        Ok(removed) => {
            relay.drop_machine(&id);
            tracing::info!(machine = %id, "machine removed");
            relay.devices.audit(
                &removed.owner.id,
                AuditEvent::new("machine_removed")
                    .device(caller.actor().as_deref())
                    .machine(&id),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => directory_error(e),
    }
}

async fn set_sharing(relay: Relay, id: String, headers: HeaderMap, sharing: bool) -> Response {
    let caller = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(r) = caller.require_device() {
        return r;
    }
    let (before, role) = match authorize(&caller, &relay, &id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_owner(role) {
        return r;
    }
    let by = match caller {
        Caller::Machine(_) => "machine",
        Caller::Account(..) => "owner",
    };
    // The machine token may stop sharing (a local kill switch) and undo a
    // stop it made itself, but not one the owner made from an account.
    if sharing
        && by == "machine"
        && !before.sharing
        && before.stopped_by.as_deref() != Some("machine")
    {
        return error(
            StatusCode::FORBIDDEN,
            "sharing was stopped from your account; start it again from your account",
        );
    }
    match relay.directory.set_sharing(&id, sharing, by) {
        Ok(record) => {
            apply_sharing(&relay, &record);
            if before.sharing != record.sharing {
                audit_sharing(&relay, &record, caller.actor().as_deref());
            }
            Json(view(&relay, &headers, &record, Role::Owner)).into_response()
        }
        Err(e) => directory_error(e),
    }
}

/// Vouches for a machine that registered without proof (S5): only the
/// owner, from an enrolled device (a machine token cannot confirm itself,
/// since the whole point is a second party's say-so).
async fn confirm(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Caller::Machine(_) = caller {
        return error(
            StatusCode::FORBIDDEN,
            "a machine cannot confirm itself; confirm it from an enrolled device",
        );
    }
    if let Err(r) = caller.require_device() {
        return r;
    }
    let (before, role) = match authorize(&caller, &relay, &id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_owner(role) {
        return r;
    }
    if before.confirmed {
        return Json(view(&relay, &headers, &before, Role::Owner)).into_response();
    }
    match relay.directory.confirm(&id) {
        Ok(record) => {
            relay.devices.audit(
                &record.owner.id,
                AuditEvent::new("machine_confirmed")
                    .device(caller.actor().as_deref())
                    .machine(&record.id),
            );
            Json(view(&relay, &headers, &record, Role::Owner)).into_response()
        }
        Err(e) => directory_error(e),
    }
}

async fn stop_sharing(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    set_sharing(relay, id, headers, false).await
}

async fn start_sharing(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    set_sharing(relay, id, headers, true).await
}

/// Records allowlist entries added to or removed from `record` in the
/// owner's audit log.
fn audit_allow_changes(
    relay: &Relay,
    record: &MachineRecord,
    before: &[String],
    actor: Option<&str>,
) {
    for added in record.allow.iter().filter(|a| !before.contains(a)) {
        relay.devices.audit(
            &record.owner.id,
            AuditEvent::new("share_added")
                .device(actor)
                .machine(&record.id)
                .subject(added.clone()),
        );
    }
    for removed in before.iter().filter(|b| !record.allow.contains(b)) {
        relay.devices.audit(
            &record.owner.id,
            AuditEvent::new("share_removed")
                .device(actor)
                .machine(&record.id)
                .subject(removed.clone()),
        );
    }
}

fn audit_viewer_changes(
    relay: &Relay,
    record: &MachineRecord,
    before: &[String],
    actor: Option<&str>,
) {
    for added in record.viewers.iter().filter(|a| !before.contains(a)) {
        relay.devices.audit(
            &record.owner.id,
            AuditEvent::new("viewer_added")
                .device(actor)
                .machine(&record.id)
                .subject(added.clone()),
        );
    }
    for removed in before.iter().filter(|b| !record.viewers.contains(b)) {
        relay.devices.audit(
            &record.owner.id,
            AuditEvent::new("viewer_removed")
                .device(actor)
                .machine(&record.id)
                .subject(removed.clone()),
        );
    }
}

fn audit_sharing(relay: &Relay, record: &MachineRecord, actor: Option<&str>) {
    relay.devices.audit(
        &record.owner.id,
        AuditEvent::new(if record.sharing {
            "sharing_started"
        } else {
            "sharing_stopped"
        })
        .device(actor)
        .machine(&record.id),
    );
}
