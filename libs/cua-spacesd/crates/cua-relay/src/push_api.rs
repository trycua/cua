// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Push notifications for the phone app (account mode), see [`cua_push`]:
//!
//! - `GET    /v1/push`                 whether push works on this relay, the
//!   account's phones (never their tokens) and its preferences.
//! - `PUT    /v1/push/registrations/{device_id}`  register this phone (the
//!   device id it enrolled with) with the token the system gave the app,
//!   the alerts it takes and its language; registering again replaces it.
//! - `DELETE /v1/push/registrations/{device_id}`  unregister it.
//! - `GET    /v1/push/preferences`     which alerts the account wants.
//! - `PUT    /v1/push/preferences`     change them (only the fields given).
//! - `POST   /v1/push/events`          a daemon of the account reports an
//!   event (an approval waits, a task finished or failed, an agent is
//!   blocked); the relay sends an alert to the account's phones.
//!
//! Every call carries the account token (a computer token is refused), and
//! the phone calls carry an enrolled device's session like the rest of the
//! account API. An event only ever reaches the phones of the account that
//! sent it, and the alert is built here from fixed phrases: nothing the
//! daemon sends beyond the event's kind, ids and the agent's short name
//! reaches a phone.
//!
//! The routes answer 404 until the relay is started with push on
//! (`CUA_RELAY_PUSH=on`): a relay deployed without it behaves as before.

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use cua_push::registry::{PreferencesPatch, RegistryError};
use cua_push::{Platform, PushEvent, PushHub};
use serde::Deserialize;

use crate::auth_log::{self, Reason, Route};
use crate::oidc::Identity;
use crate::server::{account_token, bearer, Relay};

/// Request bodies here are tiny.
const BODY_LIMIT: usize = 8 * 1024;
/// Events one account may report in a window.
pub const EVENTS_PER_WINDOW: u32 = 120;
const WINDOW: Duration = Duration::from_secs(60);

/// The relay's push setup.
#[derive(Clone)]
pub struct PushConfig {
    pub hub: Arc<PushHub>,
    phone_clients: Arc<Vec<String>>,
    /// Phones are served (`CUA_RELAY_PHONES`, on unless turned off).
    phones: bool,
    limiter: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
}

impl PushConfig {
    pub fn new(hub: Arc<PushHub>) -> PushConfig {
        PushConfig {
            hub,
            phone_clients: Arc::new(vec![crate::phone::DEFAULT_PHONE_CLIENT.to_owned()]),
            phones: true,
            limiter: Arc::default(),
        }
    }

    /// The OAuth client ids whose tokens make a device a phone
    /// (`CUA_RELAY_PHONE_CLIENTS`; default: the phone app's).
    pub fn with_phone_clients(mut self, clients: Vec<String>) -> PushConfig {
        self.phone_clients = Arc::new(clients);
        self
    }

    /// Whether phones are served. Off, every phone is refused (the rest of
    /// the phone support stays on).
    pub fn with_phones(mut self, on: bool) -> PushConfig {
        self.phones = on;
        self
    }

    /// Whether phones are served.
    pub fn phones(&self) -> bool {
        self.phones
    }

    /// The phone client ids.
    pub fn phone_clients(&self) -> &[String] {
        &self.phone_clients
    }

    /// Whether `account` may report another event now.
    fn allow(&self, account: &str) -> bool {
        let mut l = self.limiter.lock().expect("push limiter");
        let now = Instant::now();
        l.retain(|_, (at, _)| now.duration_since(*at) < WINDOW);
        let slot = l.entry(account.to_string()).or_insert((now, 0));
        slot.1 += 1;
        slot.1 <= EVENTS_PER_WINDOW
    }
}

impl std::fmt::Debug for PushConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PushConfig({})", self.hub.provider_name())
    }
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/push", get(status))
        .route(
            "/v1/push/registrations/{device_id}",
            put(register).delete(unregister),
        )
        .route(
            "/v1/push/preferences",
            get(preferences).put(set_preferences).patch(set_preferences),
        )
        .route("/v1/push/events", post(event))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({"error": message.into(), "code": code})),
    )
        .into_response()
}

fn registry_error(e: RegistryError) -> Response {
    let status = match &e {
        RegistryError::Invalid(_) => StatusCode::BAD_REQUEST,
        RegistryError::Limit(_) => StatusCode::CONFLICT,
        RegistryError::NotFound => StatusCode::NOT_FOUND,
        RegistryError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, cua_push::hub::error_code(&e), e.to_string())
}

fn config(relay: &Relay) -> Result<&PushConfig, Response> {
    relay.config.push.as_ref().ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            "not_found",
            "push notifications are not set up on this relay",
        )
    })
}

/// The account behind the request (computer tokens are refused).
async fn account(relay: &Relay, headers: &HeaderMap) -> Result<Identity, Response> {
    let Some(oidc) = &relay.config.oidc else {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "account mode is not enabled on this relay",
        ));
    };
    if bearer(headers, "authorization")
        .is_some_and(|t| t.starts_with(crate::directory::MACHINE_TOKEN_PREFIX))
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "machine_token",
            "computer tokens cannot manage push",
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

/// The account, from a phone: an enrolled device (or the grace period).
async fn phone(relay: &Relay, headers: &HeaderMap) -> Result<Identity, Response> {
    let who = account(relay, headers).await?;
    crate::device_api::gate(relay, crate::device_api::session_header(headers), &who)
        .map_err(|m| error(StatusCode::FORBIDDEN, "device_forbidden", m))?;
    Ok(who)
}

async fn status(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let cfg = match config(&relay) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let who = match phone(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    Json(serde_json::json!({
        "provider": cfg.hub.provider_name(),
        // Alerts are sent only when a provider is set up; phones may
        // register before that and start receiving the moment it is.
        "configured": cfg.hub.provider_name() != "none",
        "phones": cfg.hub.registry.list(&who.account),
        "preferences": cfg.hub.registry.preferences(&who.account),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct RegisterBody {
    platform: Platform,
    /// `apns` (the only provider built for iOS).
    #[serde(default)]
    provider: Option<String>,
    token: String,
    /// The app's bundle id.
    #[serde(default, alias = "app")]
    app_id: String,
    /// `sandbox` (a development build) or `production`.
    #[serde(default)]
    env: Option<String>,
    /// `needs_you` and `done`; absent: both.
    #[serde(default)]
    categories: Option<Vec<String>>,
    #[serde(default)]
    locale: String,
    #[serde(default)]
    name: String,
}

/// The phone behind the request: a path's device id must be the one the
/// request's device session is of (an enrolled phone registers only itself).
fn own_device(
    relay: &Relay,
    headers: &HeaderMap,
    who: &Identity,
    device_id: &str,
) -> Result<bool, Response> {
    let gate = crate::device_api::gate(relay, crate::device_api::session_header(headers), who)
        .map_err(|m| error(StatusCode::FORBIDDEN, "device_forbidden", m))?;
    match gate.device() {
        Some(d) if d != device_id => Err(error(
            StatusCode::FORBIDDEN,
            "other_device",
            "a phone registers only itself",
        )),
        other => Ok(other.is_some()),
    }
}

async fn register(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(body): Json<RegisterBody>,
) -> Response {
    let cfg = match config(&relay) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let bound = match own_device(&relay, &headers, &who, &device_id) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.provider.as_deref().is_some_and(|p| p != "apns") {
        return error(StatusCode::BAD_REQUEST, "invalid", "provider: apns");
    }
    let sandbox = match body.env.as_deref() {
        None | Some("production") => false,
        Some("sandbox") => true,
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                "env: sandbox or production",
            )
        }
    };
    // A token another account's enrolled phone still holds is not moved: that
    // would silently turn the other account's alerts off. It moves when the
    // other phone's device is gone (revoked, expired) or never was one.
    if let Some((holder, holder_device)) = cfg.hub.registry.holder(&body.token) {
        if holder != who.account && relay.devices.is_enrolled(&holder_device) {
            return error(
                StatusCode::CONFLICT,
                "token_in_use",
                "this phone is registered to another account; sign out of it there first",
            );
        }
    }
    match cfg.hub.registry.register(
        &who.account,
        &device_id,
        body.platform,
        &body.token,
        &body.app_id,
        sandbox,
        &body.name,
        body.categories.as_deref(),
        &body.locale,
        bound,
    ) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => registry_error(e),
    }
}

async fn unregister(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> Response {
    let cfg = match config(&relay) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Err(r) = own_device(&relay, &headers, &who, &device_id) {
        return r;
    }
    match cfg.hub.registry.unregister(&who.account, &device_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => registry_error(e),
    }
}

async fn preferences(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let cfg = match config(&relay) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let who = match phone(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    Json(cfg.hub.registry.preferences(&who.account)).into_response()
}

async fn set_preferences(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(patch): Json<PreferencesPatch>,
) -> Response {
    let cfg = match config(&relay) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let who = match phone(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    match cfg.hub.registry.set_preferences(&who.account, &patch) {
        Ok(p) => Json(p).into_response(),
        Err(e) => registry_error(e),
    }
}

/// Whether `id` is a computer of `account`, or a Space hosted by one.
fn relay_owns(relay: &Relay, account: &str, id: &str) -> bool {
    let owned = |id: &str| {
        relay
            .directory
            .get(id)
            .map(|r| (r.owner.id == account, r.host.clone()))
    };
    match owned(id) {
        Some((true, _)) => true,
        Some((false, Some(host))) => owned(&host).is_some_and(|(mine, _)| mine),
        _ => false,
    }
}

async fn event(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(ev): Json<PushEvent>,
) -> Response {
    let cfg = match config(&relay) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let who = match account(&relay, &headers).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    // An event comes from a computer of the account (its daemon), proved by
    // that computer's device session: an account token alone cannot make the
    // account's phones ring, and a phone never reports. The alert can only
    // reach this account's own phones.
    match crate::device_api::gate(&relay, crate::device_api::session_header(&headers), &who) {
        Ok(crate::device_api::Gate::Device(device)) => {
            if relay.devices.is_phone(&device) {
                return error(
                    StatusCode::FORBIDDEN,
                    "not_a_computer",
                    "a phone does not report events",
                );
            }
        }
        // Enrollment is off on this relay, or in its migration grace.
        Ok(_) => {}
        Err(message) => return error(StatusCode::FORBIDDEN, "device_forbidden", message),
    }
    if let Err(e) = ev.check() {
        return error(StatusCode::BAD_REQUEST, "invalid", e.to_string());
    }
    // The Space the alert is about is the account's own: a computer of it, or
    // a Space one of its computers provides.
    if let Some(space) = ev.space.as_deref() {
        // Only a Space of the relay can be opened from a phone: `relay:<id>`, the id
        // a machine id (no other spelling, no other prefix, so no way around the check).
        let Some(id) = space
            .strip_prefix("relay:")
            .filter(|id| crate::valid_machine_id(id))
        else {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                "space: relay:<id> of one of your account's Spaces",
            );
        };
        if !relay_owns(&relay, &who.account, id) {
            return error(
                StatusCode::FORBIDDEN,
                "not_yours",
                "this Space is not one of your account's",
            );
        }
    }
    if !cfg.allow(&who.account) {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many events; try again in a minute",
        );
    }
    Json(cfg.hub.notify(&who.account, &ev).await).into_response()
}
