// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teams API (account mode), see [`crate::teams`]. Every route needs an
//! account token from an enrolled device; machine tokens are refused.
//!
//! - `GET    /v1/teams`                         your teams + email invites for you
//! - `POST   /v1/teams`                         create `{name}`
//! - `POST   /v1/teams/join`                    join `{code}` or `{team}` (email invite)
//! - `GET | PATCH | DELETE /v1/teams/{id}`      view, rename `{name}`, delete (owner)
//! - `POST   /v1/teams/{id}/leave`
//! - `POST   /v1/teams/{id}/invites`            `{email?, role?, uses?}` -> code + link
//! - `DELETE /v1/teams/{id}/invites/{invite}`
//! - `PATCH | DELETE /v1/teams/{id}/members/{who}`  role `{role}` (owner) / remove
//! - `POST   /v1/teams/{id}/devices/request`    this device asks for approval
//! - `POST   /v1/teams/{id}/devices/{device}/approve` | `deny`
//! - `POST   /v1/teams/{id}/machines`           `{machine, os?}` add a machine you own
//! - `DELETE /v1/teams/{id}/machines/{machine}`
//! - `GET    /v1/teams/{id}/audit?limit=`       owner/admin
//! - `GET    /join/team/{code}`                 a plain page for an invite link

#![allow(clippy::result_large_err)]

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::{caller, error, Caller};
use crate::oidc::Identity;
use crate::server::Relay;
use crate::teams::{
    Actor, DeviceApproval, Invite, Member, Team, TeamAudit, TeamDevice, TeamError, TeamRole,
};

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/teams", get(list).post(create))
        .route("/v1/teams/join", post(join))
        .route("/v1/teams/{id}", get(get_one).patch(rename).delete(remove))
        .route("/v1/teams/{id}/leave", post(leave))
        .route("/v1/teams/{id}/invites", post(invite))
        .route("/v1/teams/{id}/invites/{invite}", delete(revoke_invite))
        .route(
            "/v1/teams/{id}/members/{who}",
            patch(set_role).delete(remove_member),
        )
        .route("/v1/teams/{id}/devices/request", post(request_device))
        .route("/v1/teams/{id}/devices/{device}/approve", post(approve))
        .route("/v1/teams/{id}/devices/{device}/deny", post(deny))
        .route("/v1/teams/{id}/machines", post(add_machine))
        .route("/v1/teams/{id}/machines/{machine}", delete(remove_machine))
        .route("/v1/teams/{id}/audit", get(audit))
        .route("/join/team/{code}", get(join_page))
}

/// With teams on Keycloak Organizations (`CUA_RELAY_TEAMS_SOURCE=org`) the
/// relay's own team routes are retired: 410 with what to do.
pub(crate) fn gone_routes() -> Router<Relay> {
    async fn gone() -> Response {
        (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "gone",
                "detail": "Teams moved to your Cua account (app.cua.ai/team); update cua",
            })),
        )
            .into_response()
    }
    Router::new()
        .route("/v1/teams", axum::routing::any(gone))
        .route("/v1/teams/{*rest}", axum::routing::any(gone))
        .route("/join/team/{code}", axum::routing::any(gone))
}

fn team_error(e: TeamError) -> Response {
    let status = match &e {
        TeamError::Invalid(_) => StatusCode::BAD_REQUEST,
        TeamError::Forbidden(_) => StatusCode::FORBIDDEN,
        TeamError::NotFound(_) => StatusCode::NOT_FOUND,
        TeamError::Conflict(_) => StatusCode::CONFLICT,
        TeamError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, e.to_string())
}

/// An invite as listed (never its code).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteView {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub role: TeamRole,
    pub expires_at: u64,
    pub uses_left: u32,
}

impl From<&Invite> for InviteView {
    fn from(i: &Invite) -> Self {
        Self {
            id: i.id.clone(),
            email: i.email.clone(),
            role: i.role,
            expires_at: i.expires_at,
            uses_left: i.uses_left,
        }
    }
}

/// A team as one of its members sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamView {
    pub id: String,
    pub name: String,
    /// The caller's role.
    pub role: TeamRole,
    pub created_at: u64,
    pub members: Vec<Member>,
    /// Team machines: id and name (when still registered).
    pub machines: Vec<TeamMachine>,
    /// Open invites (owner/admin view only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invites: Vec<InviteView>,
    /// Member devices and their approval (owner/admin view: all; member
    /// view: their own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<TeamDevice>,
    /// This device's approval for the team, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub this_device: Option<DeviceApproval>,
}

/// A team machine in a [`TeamView`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMachine {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub online: bool,
}

fn team_view(relay: &Relay, t: &Team, account: &str, device: Option<&str>) -> TeamView {
    let role = t.role_of(account).unwrap_or(TeamRole::Member);
    let manages = role.manages();
    let now = crate::assertion::now_secs();
    TeamView {
        id: t.id.clone(),
        name: t.name.clone(),
        role,
        created_at: t.created_at,
        members: t.members.clone(),
        machines: t
            .machines
            .iter()
            .map(|id| {
                let record = relay.directory.get(id);
                TeamMachine {
                    id: id.clone(),
                    name: record.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                    owner: record
                        .as_ref()
                        .map(|r| r.owner.email.clone().unwrap_or_else(|| r.owner.id.clone()))
                        .unwrap_or_default(),
                    online: relay.machine(id).is_some(),
                }
            })
            .collect(),
        invites: if manages {
            t.invites
                .iter()
                .filter(|i| i.expires_at > now && i.uses_left > 0)
                .map(InviteView::from)
                .collect()
        } else {
            Vec::new()
        },
        devices: t
            .devices
            .iter()
            .filter(|d| manages || d.account == account)
            .cloned()
            .collect(),
        this_device: device.and_then(|dev| {
            t.devices
                .iter()
                .find(|d| d.account == account && d.device == dev)
                .map(|d| d.state)
        }),
    }
}

/// The account caller from an enrolled device (machine tokens refused).
async fn account(
    relay: &Relay,
    headers: &HeaderMap,
) -> Result<(Identity, Option<String>), Response> {
    let caller = caller(relay, headers).await?;
    if let Caller::Machine(_) = caller {
        return Err(error(
            StatusCode::FORBIDDEN,
            "teams are managed with your account, not a machine token",
        ));
    }
    caller.require_device()?;
    let device = caller.actor();
    match caller {
        Caller::Account(who, _) => Ok((who, device)),
        Caller::Machine(_) => unreachable!(),
    }
}

fn device_name(relay: &Relay, device: Option<&str>) -> String {
    device
        .and_then(|d| relay.devices.get(d))
        .map(|d| d.name)
        .unwrap_or_default()
}

async fn list(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let teams: Vec<_> = relay
        .teams
        .teams_of(&who.account)
        .iter()
        .map(|(t, _)| team_view(&relay, t, &who.account, device.as_deref()))
        .collect();
    let invites: Vec<_> = who
        .email
        .as_deref()
        .map(|e| relay.teams.invites_for(e))
        .unwrap_or_default()
        .into_iter()
        .filter(|(t, _)| t.role_of(&who.account).is_none())
        .map(|(t, i)| {
            serde_json::json!({
                "team": t.id,
                "team_name": t.name,
                "invite": InviteView::from(&i),
            })
        })
        .collect();
    Json(serde_json::json!({ "teams": teams, "invites": invites })).into_response()
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn create(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<NameBody>,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.create(&actor, &body.name) {
        Ok(t) => {
            // The owner's creating device is the team's first approved one.
            if let Some(d) = device.as_deref() {
                let _ = relay
                    .teams
                    .request_approval(&t.id, &actor, &device_name(&relay, Some(d)));
            }
            let t = relay
                .teams
                .get_for(&t.id, &who.account)
                .map(|(t, _)| t)
                .unwrap_or(t);
            (
                StatusCode::CREATED,
                Json(team_view(&relay, &t, &who.account, device.as_deref())),
            )
                .into_response()
        }
        Err(e) => team_error(e),
    }
}

async fn get_one(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match relay.teams.get_for(&id, &who.account) {
        Ok((t, _)) => Json(team_view(&relay, &t, &who.account, device.as_deref())).into_response(),
        Err(e) => team_error(e),
    }
}

async fn rename(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NameBody>,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.rename(&id, &actor, &body.name) {
        Ok(t) => Json(team_view(&relay, &t, &who.account, device.as_deref())).into_response(),
        Err(e) => team_error(e),
    }
}

async fn remove(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.delete(&id, &actor) {
        Ok(_) => {
            relay.reauthorize(None);
            tracing::info!(team = %id, "team deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => team_error(e),
    }
}

#[derive(Deserialize)]
struct JoinBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    team: Option<String>,
}

async fn join(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<JoinBody>,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    // A pasted link works as well as the bare code.
    let code = body.code.as_deref().map(|c| {
        let c = c.trim();
        c.rsplit_once("/join/team/")
            .map(|(_, code)| code)
            .unwrap_or(c)
    });
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    let name = device_name(&relay, device.as_deref());
    match relay
        .teams
        .join(&actor, body.team.as_deref(), code, Some(&name))
    {
        Ok((t, _)) => Json(team_view(&relay, &t, &who.account, device.as_deref())).into_response(),
        Err(e) => team_error(e),
    }
}

async fn leave(State(relay): State<Relay>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.leave(&id, &actor) {
        Ok(_) => {
            relay.reauthorize(None);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => team_error(e),
    }
}

#[derive(Deserialize)]
struct InviteBody {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    role: Option<TeamRole>,
    #[serde(default)]
    uses: Option<u32>,
}

async fn invite(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InviteBody>,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.invite(
        &id,
        &actor,
        body.email.as_deref().filter(|e| !e.trim().is_empty()),
        body.role.unwrap_or(TeamRole::Member),
        body.uses,
    ) {
        Ok(n) => {
            let link = format!("{}/join/team/{}", relay.public_url(&headers), n.code);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "invite": InviteView::from(&n.invite),
                    "code": n.code,
                    "link": link,
                })),
            )
                .into_response()
        }
        Err(e) => team_error(e),
    }
}

async fn revoke_invite(
    State(relay): State<Relay>,
    Path((id, invite)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.revoke_invite(&id, &actor, &invite) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => team_error(e),
    }
}

#[derive(Deserialize)]
struct RoleBody {
    role: TeamRole,
}

async fn set_role(
    State(relay): State<Relay>,
    Path((id, member)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<RoleBody>,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.set_role(&id, &actor, &member, body.role) {
        Ok(m) => Json(m).into_response(),
        Err(e) => team_error(e),
    }
}

async fn remove_member(
    State(relay): State<Relay>,
    Path((id, member)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.remove_member(&id, &actor, &member) {
        Ok(_) => {
            // Their open streams and tickets to team machines (and to the
            // machines they brought, for everyone else) end now; other
            // members and owners keep theirs (CUA-1417).
            relay.reauthorize(None);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => team_error(e),
    }
}

async fn request_device(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay
        .teams
        .request_approval(&id, &actor, &device_name(&relay, device.as_deref()))
    {
        Ok(d) => Json(d).into_response(),
        Err(e) => team_error(e),
    }
}

async fn decide(
    relay: Relay,
    id: String,
    device_id: String,
    headers: HeaderMap,
    approve: bool,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    match relay.teams.decide_device(&id, &actor, &device_id, approve) {
        Ok(d) => {
            if !approve {
                // The denied device's account loses its streams and tickets
                // on the team's machines; it reconnects from an approved
                // device.
                if let Ok((t, _)) = relay.teams.get_for(&id, &who.account) {
                    relay.cut_account(&d.account, Some(&t.machines));
                }
            }
            Json(d).into_response()
        }
        Err(e) => team_error(e),
    }
}

async fn approve(
    State(relay): State<Relay>,
    Path((id, device)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    decide(relay, id, device, headers, true).await
}

async fn deny(
    State(relay): State<Relay>,
    Path((id, device)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    decide(relay, id, device, headers, false).await
}

#[derive(Deserialize)]
struct MachineBody {
    machine: String,
    /// The OS of the Space, as the adding client knows it.
    #[serde(default)]
    os: Option<String>,
}

async fn add_machine(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<MachineBody>,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    // Machines not visible to the caller are indistinguishable from
    // missing ones.
    let Some(record) = relay
        .directory
        .get(&body.machine)
        .filter(|r| r.role_of(&who).is_some())
    else {
        return error(StatusCode::NOT_FOUND, "no such machine");
    };
    let host = record.host.as_deref().and_then(|h| relay.directory.get(h));
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    let os = body.os.as_deref().map(str::trim).filter(|o| !o.is_empty());
    match relay
        .teams
        .add_machine(&id, &actor, &record, host.as_ref(), os)
    {
        Ok(t) => Json(team_view(&relay, &t, &who.account, device.as_deref())).into_response(),
        Err(e) => team_error(e),
    }
}

async fn remove_machine(
    State(relay): State<Relay>,
    Path((id, machine)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (who, device) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let actor = Actor {
        who: &who,
        device: device.as_deref(),
    };
    let owner = relay.directory.get(&machine).map(|r| r.owner.id);
    match relay
        .teams
        .remove_machine(&id, &actor, &machine, owner.as_deref())
    {
        Ok(t) => {
            // The machine (and Spaces on it, when it is a host) leaves the
            // team: members lose it, its owner keeps it.
            relay.reauthorize(None);
            Json(team_view(&relay, &t, &who.account, device.as_deref())).into_response()
        }
        Err(e) => team_error(e),
    }
}

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default)]
    limit: Option<usize>,
}

async fn audit(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    Query(q): Query<AuditQuery>,
    headers: HeaderMap,
) -> Response {
    let (who, _) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match relay
        .teams
        .audit(&id, &who.account, q.limit.unwrap_or(100).clamp(1, 5000))
    {
        Ok(events) => Json(serde_json::json!({ "events": events })).into_response(),
        Err(e) => team_error(e),
    }
}

/// What an invite link opens in a browser: how to use it. It names no
/// team and checks nothing (joining needs a signed-in, enrolled device).
async fn join_page(Path(code): Path<String>) -> Response {
    let code: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(200)
        .collect();
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
<title>Join a team on Cua Spaces</title>\
<body style=\"font-family:system-ui;max-width:36em;margin:3em auto;padding:0 1em\">\
<h1>Join a team on Cua Spaces</h1>\
<p>Open the Cua Spaces app, go to <b>Teams</b> and choose <b>Join with a link</b>, then paste this page's address. Or, in a terminal:</p>\
<pre style=\"white-space:pre-wrap;word-break:break-all\">cua team join {code}</pre>\
<p>After you join, a team admin approves your device before you can open team machines.</p></body>"
    );
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

/// For [`TeamAudit`] consumers (re-exported for clients).
pub type AuditEntry = TeamAudit;
