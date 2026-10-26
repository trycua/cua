// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The hosted relay's side of the Spaces API (feature `hosted`): the
//! `/internal/v1/*` routes it serves (membership pushes, record owners,
//! "Shared with you", account erasure), the share-invite notices it sends,
//! and the Teams routes ([`crate::team_api`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::auth_log::{self, Reason, Route};
use crate::directory::MachineRecord;
use crate::meter::Meter;
use crate::server::{text, Relay, RelayConfig};
use crate::storage::Storage;

/// The hosted relay's command-line settings (flattened into `cua-relay`'s).
#[derive(clap::Args, Debug)]
pub struct HostedArgs {
    /// Account mode: meter live sessions and relay hours against the plan
    /// allowances (`on` / `off`). Measure and prompt only, never refuses.
    #[arg(long, env = "CUA_RELAY_METERING", default_value = "on")]
    pub metering: String,
    /// Allowances per plan: `plan=SESSIONS/HOURS,…` (live sessions at a
    /// time, relay hours a month; per seat, pooled, for a team plan).
    #[arg(long, env = "CUA_RELAY_ALLOWANCES", default_value = crate::meter::DEFAULT_ALLOWANCES)]
    pub allowances: String,
    /// Plans whose live-session limit is enforced: a session past it is
    /// refused (429). Other plans stay measure and prompt only.
    #[arg(long, env = "CUA_RELAY_ENFORCE_PLANS", default_value = crate::meter::DEFAULT_ENFORCE_PLANS)]
    pub enforce_plans: String,
    /// Where the allowance prompt links (pricing page or a Payment Link).
    #[arg(long, env = "CUA_RELAY_UPGRADE_URL", default_value = crate::meter::DEFAULT_UPGRADE_URL)]
    pub upgrade_url: String,
    /// Usage totals file (default: `<state file>.usage.json`).
    #[arg(long, env = "CUA_RELAY_USAGE_FILE")]
    pub usage_file: Option<std::path::PathBuf>,
    /// Plans file: per-user plans and teams (reloaded when it changes).
    #[arg(long, env = "CUA_RELAY_PLANS_FILE")]
    pub plans_file: Option<std::path::PathBuf>,
    /// Seconds a stream must stay open before its session counts as live.
    #[arg(long, env = "CUA_RELAY_LIVE_SESSION_AFTER_SECS", default_value_t = 10)]
    pub live_session_after_secs: u64,
    /// PostHog project key (public `phc_` capture key) for hourly usage
    /// and soft-limit events; unset: logs only.
    #[arg(long, env = "CUA_RELAY_POSTHOG_KEY", hide_env_values = true)]
    pub posthog_key: Option<String>,
    /// PostHog batch endpoint.
    #[arg(
        long,
        env = "CUA_RELAY_POSTHOG_ENDPOINT",
        default_value = "https://eu.i.posthog.com/batch/"
    )]
    pub posthog_endpoint: String,
    /// Where team access comes from: `file` (the relay's teams.json),
    /// `shadow` (teams.json decides; Keycloak Organizations + Spaces API are
    /// evaluated beside it and every mismatch is logged) or `org`.
    #[arg(long, env = "CUA_RELAY_TEAMS_SOURCE", default_value = "file")]
    pub teams_source: String,
    /// Spaces API base URL for team access lookups (in-cluster).
    #[arg(long, env = "CUA_RELAY_SPACES_API_URL")]
    pub spaces_api_url: Option<String>,
    /// Token endpoint for the relay's client-credentials token (default:
    /// the OIDC issuer's).
    #[arg(long, env = "CUA_RELAY_SPACES_API_TOKEN_URL")]
    pub spaces_api_token_url: Option<String>,
    /// The relay's service client id.
    #[arg(
        long,
        env = "CUA_RELAY_SPACES_API_CLIENT_ID",
        default_value = "cua-relay-svc"
    )]
    pub spaces_api_client_id: String,
    /// The relay's service client secret.
    #[arg(
        long,
        env = "CUA_RELAY_SPACES_API_CLIENT_SECRET",
        hide_env_values = true
    )]
    pub spaces_api_client_secret: Option<String>,
    /// Seconds a team access answer is cached.
    #[arg(long, env = "CUA_RELAY_ORG_CACHE_SECS", default_value_t = 60)]
    pub org_cache_secs: u64,
    /// Seconds a previous "allowed" team answer keeps being served while the
    /// Spaces API is unreachable (0 turns it off).
    #[arg(long, env = "CUA_RELAY_ORG_STALE_SECS", default_value_t = 900)]
    pub org_stale_secs: u64,
    /// Token the Spaces API presents on `/internal/v1/*` (membership push).
    #[arg(long, env = "CUA_RELAY_INTERNAL_TOKEN", hide_env_values = true)]
    pub internal_token: Option<String>,
    /// Where metered plans come from: `file` (CUA_RELAY_PLANS_FILE, or
    /// everyone on free) or `entitlements` (the Spaces API, CUA-1659).
    #[arg(long, env = "CUA_RELAY_PLANS_SOURCE", default_value = "file")]
    pub plans_source: String,
    /// Share computers only with people in the owner's team (CUA-1883):
    /// ask the Spaces API when a share is made and each time one is used,
    /// and clean up outside shares on start and daily. Needs
    /// CUA_RELAY_SPACES_API_URL.
    #[arg(long, env = "CUA_RELAY_TEAM_ONLY_SHARING", default_value = "off")]
    pub team_only_sharing: String,
    /// Refuse to start when the internal token is missing while the
    /// organization teams source or team-only sharing is on, or team-only
    /// sharing lacks the service client secret (`on` / `off`). Off: log a
    /// loud error and keep serving, so an existing deployment stays up. A
    /// token shorter than 32 characters is always refused.
    #[arg(
        long = "strict-internal-token",
        env = "CUA_RELAY_STRICT_INTERNAL_TOKEN",
        default_value = "off"
    )]
    pub strict_internal_token: String,
}

/// The shortest internal token the relay starts with.
pub const MIN_INTERNAL_TOKEN_LEN: usize = 32;

/// Headers a reverse proxy or ingress adds. The Spaces API reaches
/// `/internal/v1/*` in-cluster, straight to the Service, so it carries none.
const PROXY_HEADERS: [&str; 4] = [
    "x-forwarded-for",
    "x-forwarded-host",
    "forwarded",
    "x-real-ip",
];

fn on_off(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        other => Err(format!("{name}={other:?}: use on or off")),
    }
}

impl HostedArgs {
    /// The meter and organization access settings, checked.
    pub fn configs(
        self,
        oidc_enabled: bool,
        oidc_issuer: Option<&str>,
    ) -> Result<
        (
            crate::meter::MeterConfig,
            crate::org_access::OrgAccessConfig,
        ),
        String,
    > {
        let metering = on_off("CUA_RELAY_METERING", &self.metering)?;
        let meter = crate::meter::MeterConfig {
            enabled: metering && oidc_enabled,
            allowances: crate::meter::parse_allowances(&self.allowances)
                .map_err(|e| format!("CUA_RELAY_ALLOWANCES: {e}"))?,
            enforce_plans: crate::meter::parse_plan_set(&self.enforce_plans),
            upgrade_url: self.upgrade_url,
            usage_file: self.usage_file,
            plans_file: self.plans_file,
            live_after: Duration::from_secs(self.live_session_after_secs),
            clock: Default::default(),
            posthog: self
                .posthog_key
                .filter(|k| !k.trim().is_empty())
                .map(|key| crate::meter::PostHog {
                    endpoint: self.posthog_endpoint,
                    key,
                }),
        };
        let teams_source = crate::org_access::TeamsSource::parse(&self.teams_source)
            .map_err(|e| format!("CUA_RELAY_TEAMS_SOURCE={e}"))?;
        let spaces_api_url = self.spaces_api_url.filter(|u| !u.trim().is_empty());
        if teams_source == crate::org_access::TeamsSource::Org
            && (spaces_api_url.is_none() || oidc_issuer.is_none())
        {
            return Err(
                "CUA_RELAY_TEAMS_SOURCE=org needs CUA_RELAY_SPACES_API_URL and CUA_RELAY_OIDC_ISSUER"
                    .into(),
            );
        }
        if teams_source.evaluates_orgs() && spaces_api_url.is_none() {
            tracing::warn!("teams shadow mode without CUA_RELAY_SPACES_API_URL: every organization check is unavailable");
        }
        let plans_source = crate::plan_source::PlansSource::parse(&self.plans_source)
            .map_err(|e| format!("CUA_RELAY_PLANS_SOURCE={e}"))?;
        if plans_source == crate::plan_source::PlansSource::Entitlements && spaces_api_url.is_none()
        {
            return Err(
                "CUA_RELAY_PLANS_SOURCE=entitlements needs CUA_RELAY_SPACES_API_URL".into(),
            );
        }
        let team_only_sharing = on_off("CUA_RELAY_TEAM_ONLY_SHARING", &self.team_only_sharing)?;
        if team_only_sharing && spaces_api_url.is_none() {
            return Err("CUA_RELAY_TEAM_ONLY_SHARING needs CUA_RELAY_SPACES_API_URL".into());
        }
        // A token that is set but shorter than the minimum is always refused.
        // A missing token or credential logs a loud error and the relay keeps
        // serving, so deployments whose secrets are optional stay up;
        // `--strict-internal-token` makes those fatal too.
        let client_secret = self.spaces_api_client_secret.filter(|s| !s.is_empty());
        let internal_token = self.internal_token.filter(|t| !t.is_empty());
        let mut problems: Vec<String> = Vec::new();
        if let Some(t) = &internal_token {
            if t.chars().count() < MIN_INTERNAL_TOKEN_LEN {
                return Err(format!(
                    "CUA_RELAY_INTERNAL_TOKEN is too short: use at least {MIN_INTERNAL_TOKEN_LEN} characters (for example `openssl rand -hex 32`)"
                ));
            }
        }
        // The organization source relies on the Spaces API's membership
        // pushes (guarded by the internal token) to cut removed members at once.
        if teams_source == crate::org_access::TeamsSource::Org && internal_token.is_none() {
            problems.push(
                "CUA_RELAY_TEAMS_SOURCE=org needs CUA_RELAY_INTERNAL_TOKEN (at least 32 characters)"
                    .into(),
            );
        }
        if team_only_sharing && internal_token.is_none() {
            problems.push(
                "CUA_RELAY_TEAM_ONLY_SHARING=on needs CUA_RELAY_INTERNAL_TOKEN (at least 32 characters)"
                    .into(),
            );
        }
        // Team-only sharing also asks the Spaces API with the relay's own
        // service credentials; without them every check fails closed.
        if team_only_sharing && client_secret.is_none() {
            problems.push(
                "CUA_RELAY_TEAM_ONLY_SHARING=on needs CUA_RELAY_SPACES_API_CLIENT_SECRET".into(),
            );
        }
        let strict = on_off(
            "CUA_RELAY_STRICT_INTERNAL_TOKEN",
            &self.strict_internal_token,
        )?;
        if strict && !problems.is_empty() {
            return Err(problems.join("; "));
        }
        for p in &problems {
            tracing::error!(
                "{p}. The relay keeps serving; set CUA_RELAY_STRICT_INTERNAL_TOKEN=on to refuse to start instead"
            );
        }
        let org_access = crate::org_access::OrgAccessConfig {
            source: teams_source,
            api_url: spaces_api_url,
            token_url: self
                .spaces_api_token_url
                .filter(|u| !u.is_empty())
                .or_else(|| {
                    oidc_issuer.map(|i| {
                        format!("{}/protocol/openid-connect/token", i.trim_end_matches('/'))
                    })
                }),
            client_id: self.spaces_api_client_id,
            client_secret,
            ttl: Some(Duration::from_secs(self.org_cache_secs.clamp(1, 300))),
            stale_window: Some(Duration::from_secs(self.org_stale_secs.min(900))),
            internal_token,
            plans_source,
            team_only_sharing,
            sweep_every: None,
            ..Default::default()
        };
        tracing::info!(teams_source = teams_source.as_str(), "team access source");
        Ok((meter, org_access))
    }
}

/// The Teams routes (retired with Keycloak Organizations) and the
/// `/internal/v1/*` routes.
pub(crate) fn routes(relay: &Relay) -> Router<Relay> {
    Router::new()
        .merge(
            if relay.org.source() == crate::org_access::TeamsSource::Org {
                crate::team_api::gone_routes()
            } else {
                crate::team_api::routes()
            },
        )
        .route(
            "/internal/v1/membership-changed",
            axum::routing::post(membership_changed),
        )
        .route("/internal/v1/records/{id}", get(internal_record))
        .route("/internal/v1/shared", get(internal_shared))
        .route("/internal/v1/devices/{id}", get(internal_device))
        .route(
            "/internal/v1/accounts/{sub}",
            axum::routing::delete(crate::account_erase::erase),
        )
}

/// The meter, teams, organization access and plan source of a hosted relay.
pub(crate) struct HostedState {
    pub meter: Arc<Meter>,
    pub teams: Arc<crate::teams::Teams>,
    pub org: Arc<crate::org_access::OrgAccess>,
    pub plans: Option<crate::plan_source::EntitlementPlans>,
}

/// Opens [`HostedState`] next to the relay's state file.
pub(crate) fn open(config: &RelayConfig, storage: &Arc<Storage>) -> std::io::Result<HostedState> {
    let mut meter_config = config.meter.clone();
    let entitlements =
        config.org_access.plans_source == crate::plan_source::PlansSource::Entitlements;
    if entitlements && meter_config.plans_file.take().is_some() {
        tracing::warn!("plans come from the Spaces API's entitlements; the plans file is ignored");
    }
    if meter_config.usage_file.is_none() {
        meter_config.usage_file = config
            .state_file
            .as_ref()
            .map(|p| p.with_extension("usage.json"));
    }
    let meter = Arc::new(Meter::open_with(meter_config, storage.clone())?);
    // Teams live next to the directory (`<state>.teams.json`).
    let teams = match config
        .state_file
        .as_ref()
        .map(|p| p.with_extension("teams.json"))
    {
        Some(path) => crate::teams::Teams::open_with(path, storage.clone())?,
        None => crate::teams::Teams::in_memory(),
    };
    let teams = Arc::new(teams);
    // A paying team (in the plans file) pools its members' allowance.
    meter.set_team_directory(teams.clone());
    let org = Arc::new(crate::org_access::OrgAccess::new(config.org_access.clone()));
    let plans = entitlements.then(|| {
        let plans = crate::plan_source::EntitlementPlans::with_snapshot(
            org.clone(),
            None,
            config
                .state_file
                .as_ref()
                .map(|p| p.with_extension("plans.json")),
        );
        let weak = Arc::downgrade(&meter);
        plans.set_on_change(move |user| {
            if let Some(m) = weak.upgrade() {
                m.refresh_user(user);
            }
        });
        meter.set_plan_source(Arc::new(plans.clone()));
        plans
    });
    tracing::info!(
        plans_source = config.org_access.plans_source.as_str(),
        "relay plan source"
    );
    Ok(HostedState {
        meter,
        teams,
        org,
        plans,
    })
}

/// `POST /internal/v1/membership-changed {org, sub?}`: the Spaces API says
/// a member was removed or their role changed (every member when `sub` is
/// absent). Drops what the relay cached for them and re-checks the open
/// sessions on that organization's machines, so a removed member's are cut.
/// Needs the internal token; the ingress must not route `/internal/`.
async fn membership_changed(
    State(relay): State<Relay>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(r) = require_internal(&relay, &headers) {
        return r;
    }
    #[derive(serde::Deserialize)]
    struct Change {
        org: String,
        #[serde(default)]
        sub: Option<String>,
    }
    let Ok(change) = serde_json::from_slice::<Change>(&body) else {
        return text(
            StatusCode::BAD_REQUEST,
            "expected {\"org\": \"…\", \"sub\": \"…\"}",
        );
    };
    if change.org.is_empty() {
        return text(StatusCode::BAD_REQUEST, "org is required");
    }
    let machines = relay
        .org
        .membership_changed(&change.org, change.sub.as_deref().filter(|s| !s.is_empty()));
    tracing::info!(org = %change.org, sub = ?change.sub.as_deref().map(crate::ident::tag), machines = machines.len(), "membership changed");
    if relay.org.source() == crate::org_access::TeamsSource::Org && !machines.is_empty() {
        relay.reauthorize(Some(&machines));
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Checks the internal token the Spaces API presents on `/internal/v1/*`
/// (404 when none is configured, or when the request carries proxy headers).
#[allow(clippy::result_large_err)]
pub(crate) fn require_internal(relay: &Relay, headers: &HeaderMap) -> Result<(), Response> {
    // Defence in depth: a request that came through an ingress or proxy is
    // not the in-cluster Spaces API. Answer as if the route did not exist.
    if PROXY_HEADERS.iter().any(|h| headers.contains_key(*h)) {
        return Err(text(StatusCode::NOT_FOUND, "not found"));
    }
    let Some(expected) = relay.org.internal_token() else {
        return Err(text(StatusCode::NOT_FOUND, "not found"));
    };
    let ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| {
            use subtle::ConstantTimeEq as _;
            t.len() == expected.len() && bool::from(t.as_bytes().ct_eq(expected.as_bytes()))
        });
    if !ok {
        auth_log::rejected(headers, Route::Admin, Reason::InvalidAdminToken, None);
        return Err(text(StatusCode::UNAUTHORIZED, "invalid internal token"));
    }
    Ok(())
}

/// `GET /internal/v1/records/{id}`: who owns a relay record, so the Spaces
/// API binds a machine to an org only for its owner. 200 `{id, owner_sub,
/// platform: macos|linux|windows|unknown, kind: host|space}`, 404
/// `record_not_found`. Needs the internal token.
async fn internal_record(
    State(relay): State<Relay>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = require_internal(&relay, &headers) {
        return r;
    }
    let Some(record) = relay.directory.get(&id) else {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "record_not_found",
                "detail": format!("no relay record {id:?}"),
            })),
        )
            .into_response();
    };
    axum::Json(record_facts(&record)).into_response()
}

/// `GET /internal/v1/devices/{id}`: one client device, read-only, for the
/// in-cluster services that must know whether a device is enrolled and
/// not revoked. 200 `{id, account,
/// public_jwk: {kty: "EC", crv: "P-256", x, y}, state:
/// pending|enrolled|expired|revoked, enrolled, revoked}`; 404
/// `device_not_found`. Needs the internal token, like every
/// `/internal/v1/*` route (and the ingress never routes `/internal/`).
async fn internal_device(
    State(relay): State<Relay>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = require_internal(&relay, &headers) {
        return r;
    }
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "device_not_found",
                "detail": "no such device",
            })),
        )
            .into_response()
    };
    if id.len() > 64 || !id.starts_with(crate::devices::DEVICE_ID_PREFIX) {
        return not_found();
    }
    let Some(device) = relay.devices.get(&id) else {
        return not_found();
    };
    axum::Json(device_facts(&device, crate::assertion::now_secs())).into_response()
}

/// The issuer's view of a device (`/internal/v1/devices/{id}`): its key as a
/// JWK, so the issuer can match it against a device proof's `jwk` header.
pub(crate) fn device_facts(device: &crate::devices::DeviceRecord, now: u64) -> serde_json::Value {
    use crate::devices::DeviceState;
    let state = device.state(now);
    let public_jwk = crate::devices::decode_public_key(&device.public_key)
        .ok()
        .map(|raw| {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            serde_json::json!({
                "kty": "EC",
                "crv": "P-256",
                "x": b64.encode(&raw[1..33]),
                "y": b64.encode(&raw[33..65]),
            })
        });
    serde_json::json!({
        "id": device.id,
        "account": device.account,
        "public_jwk": public_jwk,
        "state": state,
        "enrolled": state == DeviceState::Enrolled,
        "revoked": state == DeviceState::Revoked,
    })
}

/// `GET /internal/v1/shared?sub=&email=&email_verified=`: what other
/// accounts share directly with this one, for the "Shared with you" list
/// on the web (CUA-1764). `shared` are the records the account reaches now
/// (by account id, or by email when `email_verified=true`). `pending`
/// counts shares to `email` that wait for it to be verified; their names
/// are not returned, since an unverified address proves nothing. Team
/// machines are not listed here. Needs the internal token.
async fn internal_shared(
    State(relay): State<Relay>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = require_internal(&relay, &headers) {
        return r;
    }
    let sub = q.get("sub").map(|s| s.trim()).unwrap_or_default();
    if sub.is_empty() {
        return text(StatusCode::BAD_REQUEST, "sub is required");
    }
    let email = q
        .get("email")
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| e.contains('@'));
    let verified = q.get("email_verified").is_some_and(|v| v == "true");
    axum::Json(shared_with(&relay, sub, email.as_deref(), verified)).into_response()
}

pub(crate) fn shared_with(
    relay: &Relay,
    sub: &str,
    email: Option<&str>,
    verified: bool,
) -> serde_json::Value {
    let who = crate::oidc::Identity {
        user: sub.to_owned(),
        account: sub.to_owned(),
        email: email.filter(|_| verified).map(str::to_owned),
        name: None,
        auth_time: None,
        mfa: false,
        orgs: Vec::new(),
    };
    let mut shared = Vec::new();
    for (record, role) in relay.directory.visible_to(&who) {
        if role == crate::directory::Role::Owner || !record.sharing {
            continue;
        }
        let host = record.host.as_deref().and_then(|h| relay.directory.get(h));
        shared.push(serde_json::json!({
            "id": record.id,
            "name": record.name,
            "kind": if record.host.is_some() { "space" } else { "host" },
            "host_name": host.map(|h| h.name),
            "role": role.as_str(),
            "online": relay.machine(&record.id).is_some(),
            "owner": {
                "id": record.owner.id,
                "name": record.owner.name,
                "email": record.owner.email,
            },
        }));
    }
    let pending = match email {
        Some(e) if !verified => relay
            .directory
            .all()
            .iter()
            .filter(|r| r.owner.id != sub && r.sharing)
            .filter(|r| {
                r.allow
                    .iter()
                    .chain(r.viewers.iter())
                    .any(|a| a.eq_ignore_ascii_case(e))
            })
            .count(),
        _ => 0,
    };
    serde_json::json!({ "shared": shared, "pending": pending })
}

/// The owner, platform and kind of a record (`/internal/v1/records/{id}`).
pub(crate) fn record_facts(record: &crate::directory::MachineRecord) -> serde_json::Value {
    use crate::teams::{META_HOST_OS, META_SPACE_OS};
    let os = |key: &str| record.meta.get(key).map(|v| v.trim().to_ascii_lowercase());
    let declared = if record.host.is_some() {
        os(META_SPACE_OS)
    } else {
        os(META_HOST_OS).or_else(|| os(META_SPACE_OS))
    };
    let platform = match declared.as_deref() {
        Some("macos" | "darwin" | "mac" | "osx") => "macos",
        Some("linux") => "linux",
        Some("windows" | "win32" | "win") => "windows",
        _ => "unknown",
    };
    serde_json::json!({
        "id": record.id,
        "owner_sub": record.owner.id,
        "platform": platform,
        "kind": if record.host.is_some() { "space" } else { "host" },
    })
}

/// A share by email is a pending invite until someone signs in with that
/// email verified (CUA-1764). Tell the Spaces API about each new one so it
/// emails the recipient; the relay itself sends no mail. Best effort: the
/// share stands whether or not the email goes out.
pub(crate) fn notify_email_invites(relay: &Relay, before: &MachineRecord, after: &MachineRecord) {
    if !relay.org.has_api() {
        return;
    }
    let invites = new_email_shares(before, after);
    if invites.is_empty() {
        return;
    }
    let org = relay.org.clone();
    let host_name = after
        .host
        .as_deref()
        .and_then(|h| relay.directory.get(h))
        .map(|h| h.name);
    let base = serde_json::json!({
        "machine_id": after.id,
        "machine_name": after.name,
        "kind": if after.host.is_some() { "space" } else { "host" },
        "host_name": host_name,
        "owner_sub": after.owner.id,
        "owner_name": after.owner.name,
        "owner_email": after.owner.email,
    });
    tokio::spawn(async move {
        for (email, role) in invites {
            let mut body = base.clone();
            body["email"] = email.into();
            body["role"] = role.into();
            match org.post_json("/internal/v1/share-invites", &body).await {
                Ok(s) if s.is_success() => {
                    tracing::info!(machine = %body["machine_id"], "share invite sent to the Spaces API")
                }
                Ok(s) => tracing::warn!(status = %s, "share invite: Spaces API refused"),
                Err(e) => tracing::warn!(error = %e, "share invite: Spaces API unreachable"),
            }
        }
    });
}

/// Emails newly added to a machine's allowlist or viewers, with the role
/// each gets (`viewer` wins, as in [`MachineRecord::role_of`]).
fn new_email_shares(before: &MachineRecord, after: &MachineRecord) -> Vec<(String, &'static str)> {
    let had = |e: &str| {
        before
            .allow
            .iter()
            .chain(before.viewers.iter())
            .any(|b| b.eq_ignore_ascii_case(e))
    };
    let mut out: Vec<(String, &'static str)> = Vec::new();
    for e in after.viewers.iter().chain(after.allow.iter()) {
        if !e.contains('@') || had(e) || out.iter().any(|(o, _)| o.eq_ignore_ascii_case(e)) {
            continue;
        }
        let role = if after.viewers.iter().any(|v| v.eq_ignore_ascii_case(e)) {
            "viewer"
        } else {
            "editor"
        };
        out.push((e.to_ascii_lowercase(), role));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        hosted: HostedArgs,
    }

    const ISSUER: &str = "https://auth.example.com/realms/test";
    const GOOD: &str = "0123456789abcdef0123456789abcdef";

    fn configs(args: &[&str]) -> Result<(), String> {
        let mut argv = vec!["cua-relay"];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(argv).expect("args");
        cli.hosted.configs(true, Some(ISSUER)).map(|_| ())
    }

    fn strict(args: &[&str]) -> Result<(), String> {
        let mut all = vec!["--strict-internal-token", "on"];
        all.extend_from_slice(args);
        configs(&all)
    }

    #[test]
    fn a_short_internal_token_is_refused_in_any_mode_without_echoing_it() {
        let short = "abcdefghijklmnopqrstuvwxyz01234";
        assert_eq!(short.len(), 31);
        for run in [configs, strict] {
            let err = run(&["--internal-token", short]).unwrap_err();
            assert!(
                err.contains("CUA_RELAY_INTERNAL_TOKEN") && err.contains("32"),
                "{err}"
            );
            assert!(!err.contains(short), "{err}");
            run(&["--internal-token", GOOD]).unwrap();
            // Unset or empty is allowed on its own (the routes answer 404).
            run(&[]).unwrap();
            run(&["--internal-token", ""]).unwrap();
        }
    }

    #[test]
    fn a_missing_token_with_the_org_source_logs_and_starts_but_strict_mode_refuses() {
        let org = [
            "--teams-source",
            "org",
            "--spaces-api-url",
            "http://api.test",
        ];
        configs(&org).unwrap();
        let err = strict(&org).unwrap_err();
        assert!(err.contains("CUA_RELAY_INTERNAL_TOKEN"), "{err}");
        let mut with = org.to_vec();
        with.extend(["--internal-token", GOOD]);
        strict(&with).unwrap();
        configs(&with).unwrap();
        let mut short = org.to_vec();
        short.extend(["--internal-token", "tooshort"]);
        assert!(configs(&short).unwrap_err().contains("too short"));
        assert!(strict(&short).unwrap_err().contains("too short"));
    }

    #[test]
    fn team_only_sharing_needs_a_token_and_the_services_own_credentials_in_strict_mode() {
        let on = [
            "--team-only-sharing",
            "on",
            "--spaces-api-url",
            "http://api.test",
        ];
        let err = strict(&on).unwrap_err();
        assert!(
            err.contains("INTERNAL_TOKEN") && err.contains("CLIENT_SECRET"),
            "{err}"
        );
        let mut with = on.to_vec();
        with.extend([
            "--internal-token",
            GOOD,
            "--spaces-api-client-secret",
            "s3cret-value",
        ]);
        strict(&with).unwrap();
        configs(&on).unwrap();
    }

    #[test]
    fn strict_mode_value_is_checked() {
        assert!(configs(&["--strict-internal-token", "maybe"]).is_err());
    }
}
