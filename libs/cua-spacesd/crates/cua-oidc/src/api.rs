// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The issuer's HTTP API. See the crate docs for the routes, and
//! [`ApiError`] for the error shape.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::account::{valid_account_id, AccountIds};
use crate::devices::DeviceDirectory;
use crate::keycloak::{KeycloakValidator, User};
use crate::model::{
    validate_identifiers, ApprovedBy, AuditRecord, Cloud, Connection, CreatedBy, Limits, Purpose,
    SealedToken, Status, MAX_SEALED_BYTES,
};
use crate::proof::{Expect, Proof, PROOF_HEADER, PROOF_SKEW_SECS};
use crate::registry::{Ledger, Registry, StoreError};
use crate::signer::KeySet;
use crate::token::{issuer_url, new_jti, Claims, TOKEN_TTL_SECS};

/// The claims the discovery document lists.
pub const CLAIMS_SUPPORTED: &[&str] = &[
    "iss",
    "sub",
    "aud",
    "iat",
    "nbf",
    "exp",
    "jti",
    "cua_conn",
    "cua_device",
    "cua_agent",
    "cua_purpose",
];

/// The AWS CloudFormation template. There is one copy, cua-byoc's
/// `templates/aws/cua-spaces.yaml`: a parameterised stack (`CuaAccount`,
/// `IssuerHost`) that the CloudFormation quick-create link fills in from a
/// public S3 copy. Served here with one account's values as the defaults.
pub const AWS_TEMPLATE: &str =
    include_str!("../../../../cua/crates/cua-byoc/templates/aws/cua-spaces.yaml");

/// Everything the issuer needs.
#[derive(Clone)]
pub struct Issuer {
    /// Public base URL, no trailing slash (`https://oidc.cua.ai`).
    pub base_url: String,
    /// Signing and published keys.
    pub keys: KeySet,
    /// Keycloak access-token validation.
    pub keycloak: Arc<KeycloakValidator>,
    /// Account id derivation.
    pub accounts: AccountIds,
    /// The relay's device registry.
    pub devices: Arc<dyn DeviceDirectory>,
    /// Connections, audit log, replay cache.
    pub registry: Arc<dyn Registry>,
    /// This month's spend per connection.
    pub ledger: Arc<dyn Ledger>,
}

/// A typed refusal: `{"error": code, "message": plain words}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// HTTP status.
    pub status: StatusCode,
    /// Stable machine-readable code.
    pub code: &'static str,
    /// What happened, in plain words.
    pub message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn store(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => Self::not_found(),
            StoreError::Exists(id) => Self::new(
                StatusCode::CONFLICT,
                "connection_exists",
                format!("this cloud is already linked as connection {id}"),
            ),
            StoreError::Conflict => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "the connection changed at the same time; try again",
            ),
            StoreError::Unavailable(e) => {
                tracing::error!(%e, "storage failed");
                Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "Cua's sign-in for clouds can't reach its database; try again shortly",
                )
            }
        }
    }

    fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "connection_not_found",
            "no such connection on this account",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut r = (
            self.status,
            Json(serde_json::json!({"error": self.code, "message": self.message})),
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer, DPoP algs=\"ES256\""),
            );
        }
        r
    }
}

/// The signed-in caller.
struct Caller {
    user: User,
    acct: String,
    access_token: String,
}

/// A caller with a verified device proof.
struct DeviceCaller {
    caller: Caller,
    proof: Proof,
}

impl Issuer {
    /// The HTTP router.
    pub fn router(self) -> Router {
        Router::new()
            .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
            .route("/readyz", get(readyz))
            .route("/.well-known/jwks.json", get(jwks))
            .route("/a/{acct}/.well-known/jwks.json", get(account_jwks))
            .route("/a/{acct}/.well-known/openid-configuration", get(discovery))
            .route("/v1/token", post(token))
            .route("/v1/account", get(account))
            .route("/v1/connections", post(create).get(list))
            .route("/v1/connections/{id}", get(get_one).delete(delete))
            .route("/v1/connections/{id}/approve", post(approve))
            .route("/v1/connections/{id}/deny", post(deny))
            .route("/v1/connections/{id}/limits", post(limits))
            .route("/v1/connections/{id}/revoke", post(revoke))
            .route("/v1/connections/{id}/spend", post(spend))
            .route("/v1/connections/{id}/sealed-token", post(sealed_token))
            .route("/v1/audit", get(audit))
            .route("/v1/templates/aws.yaml", get(aws_template))
            .with_state(Arc::new(self))
    }

    fn bearer(headers: &HeaderMap) -> Option<&str> {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }

    async fn caller(&self, headers: &HeaderMap) -> Result<Caller, ApiError> {
        let token = Self::bearer(headers).ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "sign in to Cua first (no access token)",
            )
        })?;
        let user = self.keycloak.validate(token).await.map_err(|reason| {
            tracing::info!(%reason, "access token refused");
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                format!("the Cua sign-in was refused: {reason}"),
            )
        })?;
        let acct = self.accounts.derive(&user.account);
        Ok(Caller {
            user,
            acct,
            access_token: token.to_owned(),
        })
    }

    /// Verifies the device proof of a request: signature, freshness, the
    /// request it is bound to, the claims it must carry, no replay, and an
    /// enrolled, non-revoked device of the caller's account. `agent`:
    /// `Some(x)` when the proof must name agent `x` (or none), `None` to
    /// take the proof's.
    #[allow(clippy::too_many_arguments)]
    async fn device_proof(
        &self,
        caller: &Caller,
        headers: &HeaderMap,
        method: &Method,
        uri: &Uri,
        connection: &str,
        purpose: &str,
        agent: Option<Option<&str>>,
    ) -> Result<Proof, ApiError> {
        let invalid = |m: String| ApiError::new(StatusCode::UNAUTHORIZED, "invalid_proof", m);
        let raw = headers
            .get(PROOF_HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| invalid("this request needs a device proof (DPoP header)".into()))?;
        let htu = format!("{}{}", self.base_url, uri.path());
        let now = crate::now_secs();
        let proof = crate::proof::verify(
            raw,
            Expect {
                htm: method.as_str(),
                htu: &htu,
                access_token: &caller.access_token,
                now,
            },
        )
        .map_err(invalid)?;
        let agent_matches = match agent {
            Some(want) => proof.agent.as_deref() == want.filter(|a| !a.is_empty()),
            None => true,
        };
        if proof.connection != connection || proof.purpose != purpose || !agent_matches {
            return Err(invalid(
                "the device proof was signed for another connection, purpose or agent".into(),
            ));
        }
        let key = format!("{}:{}", proof.device, proof.jti);
        if !self
            .registry
            .claim_proof(&key, now + 2 * PROOF_SKEW_SECS + 60)
            .await
            .map_err(ApiError::store)?
        {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "proof_replayed",
                "this device proof was already used; sign a new one",
            ));
        }
        let not_enrolled = || {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "device_not_enrolled",
                "this device is not enrolled on your Cua account; sign in on it again",
            )
        };
        let info = self.devices.device(&proof.device).await.map_err(|e| {
            tracing::error!(%e, "device lookup failed");
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "device_directory_unavailable",
                "Cua can't check this device right now; try again shortly",
            )
        })?;
        let Some(info) = info else {
            return Err(not_enrolled());
        };
        if info.account != caller.user.account {
            tracing::warn!(acct = %caller.acct, device = %proof.device, "device proof from another account's device");
            return Err(not_enrolled());
        }
        if info.revoked {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "device_revoked",
                "this device was removed from your Cua account",
            ));
        }
        let same_key = info
            .public_jwk
            .as_ref()
            .and_then(|k| k.uncompressed())
            .zip(proof.jwk.uncompressed())
            .is_some_and(|(a, b)| a == b);
        if !info.enrolled || !same_key {
            return Err(not_enrolled());
        }
        Ok(proof)
    }

    async fn device_caller(
        &self,
        headers: &HeaderMap,
        method: &Method,
        uri: &Uri,
        connection: &str,
        action: &str,
    ) -> Result<DeviceCaller, ApiError> {
        let caller = self.caller(headers).await?;
        // The agent is whatever the signed proof says (the daemon stamps
        // it); it decides the approval rules.
        let proof = self
            .device_proof(&caller, headers, method, uri, connection, action, None)
            .await;
        match proof {
            Ok(proof) => Ok(DeviceCaller { caller, proof }),
            Err(e) => {
                let mut r = AuditRecord::new(&caller.acct, "request_refused");
                r.user = Some(caller.user.user.clone());
                r.connection = Some(connection.to_owned()).filter(|c| !c.is_empty());
                r.purpose = Some(action.to_owned());
                r.code = Some(e.code.to_owned());
                self.audit_best_effort(&r).await;
                Err(e)
            }
        }
    }

    /// `c` as JSON with this month's account-wide spend
    /// (`spent_month_usd`); left out when the ledger can't say.
    async fn with_spend(&self, c: &Connection) -> serde_json::Value {
        let mut v = serde_json::to_value(c).unwrap_or_default();
        match self.ledger.spent_month(&c.account, &c.id).await {
            Ok(usd) => v["spent_month_usd"] = serde_json::json!(usd),
            Err(e) => tracing::warn!(%e, connection = %c.id, "spend not read"),
        }
        v
    }

    async fn audit_best_effort(&self, r: &AuditRecord) {
        if let Err(e) = self.registry.append_audit(r).await {
            tracing::error!(%e, kind = %r.kind, "audit write failed");
        }
    }

    /// Writes an audit row; a change is refused when it can't be recorded.
    async fn audit_required(&self, r: &AuditRecord) -> Result<(), ApiError> {
        self.registry.append_audit(r).await.map_err(ApiError::store)
    }

    /// Reads, changes and stores a connection, retrying a concurrent change.
    async fn modify<F>(&self, acct: &str, id: &str, mut change: F) -> Result<Connection, ApiError>
    where
        F: FnMut(&mut Connection) -> Result<(), ApiError>,
    {
        for _ in 0..5 {
            let mut c = self
                .registry
                .get(acct, id)
                .await
                .map_err(ApiError::store)?
                .ok_or_else(ApiError::not_found)?;
            change(&mut c)?;
            c.version += 1;
            c.updated_at = crate::now_secs();
            match self.registry.update(&c).await {
                Ok(()) => return Ok(c),
                Err(StoreError::Conflict) => continue,
                Err(e) => return Err(ApiError::store(e)),
            }
        }
        Err(ApiError::store(StoreError::Conflict))
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request(format!("bad request body: {e}")))
}

async fn readyz(State(s): State<Arc<Issuer>>) -> Response {
    match s.registry.ready().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::store(e).into_response(),
    }
}

fn jwks_response(s: &Issuer) -> Response {
    (
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(serde_json::json!({"keys": s.keys.jwks()})),
    )
        .into_response()
}

async fn jwks(State(s): State<Arc<Issuer>>) -> Response {
    jwks_response(&s)
}

fn unknown_account() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn account_jwks(State(s): State<Arc<Issuer>>, Path(acct): Path<String>) -> Response {
    if !valid_account_id(&acct) {
        return unknown_account();
    }
    jwks_response(&s)
}

/// The OIDC discovery document of one account's issuer.
pub fn discovery_document(base_url: &str, acct: &str) -> serde_json::Value {
    let iss = issuer_url(base_url, acct);
    serde_json::json!({
        "issuer": iss,
        "jwks_uri": format!("{iss}/.well-known/jwks.json"),
        "response_types_supported": ["id_token"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid"],
        "claims_supported": CLAIMS_SUPPORTED,
    })
}

async fn discovery(State(s): State<Arc<Issuer>>, Path(acct): Path<String>) -> Response {
    if !valid_account_id(&acct) {
        return unknown_account();
    }
    (
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(discovery_document(&s.base_url, &acct)),
    )
        .into_response()
}

async fn account(State(s): State<Arc<Issuer>>, headers: HeaderMap) -> Response {
    match s.caller(&headers).await {
        Ok(c) => Json(serde_json::json!({
            "account": c.acct,
            "issuer": issuer_url(&s.base_url, &c.acct),
            "subject": format!("account:{}", c.acct),
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct TokenRequest {
    connection: String,
    purpose: Purpose,
    audience: String,
    #[serde(default)]
    agent: Option<String>,
}

/// `POST /v1/token` response.
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    /// The JWT.
    pub token: String,
    /// `iss`.
    pub issuer: String,
    /// `sub`.
    pub subject: String,
    /// `aud`.
    pub audience: String,
    /// `jti`.
    pub jti: String,
    /// `exp`.
    pub expires_at: u64,
    /// Seconds until `exp` (900).
    pub expires_in: u64,
}

async fn token(
    State(s): State<Arc<Issuer>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match s.caller(&headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let mut record = AuditRecord::new(&caller.acct, "token_issued");
    record.user = Some(caller.user.user.clone());
    match mint(&s, &caller, &method, &uri, &headers, &body, &mut record).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            record.kind = "token_refused".into();
            record.code = Some(e.code.to_owned());
            record.jti = None;
            tracing::info!(acct = %caller.acct, code = e.code, connection = ?record.connection, device = ?record.device, "token refused");
            s.audit_best_effort(&record).await;
            e.into_response()
        }
    }
}

async fn mint(
    s: &Issuer,
    caller: &Caller,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &Bytes,
    record: &mut AuditRecord,
) -> Result<TokenResponse, ApiError> {
    let req: TokenRequest = parse(body)?;
    let agent = req.agent.filter(|a| !a.is_empty());
    record.connection = Some(req.connection.clone());
    record.purpose = Some(req.purpose.as_str().to_owned());
    record.audience = Some(req.audience.clone());
    record.agent = agent.clone();
    if let Some(a) = &agent {
        if !crate::model::valid_agent(a) {
            return Err(ApiError::bad_request(
                "agent must be 1-128 printable characters",
            ));
        }
    }
    let proof = s
        .device_proof(
            caller,
            headers,
            method,
            uri,
            &req.connection,
            req.purpose.as_str(),
            Some(agent.as_deref()),
        )
        .await?;
    record.device = Some(proof.device.clone());
    let conn = s
        .registry
        .get(&caller.acct, &req.connection)
        .await
        .map_err(ApiError::store)?
        .ok_or_else(ApiError::not_found)?;
    if conn.cloud.auth() != crate::model::Auth::Oidc {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "connection_not_oidc",
            format!(
                "{} connections use a sealed API token, not Cua's sign-in",
                conn.cloud.as_str()
            ),
        ));
    }
    if !conn.allows_audience(&req.audience) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "audience_not_allowed",
            format!(
                "{:?} is not an audience {} accepts for this connection",
                req.audience,
                conn.cloud.as_str()
            ),
        ));
    }
    match conn.status {
        Status::Revoked => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "connection_revoked",
                "this cloud was disconnected",
            ))
        }
        Status::Pending if req.purpose != Purpose::Check => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "connection_pending",
                "this cloud waits for a person to approve it in Cua",
            ))
        }
        _ => {}
    }
    if req.purpose == Purpose::Create {
        let Some(budget) = conn.limits.budget_usd_month else {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "budget_required",
                "set a monthly budget for this cloud before creating machines",
            ));
        };
        let spent = s
            .ledger
            .spent_month(&caller.acct, &conn.id)
            .await
            .map_err(ApiError::store)?;
        if spent >= budget {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "budget_exhausted",
                format!(
                    "this month's budget for this cloud (${budget:.2}) is used up; stopping and deleting machines still works"
                ),
            ));
        }
    }
    let iat = crate::now_secs();
    let claims = Claims {
        iss: issuer_url(&s.base_url, &caller.acct),
        sub: format!("account:{}", caller.acct),
        aud: req.audience.clone(),
        iat,
        nbf: iat,
        exp: iat + TOKEN_TTL_SECS,
        jti: new_jti(),
        cua_conn: conn.id.clone(),
        cua_device: proof.device.clone(),
        cua_agent: agent.clone(),
        cua_purpose: req.purpose.as_str().to_owned(),
    };
    record.jti = Some(claims.jti.clone());
    let token = crate::token::sign(s.keys.active().as_ref(), &claims)
        .await
        .map_err(|e| {
            tracing::error!(%e, "signing failed");
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "signing_unavailable",
                "Cua's sign-in for clouds can't sign right now; try again shortly",
            )
        })?;
    // No token leaves without its audit row.
    s.audit_required(record).await?;
    tracing::info!(acct = %caller.acct, connection = %conn.id, device = %proof.device, purpose = %claims.cua_purpose, agent = ?claims.cua_agent, jti = %claims.jti, "token issued");
    Ok(TokenResponse {
        token,
        issuer: claims.iss,
        subject: claims.sub,
        audience: claims.aud,
        jti: claims.jti,
        expires_at: claims.exp,
        expires_in: TOKEN_TTL_SECS,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    cloud: Cloud,
    #[serde(default)]
    identifiers: BTreeMap<String, String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    limits: Limits,
    #[serde(default)]
    sealed_token: Option<SealedToken>,
}

fn new_connection_id() -> String {
    format!("conn_{}", &new_jti_hex()[..16])
}

fn new_jti_hex() -> String {
    use rand::RngCore as _;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

async fn create(
    State(s): State<Arc<Issuer>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, "", "connection.create")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match create_inner(&s, &dc, &body).await {
        Ok(c) => (StatusCode::CREATED, Json(c)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn create_inner(s: &Issuer, dc: &DeviceCaller, body: &Bytes) -> Result<Connection, ApiError> {
    let req: CreateRequest = parse(body)?;
    validate_identifiers(req.cloud, &req.identifiers).map_err(ApiError::bad_request)?;
    req.limits.validate().map_err(ApiError::bad_request)?;
    if let Some(region) = &req.region {
        if region.is_empty() || region.len() > 64 || region.chars().any(|c| c.is_control()) {
            return Err(ApiError::bad_request(
                "region must be 1-64 printable characters",
            ));
        }
    }
    let auth = req.cloud.auth();
    if let Some(sealed) = &req.sealed_token {
        if auth != crate::model::Auth::Sealed {
            return Err(ApiError::bad_request(format!(
                "{} connections use Cua's sign-in and take no token",
                req.cloud.as_str()
            )));
        }
        if sealed.ciphertext.is_empty() || sealed.ciphertext.len() > MAX_SEALED_BYTES {
            return Err(ApiError::bad_request(
                "sealed_token.ciphertext must be 1-16384 bytes",
            ));
        }
    }
    let now = crate::now_secs();
    let agent = dc.proof.agent.clone();
    let c = Connection {
        id: new_connection_id(),
        account: dc.caller.acct.clone(),
        cloud: req.cloud,
        auth,
        identifiers: req.identifiers,
        region: req.region,
        limits: req.limits,
        pending_limits: None,
        pending_agent: None,
        // An agent's connection waits for a person.
        status: if agent.is_some() {
            Status::Pending
        } else {
            Status::Connected
        },
        created_by: CreatedBy {
            user: dc.caller.user.user.clone(),
            device: dc.proof.device.clone(),
            agent: agent.clone(),
        },
        approved_by: None,
        sealed_token: req.sealed_token,
        created_at: now,
        updated_at: now,
        version: 1,
    };
    s.registry.create(&c).await.map_err(ApiError::store)?;
    let mut r = AuditRecord::new(&c.account, "connection_created");
    r.connection = Some(c.id.clone());
    r.device = Some(dc.proof.device.clone());
    r.agent = agent;
    r.user = Some(dc.caller.user.user.clone());
    s.audit_best_effort(&r).await;
    tracing::info!(acct = %c.account, connection = %c.id, cloud = c.cloud.as_str(), status = c.status.as_str(), "connection created");
    Ok(c)
}

async fn list(State(s): State<Arc<Issuer>>, headers: HeaderMap) -> Response {
    let caller = match s.caller(&headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    match s.registry.list(&caller.acct).await {
        Ok(connections) => {
            let mut out = Vec::with_capacity(connections.len());
            for c in &connections {
                out.push(s.with_spend(c).await);
            }
            Json(serde_json::json!({"connections": out})).into_response()
        }
        Err(e) => ApiError::store(e).into_response(),
    }
}

async fn get_one(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match s.caller(&headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    match s.registry.get(&caller.acct, &id).await {
        Ok(Some(c)) => Json(s.with_spend(&c).await).into_response(),
        Ok(None) => ApiError::not_found().into_response(),
        Err(e) => ApiError::store(e).into_response(),
    }
}

/// `POST /v1/connections/{id}/spend` body: what this device spent there
/// in `month`, its running total.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpendRequest {
    /// `YYYY-MM` (UTC): this month, or the last one (a late report).
    month: String,
    /// US dollars.
    usd: f64,
}

/// Most a device may report for one connection and month (sanity bound).
const MAX_SPEND_USD: f64 = 10_000_000.0;

/// The month before `month` (`YYYY-MM`).
fn previous_month(month: &str) -> Option<String> {
    let (y, m) = month.split_once('-')?;
    let (y, m): (i32, u32) = (y.parse().ok()?, m.parse().ok()?);
    Some(if m == 1 {
        format!("{:04}-12", y - 1)
    } else {
        format!("{y:04}-{:02}", m - 1)
    })
}

async fn spend(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.spend")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let req: SpendRequest = match parse(&body) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    if !req.usd.is_finite() || !(0.0..=MAX_SPEND_USD).contains(&req.usd) {
        return ApiError::bad_request("usd must be between 0 and 10000000").into_response();
    }
    let now = crate::this_month();
    if req.month != now && previous_month(&now).as_deref() != Some(req.month.as_str()) {
        return ApiError::bad_request(format!("month must be this month ({now}) or the last one"))
            .into_response();
    }
    let conn = match s.registry.get(&dc.caller.acct, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return ApiError::not_found().into_response(),
        Err(e) => return ApiError::store(e).into_response(),
    };
    if let Err(e) = s
        .ledger
        .record_spend(
            &conn.account,
            &conn.id,
            &dc.proof.device,
            &req.month,
            req.usd,
        )
        .await
    {
        return ApiError::store(e).into_response();
    }
    tracing::debug!(acct = %conn.account, connection = %conn.id, device = %dc.proof.device, month = %req.month, usd = req.usd, "spend reported");
    match s.ledger.spent_month(&conn.account, &conn.id).await {
        Ok(total) => Json(serde_json::json!({
            "connection": conn.id,
            "month": now,
            "spent_month_usd": total,
            "budget_usd_month": conn.limits.budget_usd_month,
        }))
        .into_response(),
        Err(e) => ApiError::store(e).into_response(),
    }
}

/// `POST /v1/connections/{id}/limits` response.
#[derive(Debug, Serialize, Deserialize)]
pub struct LimitsResponse {
    /// The connection after the change.
    pub connection: Connection,
    /// Some of the change raised a limit for an agent and waits for a
    /// person's approval (in `connection.pending_limits`).
    pub approval_required: bool,
}

async fn limits(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.limits")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let change: Limits = match parse(&body) {
        Ok(l) => l,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = change.validate() {
        return ApiError::bad_request(e).into_response();
    }
    if change.is_empty() {
        return ApiError::bad_request("set budget_usd_month, max_size or ttl_hours")
            .into_response();
    }
    let agent = dc.proof.agent.clone();
    let mut approval_required = false;
    let result = s
        .modify(&dc.caller.acct, &id, |c| {
            if c.status == Status::Revoked {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "connection_revoked",
                    "this cloud was disconnected",
                ));
            }
            if agent.is_some() {
                // An agent may lower limits; raising waits for a person.
                let (now, raise) = change.split_raise(&c.limits);
                c.limits.apply(&now);
                approval_required = !raise.is_empty();
                if approval_required {
                    let mut pending = c.pending_limits.clone().unwrap_or_default();
                    pending.apply(&raise);
                    c.pending_limits = Some(pending);
                    c.pending_agent = agent.clone();
                }
            } else {
                approval_required = false;
                c.limits.apply(&change);
                c.pending_limits = None;
                c.pending_agent = None;
            }
            Ok(())
        })
        .await;
    match result {
        Ok(c) => {
            let mut r = AuditRecord::new(
                &c.account,
                if approval_required {
                    "limits_requested"
                } else {
                    "limits_changed"
                },
            );
            r.connection = Some(c.id.clone());
            r.device = Some(dc.proof.device.clone());
            r.agent = agent;
            r.user = Some(dc.caller.user.user.clone());
            s.audit_best_effort(&r).await;
            Json(LimitsResponse {
                connection: c,
                approval_required,
            })
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn approve(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.approve")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    if dc.proof.agent.is_some() {
        let mut r = AuditRecord::new(&dc.caller.acct, "request_refused");
        r.connection = Some(id.clone());
        r.device = Some(dc.proof.device.clone());
        r.agent = dc.proof.agent.clone();
        r.user = Some(dc.caller.user.user.clone());
        r.purpose = Some("connection.approve".into());
        r.code = Some("agent_cannot_approve".into());
        s.audit_best_effort(&r).await;
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "agent_cannot_approve",
            "only a person can approve a cloud or its limits",
        )
        .into_response();
    }
    let now = crate::now_secs();
    let result = s
        .modify(&dc.caller.acct, &id, |c| {
            match c.status {
                Status::Revoked => {
                    return Err(ApiError::new(
                        StatusCode::FORBIDDEN,
                        "connection_revoked",
                        "this cloud was disconnected",
                    ))
                }
                Status::Connected if c.pending_limits.is_none() => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "nothing_to_approve",
                        "this cloud has nothing waiting for approval",
                    ))
                }
                _ => {}
            }
            c.status = Status::Connected;
            if let Some(pending) = c.pending_limits.take() {
                c.limits.apply(&pending);
            }
            c.pending_agent = None;
            c.approved_by = Some(ApprovedBy {
                user: dc.caller.user.user.clone(),
                device: dc.proof.device.clone(),
                at: now,
            });
            Ok(())
        })
        .await;
    match result {
        Ok(c) => {
            let mut r = AuditRecord::new(&c.account, "connection_approved");
            r.connection = Some(c.id.clone());
            r.device = Some(dc.proof.device.clone());
            r.user = Some(dc.caller.user.user.clone());
            s.audit_best_effort(&r).await;
            Json(c).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `POST /v1/connections/{id}/deny`: a person declines what an agent asked
/// for. An agent's raise is dropped (the limits in force stay); an agent's
/// link that was never approved is revoked. Agents can't deny either.
async fn deny(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.deny")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    if dc.proof.agent.is_some() {
        let mut r = AuditRecord::new(&dc.caller.acct, "request_refused");
        r.connection = Some(id.clone());
        r.device = Some(dc.proof.device.clone());
        r.agent = dc.proof.agent.clone();
        r.user = Some(dc.caller.user.user.clone());
        r.purpose = Some("connection.deny".into());
        r.code = Some("agent_cannot_approve".into());
        s.audit_best_effort(&r).await;
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "agent_cannot_approve",
            "only a person can approve or deny a cloud or its limits",
        )
        .into_response();
    }
    let result = s
        .modify(&dc.caller.acct, &id, |c| {
            match c.status {
                Status::Revoked => {
                    return Err(ApiError::new(
                        StatusCode::FORBIDDEN,
                        "connection_revoked",
                        "this cloud was disconnected",
                    ))
                }
                Status::Pending => c.status = Status::Revoked,
                Status::Connected if c.pending_limits.is_none() => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "nothing_to_approve",
                        "this cloud has nothing waiting for approval",
                    ))
                }
                Status::Connected => {}
            }
            c.pending_limits = None;
            c.pending_agent = None;
            Ok(())
        })
        .await;
    match result {
        Ok(c) => {
            let mut r = AuditRecord::new(&c.account, "connection_denied");
            r.connection = Some(c.id.clone());
            r.device = Some(dc.proof.device.clone());
            r.user = Some(dc.caller.user.user.clone());
            s.audit_best_effort(&r).await;
            Json(c).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `POST /v1/connections/{id}/sealed-token`: a new sealed token for a
/// `sealed` connection, swapped in only if the connection is still as the
/// caller read it (compare-and-swap).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedTokenRequest {
    /// The new ciphertext and the pool key generation it is sealed under.
    sealed_token: SealedToken,
    /// The connection's `version` the caller read.
    #[serde(default)]
    expected_version: Option<u64>,
    /// The generation of the sealed token the caller read (0: none).
    #[serde(default)]
    expected_generation: Option<u64>,
}

async fn sealed_token(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.seal")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match sealed_token_inner(&s, &dc, &id, &body).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn sealed_token_inner(
    s: &Issuer,
    dc: &DeviceCaller,
    id: &str,
    body: &Bytes,
) -> Result<Connection, ApiError> {
    let req: SealedTokenRequest = parse(body)?;
    let sealed = &req.sealed_token;
    if sealed.ciphertext.is_empty() || sealed.ciphertext.len() > MAX_SEALED_BYTES {
        return Err(ApiError::bad_request(
            "sealed_token.ciphertext must be 1-16384 bytes",
        ));
    }
    if sealed.generation == 0 {
        return Err(ApiError::bad_request(
            "sealed_token.generation must be at least 1",
        ));
    }
    if req.expected_version.is_none() && req.expected_generation.is_none() {
        return Err(ApiError::bad_request(
            "name the connection as you read it: expected_version or expected_generation",
        ));
    }
    let mut c = s
        .registry
        .get(&dc.caller.acct, id)
        .await
        .map_err(ApiError::store)?
        .ok_or_else(ApiError::not_found)?;
    if c.auth != crate::model::Auth::Sealed {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "connection_not_sealed",
            format!(
                "{} connections use Cua's sign-in and take no token",
                c.cloud.as_str()
            ),
        ));
    }
    if c.status == Status::Revoked {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "connection_revoked",
            "this cloud was disconnected",
        ));
    }
    let had = c.sealed_token.as_ref().map_or(0, |t| t.generation);
    let stale = req.expected_version.is_some_and(|v| v != c.version)
        || req.expected_generation.is_some_and(|g| g != had);
    if stale {
        return Err(ApiError::store(StoreError::Conflict));
    }
    let agent = dc.proof.agent.clone();
    let resealed = agent.is_none() && sealed.generation > had && had > 0;
    c.sealed_token = Some(req.sealed_token.clone());
    // A token an agent puts in waits for a person, like an agent's link.
    if agent.is_some() {
        c.status = Status::Pending;
    }
    c.version += 1;
    c.updated_at = crate::now_secs();
    // One attempt: a concurrent change means the caller's read is stale.
    s.registry.update(&c).await.map_err(ApiError::store)?;
    let mut r = AuditRecord::new(
        &c.account,
        if resealed {
            "sealed_token_resealed"
        } else {
            "sealed_token_replaced"
        },
    );
    r.connection = Some(c.id.clone());
    r.device = Some(dc.proof.device.clone());
    r.agent = agent;
    r.user = Some(dc.caller.user.user.clone());
    s.audit_best_effort(&r).await;
    tracing::info!(acct = %c.account, connection = %c.id, generation = sealed.generation, resealed, "sealed token stored");
    Ok(c)
}

async fn revoke(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.revoke")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let result = s
        .modify(&dc.caller.acct, &id, |c| {
            c.status = Status::Revoked;
            c.pending_limits = None;
            c.pending_agent = None;
            // A disconnected cloud's sealed token is of no use to anyone:
            // the ciphertext goes with it.
            c.sealed_token = None;
            Ok(())
        })
        .await;
    match result {
        Ok(c) => {
            let mut r = AuditRecord::new(&c.account, "connection_revoked");
            r.connection = Some(c.id.clone());
            r.device = Some(dc.proof.device.clone());
            r.agent = dc.proof.agent.clone();
            r.user = Some(dc.caller.user.user.clone());
            s.audit_best_effort(&r).await;
            tracing::info!(acct = %c.account, connection = %c.id, "connection revoked");
            Json(c).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn delete(
    State(s): State<Arc<Issuer>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let dc = match s
        .device_caller(&headers, &method, &uri, &id, "connection.delete")
        .await
    {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match s.registry.delete(&dc.caller.acct, &id).await {
        Ok(true) => {
            let mut r = AuditRecord::new(&dc.caller.acct, "connection_deleted");
            r.connection = Some(id.clone());
            r.device = Some(dc.proof.device.clone());
            r.agent = dc.proof.agent.clone();
            r.user = Some(dc.caller.user.user.clone());
            s.audit_best_effort(&r).await;
            tracing::info!(acct = %dc.caller.acct, connection = %id, "connection deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => ApiError::not_found().into_response(),
        Err(e) => ApiError::store(e).into_response(),
    }
}

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default)]
    connection: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn audit(
    State(s): State<Arc<Issuer>>,
    Query(q): Query<AuditQuery>,
    headers: HeaderMap,
) -> Response {
    let caller = match s.caller(&headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    match s
        .registry
        .audit(&caller.acct, q.connection.as_deref(), limit)
        .await
    {
        Ok(events) => Json(serde_json::json!({"events": events})).into_response(),
        Err(e) => ApiError::store(e).into_response(),
    }
}

/// The AWS template for one account's issuer: the shared template with
/// `CuaAccount` and `IssuerHost` defaulting to this account and this issuer,
/// so it can be deployed as is (CLI or console upload).
pub fn render_aws_template(base_url: &str, acct: &str) -> String {
    let host = base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let mut out = String::with_capacity(AWS_TEMPLATE.len() + 64);
    let mut section = "";
    let mut param = "";
    for line in AWS_TEMPLATE.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            section = line.trim_end_matches(':');
        } else if section == "Parameters" && line.starts_with("  ") && !line.starts_with("   ") {
            param = line.trim().trim_end_matches(':');
            out.push_str(line);
            out.push('\n');
            let value = match param {
                "CuaAccount" => Some(acct),
                "IssuerHost" => Some(host),
                _ => None,
            };
            if let Some(v) = value {
                out.push_str(&format!("    Default: \"{v}\"\n"));
            }
            continue;
        }
        // The shared template's own default is replaced by the line above.
        if section == "Parameters"
            && matches!(param, "CuaAccount" | "IssuerHost")
            && line.trim_start().starts_with("Default:")
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[derive(Deserialize)]
struct TemplateQuery {
    #[serde(default)]
    acct: Option<String>,
}

async fn aws_template(
    State(s): State<Arc<Issuer>>,
    Query(q): Query<TemplateQuery>,
    headers: HeaderMap,
) -> Response {
    let acct = match q.acct {
        Some(a) if valid_account_id(&a) => a,
        Some(_) => {
            return ApiError::bad_request("acct must be a 12-character account id").into_response()
        }
        None => match s.caller(&headers).await {
            Ok(c) => c.acct,
            Err(_) => {
                return ApiError::bad_request(
                    "pass ?acct=<account id> or sign in to get your account's template",
                )
                .into_response()
            }
        },
    };
    (
        [
            (header::CONTENT_TYPE, "application/yaml; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        render_aws_template(&s.base_url, &acct),
    )
        .into_response()
}
