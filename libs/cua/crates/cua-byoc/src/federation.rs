// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cloud access without a stored key: OIDC federation through Cua's
//! issuer (`cua-oidc`, `https://oidc.cua.ai`).
//!
//! The account links a cloud once: the customer's cloud trusts tokens Cua's
//! issuer signs for this account (the CloudFormation stack in
//! `templates/aws/cua-spaces.yaml` for AWS). When a device needs the cloud,
//! it asks the issuer for a 15-minute token ([`IssuerClient::token`]),
//! proving two things:
//!
//! 1. the account's sign-in: the `cua auth login` access token as a bearer;
//! 2. this device: a DPoP-style proof (RFC 9449) signed with the device's
//!    enrolled P-256 key ([`DeviceSigner`]), over the request's URL, method,
//!    time, a nonce, the connection, the purpose and the agent.
//!
//! The token is then exchanged with the cloud for short-lived credentials
//! ([`aws::WebIdentityProvider`]: STS `AssumeRoleWithWebIdentity`), cached
//! until 2 minutes before they expire. Agents never see the token or the
//! credentials: this process does the exchange and the cloud calls.
//!
//! The issuer can refuse (the connection waits for a person's approval, it
//! was revoked, the month's budget is used up, the device is not enrolled):
//! each refusal is an [`IssuerError`] whose text says what to do.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// The environment variable that points the client at another issuer (a
/// self-hosted one, or a test's).
pub const ISSUER_URL_VAR: &str = "CUA_OIDC_ISSUER_URL";

/// The production issuer.
pub const DEFAULT_ISSUER_URL: &str = "https://oidc.cua.ai";

/// The staging issuer (`CUA_ENV=staging`).
pub const STAGING_ISSUER_URL: &str = "https://oidc.staging.cua.ai";

pub use cua_sandbox_core::byoc::{
    AGENT_RUN_VAR, AGENT_VAR, CODING_AGENT_VARS, caller_agent, caller_agent_with,
};

/// The issuer's base URL: [`ISSUER_URL_VAR`], else the Cua environment's.
pub fn issuer_url() -> String {
    if let Ok(v) = std::env::var(ISSUER_URL_VAR)
        && !v.trim().is_empty()
    {
        return v.trim().trim_end_matches('/').to_string();
    }
    if cua_home::env::CuaEnv::current().is_prod() {
        DEFAULT_ISSUER_URL.into()
    } else {
        STAGING_ISSUER_URL.into()
    }
}

/// The role the AWS trust stack creates, by account:
/// `arn:aws:iam::<account>:role/cua-spaces`.
pub fn aws_role_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:role/cua-spaces")
}

/// The 12-digit account id of an AWS role ARN.
pub fn aws_account_of(role_arn: &str) -> Option<&str> {
    let acct = role_arn.split(':').nth(4)?;
    (acct.len() == 12 && acct.bytes().all(|b| b.is_ascii_digit())).then_some(acct)
}

/// What a token is for (the `cua_purpose` claim). The issuer refuses
/// `create` once the month's budget is used; `manage` (stop, start,
/// delete, sweep) always works so you can clean up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// New machines.
    Create,
    /// Describe, stop, start, delete, sweep.
    Manage,
    /// The connect check (allowed while the connection is pending).
    Check,
    /// Billing reads.
    Cost,
}

impl Purpose {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Create => "create",
            Purpose::Manage => "manage",
            Purpose::Check => "check",
            Purpose::Cost => "cost",
        }
    }
}

/// One `POST /v1/token`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRequest {
    /// The account connection (`conn_...`).
    pub connection: String,
    /// What it is for.
    pub purpose: Purpose,
    /// The cloud's audience (`sts.amazonaws.com`).
    pub audience: String,
    /// The agent the device asks for, when an agent is the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// A token the issuer signed.
#[derive(Clone, PartialEq, Eq)]
pub struct MintedToken {
    /// The signed JWT (secret until it expires).
    pub token: String,
    /// Unix seconds it expires at.
    pub expires_at: u64,
}

impl std::fmt::Debug for MintedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedToken")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl MintedToken {
    /// The token's claims, unverified (the cloud verifies them; Cua reads
    /// them only to name the session).
    pub fn claims(&self) -> serde_json::Value {
        self.token
            .split('.')
            .nth(1)
            .and_then(|p| b64().decode(p.trim_end_matches('=')).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

/// Why the issuer did not sign a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuerError {
    /// The issuer could not be reached.
    Unreachable(String),
    /// The account sign-in is missing or expired.
    SignIn(String),
    /// This device is not enrolled (or was revoked).
    Device(String),
    /// The connection waits for a person to approve it.
    Pending,
    /// The connection was revoked or disconnected.
    Revoked,
    /// This month's budget for the connection is used up.
    BudgetReached,
    /// The connection has no monthly budget yet (needed before the first
    /// machine).
    BudgetRequired,
    /// No such connection on the account.
    NotFound(String),
    /// The cloud is linked already, as this connection id.
    Exists(String),
    /// The connection changed since it was read (another device swapped
    /// its sealed token): read it again.
    Conflict,
    /// Anything else the issuer said.
    Other(String),
}

impl std::fmt::Display for IssuerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssuerError::Unreachable(m) => {
                write!(f, "Can't reach Cua's sign-in for the cloud ({m})")
            }
            IssuerError::SignIn(m) if m.contains("auth login") => {
                write!(f, "Cua sign-in needed: {m}")
            }
            IssuerError::SignIn(m) => write!(f, "Cua sign-in needed: {m} (run `cua auth login`)"),
            IssuerError::Device(m) => write!(
                f,
                "this device is not enrolled for the account: {m} (run `cua devices status`)"
            ),
            IssuerError::Pending => write!(
                f,
                "the connection waits for a person to approve it in Cua Spaces"
            ),
            IssuerError::Revoked => write!(f, "the connection was disconnected"),
            IssuerError::BudgetReached => write!(
                f,
                "this month's budget for the connection is used up; new machines are refused \
                 (stop and delete still work; raise it with `cua cloud connect <cloud> --budget`)"
            ),
            IssuerError::BudgetRequired => write!(
                f,
                "the connection needs a monthly budget before the first machine \
                 (`cua cloud connect <cloud> --budget <usd>`)"
            ),
            IssuerError::NotFound(m) => write!(f, "no such connection on the account: {m}"),
            IssuerError::Exists(id) => write!(f, "this cloud is linked already ({id})"),
            IssuerError::Conflict => write!(
                f,
                "the connection changed at the same time on another device; try again"
            ),
            IssuerError::Other(m) => write!(f, "Cua's issuer refused: {m}"),
        }
    }
}

impl std::error::Error for IssuerError {}

impl From<IssuerError> for cua_sandbox_core::Error {
    fn from(e: IssuerError) -> Self {
        cua_sandbox_core::Error::Cloud(e.to_string())
    }
}

/// Cua's issuer, as this device sees it. The HTTP client is [`HttpIssuer`];
/// tests use a fake.
#[async_trait]
pub trait IssuerClient: Send + Sync + 'static {
    /// A token for `req`.
    async fn token(&self, req: &TokenRequest) -> Result<MintedToken, IssuerError>;

    /// The signed-in account's issuer id and URL (`GET /v1/account`).
    async fn account(&self) -> Result<IssuerAccount, IssuerError> {
        Err(unsupported())
    }

    /// The account's connections (`GET /v1/connections`).
    async fn connections(&self) -> Result<Vec<ServerConnection>, IssuerError> {
        Err(unsupported())
    }

    /// Links a cloud (`POST /v1/connections`). An agent's link waits for
    /// a person's approval. [`IssuerError::Exists`] when the cloud is
    /// linked already.
    async fn create_connection(
        &self,
        _new: &NewConnection,
        _agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        Err(unsupported())
    }

    /// Changes a connection's limits (`POST /v1/connections/{id}/limits`).
    /// An agent may lower them; what it raises waits for approval (the
    /// `bool`).
    async fn set_limits(
        &self,
        _id: &str,
        _limits: &IssuerLimits,
        _agent: Option<&str>,
    ) -> Result<(ServerConnection, bool), IssuerError> {
        Err(unsupported())
    }

    /// Stops a connection minting tokens, for every device of the account
    /// (`POST /v1/connections/{id}/revoke`). The trust in the cloud stays
    /// until its owner removes it; a sealed token's ciphertext is dropped.
    async fn revoke(&self, _id: &str, _agent: Option<&str>) -> Result<(), IssuerError> {
        Err(unsupported())
    }

    /// A person approves what an agent asked for (`POST
    /// /v1/connections/{id}/approve`), or with `deny` declines it (`/deny`:
    /// a raise is dropped, a link never approved is revoked). The issuer
    /// refuses agents.
    async fn approve(&self, _id: &str, _deny: bool) -> Result<ServerConnection, IssuerError> {
        Err(unsupported())
    }

    /// The account's audit (`GET /v1/audit`): tokens issued or refused,
    /// links made, approved, changed and revoked, newest first. `connection`
    /// narrows it to one link.
    async fn audit(
        &self,
        _connection: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<IssuerAuditEvent>, IssuerError> {
        Err(unsupported())
    }

    /// Reports what this device spent on connection `id` in `month`
    /// (`YYYY-MM`, its running total) and answers the account's month
    /// total (`POST /v1/connections/{id}/spend`).
    async fn report_spend(&self, _id: &str, _month: &str, _usd: f64) -> Result<f64, IssuerError> {
        Err(unsupported())
    }

    /// Puts a new sealed token on a `sealed` connection
    /// (`POST /v1/connections/{id}/sealed-token`), only if the connection
    /// is still as read: [`IssuerError::Conflict`] otherwise. A token an
    /// agent puts in waits for a person's approval.
    async fn set_sealed_token(
        &self,
        _id: &str,
        _token: &SealedBlob,
        _expected: Expected,
        _agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        Err(unsupported())
    }
}

/// One row of the issuer's audit. Never a token: the token id, purpose and
/// who asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerAuditEvent {
    /// Unix seconds.
    #[serde(default)]
    pub at: u64,
    /// `token_issued`, `token_refused`, `connection_created`,
    /// `connection_approved`, `limits_changed`, `limits_requested`,
    /// `connection_revoked`, `connection_deleted`, `request_refused`, ...
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub connection: Option<String>,
    /// `dev_…`.
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    /// A token's purpose (`create`, `manage`, `check`, `cost`).
    #[serde(default)]
    pub purpose: Option<String>,
    /// A refusal's error code.
    #[serde(default)]
    pub code: Option<String>,
}

/// A sealed API token as the issuer stores it: ciphertext only the
/// account's devices open (standard base64 on the wire), and the pool key
/// generation it is sealed under.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedBlob {
    /// Opaque ciphertext.
    #[serde(with = "std_b64")]
    pub ciphertext: Vec<u8>,
    /// The pool key generation.
    pub generation: u64,
}

impl std::fmt::Debug for SealedBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedBlob")
            .field("bytes", &self.ciphertext.len())
            .field("generation", &self.generation)
            .finish()
    }
}

mod std_b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        base64::engine::general_purpose::STANDARD
            .decode(String::deserialize(d)?)
            .map_err(serde::de::Error::custom)
    }
}

/// What a swap expects the connection to be (compare-and-swap).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Expected {
    /// The connection's version as read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
    /// The generation of its sealed token as read (0: none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_generation: Option<u64>,
}

fn unsupported() -> IssuerError {
    IssuerError::Other("this issuer does not support that call".into())
}

/// `GET /v1/account`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerAccount {
    /// The 12-character issuer account id (`acct`).
    pub account: String,
    /// The account's issuer URL (`https://oidc.cua.ai/a/{acct}`).
    #[serde(default)]
    pub issuer: String,
    /// The `sub` its tokens carry (`account:{acct}`).
    #[serde(default)]
    pub subject: String,
}

/// The largest machine, as the issuer records it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerMaxSize {
    /// vCPUs.
    pub vcpu: u32,
    /// GiB.
    pub memory_gib: u32,
}

/// A connection's limits on the issuer (`None`: not set).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IssuerLimits {
    /// US dollars per month.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd_month: Option<f64>,
    /// The largest machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<IssuerMaxSize>,
    /// Hours a machine may live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_hours: Option<u32>,
}

impl IssuerLimits {
    /// Whether nothing is set.
    pub fn is_empty(&self) -> bool {
        self.budget_usd_month.is_none() && self.max_size.is_none() && self.ttl_hours.is_none()
    }
}

/// A linked cloud as the issuer records it (identifiers, never a secret).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerConnection {
    /// `conn_…`.
    pub id: String,
    /// The issuer account id.
    #[serde(default)]
    pub account: String,
    /// `aws`, `gcp`, `azure`, `hetzner`, `modal`.
    #[serde(default)]
    pub cloud: String,
    /// `oidc` or `sealed`.
    #[serde(default)]
    pub auth: String,
    /// The cloud's identifiers (AWS: `account_id`, `role_arn`).
    #[serde(default)]
    pub identifiers: std::collections::BTreeMap<String, String>,
    /// Default region.
    #[serde(default)]
    pub region: Option<String>,
    /// Limits in force.
    #[serde(default)]
    pub limits: IssuerLimits,
    /// Limits an agent raised, waiting for a person.
    #[serde(default)]
    pub pending_limits: Option<IssuerLimits>,
    /// `pending`, `connected` or `revoked`.
    #[serde(default)]
    pub status: String,
    /// Who linked it (the agent, when an agent did).
    #[serde(default)]
    pub created_by: Option<ServerCreatedBy>,
    /// The agent whose raise waits in `pending_limits`.
    #[serde(default)]
    pub pending_agent: Option<String>,
    /// What the account's devices spent there this month (UTC), US
    /// dollars, as they reported it (absent from older issuers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_month_usd: Option<f64>,
    /// `sealed` connections: the token's ciphertext (none yet, or dropped
    /// when disconnected).
    #[serde(default)]
    pub sealed_token: Option<SealedBlob>,
    /// Bumped by every change (what a swap names).
    #[serde(default)]
    pub version: u64,
}

/// Who linked a cloud on the issuer (identifiers only; the user's id is
/// not kept).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerCreatedBy {
    /// The agent or routine that asked, when one did.
    #[serde(default)]
    pub agent: Option<String>,
}

impl ServerConnection {
    /// The agent whose request waits for a person (the link's creator
    /// while it is pending, else the one that raised the limits).
    pub fn approval_agent(&self) -> Option<&str> {
        match self.approval() {
            "connection" => self.created_by.as_ref()?.agent.as_deref(),
            "limits" => self.pending_agent.as_deref(),
            _ => None,
        }
    }

    /// What waits for a person: `connection`, `limits` or empty.
    pub fn approval(&self) -> &'static str {
        if self.status == "pending" {
            "connection"
        } else if self.pending_limits.as_ref().is_some_and(|l| !l.is_empty()) {
            "limits"
        } else {
            ""
        }
    }
}

/// `POST /v1/connections`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewConnection {
    /// `aws`, ...
    pub cloud: String,
    /// The cloud's identifiers.
    #[serde(default)]
    pub identifiers: std::collections::BTreeMap<String, String>,
    /// Default region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Limits.
    #[serde(default)]
    pub limits: IssuerLimits,
}

/// The device key that proves which device asks.
pub trait DeviceSigner: Send + Sync + 'static {
    /// The public key as an uncompressed P-256 point (65 bytes).
    fn public_point(&self) -> Result<Vec<u8>, IssuerError>;
    /// An ES256 signature of `message` (r || s, base64url).
    fn sign(&self, message: &str) -> Result<String, IssuerError>;
}

impl DeviceSigner for cua_host::DeviceKey {
    fn public_point(&self) -> Result<Vec<u8>, IssuerError> {
        b64()
            .decode(self.public_key())
            .map_err(|e| IssuerError::Device(e.to_string()))
    }

    fn sign(&self, message: &str) -> Result<String, IssuerError> {
        cua_host::DeviceKey::sign(self, message).map_err(|e| IssuerError::Device(e.to_string()))
    }
}

/// The device key of a [`cua_host::DeviceAuth`], read per proof (a key
/// created after start is used).
pub struct DeviceAuthSigner(pub Arc<cua_host::DeviceAuth>);

impl DeviceAuthSigner {
    fn key(&self) -> Result<cua_host::DeviceKey, IssuerError> {
        self.0
            .key()
            .map_err(|e| IssuerError::Device(e.to_string()))?
            .ok_or_else(|| {
                IssuerError::Device("this device has no key yet (run `cua auth login`)".into())
            })
    }
}

impl DeviceSigner for DeviceAuthSigner {
    fn public_point(&self) -> Result<Vec<u8>, IssuerError> {
        self.key()?.public_point()
    }

    fn sign(&self, message: &str) -> Result<String, IssuerError> {
        DeviceSigner::sign(&self.key()?, message)
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What a device proof is for: the connection (`""` when creating one),
/// the purpose (a token purpose, or a connection action such as
/// `connection.create`) and the agent the device acts for.
#[derive(Clone, Copy, Debug)]
pub struct ProofFor<'a> {
    /// The connection id, `""` when creating one.
    pub connection: &'a str,
    /// `create`, `manage`, `check`, `cost`, or `connection.<action>`.
    pub purpose: &'a str,
    /// The agent, `None` when a person acts.
    pub agent: Option<&'a str>,
}

/// The device proof of one issuer call: a compact JWS (ES256) whose header
/// carries the device's public key (`jwk`) and whose claims bind it to this
/// call (`htu`, `htm`, `ath`: the SHA-256 of the access token sent with
/// it), this moment (`iat`, `jti`) and this request (`connection`,
/// `purpose`, `agent`).
pub fn device_proof(
    signer: &dyn DeviceSigner,
    htu: &str,
    htm: &str,
    access_token: &str,
    what: ProofFor<'_>,
) -> Result<String, IssuerError> {
    use sha2::Digest as _;
    let point = signer.public_point()?;
    if point.len() != 65 || point[0] != 4 {
        return Err(IssuerError::Device(
            "the device key is not an uncompressed P-256 point".into(),
        ));
    }
    let header = serde_json::json!({
        "typ": "dpop+jwt",
        "alg": "ES256",
        "jwk": {
            "kty": "EC",
            "crv": "P-256",
            "x": b64().encode(&point[1..33]),
            "y": b64().encode(&point[33..65]),
        },
    });
    let mut claims = serde_json::json!({
        "htu": htu,
        "htm": htm,
        "iat": now_secs(),
        "jti": format!("{:032x}", rand::random::<u128>()),
        "ath": b64().encode(sha2::Sha256::digest(access_token.as_bytes())),
        "connection": what.connection,
        "purpose": what.purpose,
    });
    if let Some(a) = what.agent {
        claims["agent"] = serde_json::Value::String(a.to_string());
    }
    let signing_input = format!(
        "{}.{}",
        b64().encode(header.to_string()),
        b64().encode(claims.to_string())
    );
    let sig = signer.sign(&signing_input)?;
    Ok(format!("{signing_input}.{sig}"))
}

/// The issuer over HTTP: `POST {base}/v1/token` with the account's bearer
/// and a `DPoP` device proof.
pub struct HttpIssuer {
    base: String,
    http: reqwest::Client,
    tokens: Arc<dyn cua_host::relay::AccountTokens>,
    signer: Arc<dyn DeviceSigner>,
}

impl HttpIssuer {
    /// The issuer at `base`, with these account tokens and device key.
    pub fn new(
        base: impl Into<String>,
        tokens: Arc<dyn cua_host::relay::AccountTokens>,
        signer: Arc<dyn DeviceSigner>,
    ) -> Self {
        HttpIssuer {
            base: base.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            tokens,
            signer,
        }
    }

    /// [`issuer_url`], the `cua auth login` session and this device's key
    /// (as the relay knows it).
    pub fn from_env() -> Option<Self> {
        let relay = crate::relay::RelayAccess::from_env();
        let device = relay.device.clone()?;
        Some(HttpIssuer::new(
            issuer_url(),
            relay.tokens.clone(),
            Arc::new(DeviceAuthSigner(device)),
        ))
    }

    /// The base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The account's bearer and a device proof for a call to `url`.
    async fn authorize(
        &self,
        method: &str,
        url: &str,
        what: ProofFor<'_>,
    ) -> Result<(String, String), IssuerError> {
        let bearer = self
            .tokens
            .access_token()
            .await
            .map_err(|e| IssuerError::SignIn(e.to_string()))?;
        let proof = device_proof(self.signer.as_ref(), url, method, &bearer, what)?;
        Ok((bearer, proof))
    }
}

/// The issuer's error body.
#[derive(Debug, Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default, alias = "error_description")]
    message: String,
}

/// An issuer answer that is not a success, as an [`IssuerError`].
pub fn issuer_error(status: u16, body: &str) -> IssuerError {
    let b: ErrorBody = serde_json::from_str(body).unwrap_or_default();
    let detail = if b.message.is_empty() {
        if b.error.is_empty() {
            format!("HTTP {status}")
        } else {
            b.error.clone()
        }
    } else {
        b.message.clone()
    };
    match b.error.as_str() {
        "pending" | "connection_pending" => IssuerError::Pending,
        "revoked" | "connection_revoked" => IssuerError::Revoked,
        "budget" | "budget_reached" | "budget_exhausted" => IssuerError::BudgetReached,
        "budget_required" => IssuerError::BudgetRequired,
        "device"
        | "device_not_enrolled"
        | "device_revoked"
        | "invalid_proof"
        | "proof_replayed"
        | "invalid_dpop_proof" => IssuerError::Device(detail),
        "not_found" | "connection_not_found" => IssuerError::NotFound(detail),
        "connection_exists" => IssuerError::Exists(
            detail
                .split_whitespace()
                .find(|w| w.starts_with("conn_"))
                .unwrap_or_default()
                .trim_end_matches(['.', ','])
                .to_string(),
        ),
        "unauthenticated" | "invalid_token" => IssuerError::SignIn(detail),
        "conflict" => IssuerError::Conflict,
        _ => match status {
            401 => IssuerError::SignIn(detail),
            404 => IssuerError::NotFound(detail),
            _ => IssuerError::Other(detail),
        },
    }
}

/// The issuer's token answer.
#[derive(Deserialize)]
struct TokenBody {
    #[serde(alias = "id_token", alias = "access_token")]
    token: String,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl HttpIssuer {
    /// One JSON call: the bearer always, a device proof for `what`.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        what: Option<ProofFor<'_>>,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, IssuerError> {
        let url = format!("{}{path}", self.base);
        let mut rb = self.http.request(method.clone(), &url);
        match what {
            Some(w) => {
                let (bearer, proof) = self.authorize(method.as_str(), &url, w).await?;
                rb = rb.bearer_auth(bearer).header("DPoP", proof);
            }
            None => {
                let bearer = self
                    .tokens
                    .access_token()
                    .await
                    .map_err(|e| IssuerError::SignIn(e.to_string()))?;
                rb = rb.bearer_auth(bearer);
            }
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| IssuerError::Unreachable(format!("{}: {e}", self.base)))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| IssuerError::Unreachable(format!("{}: {e}", self.base)))?;
        if !(200..300).contains(&status) {
            return Err(issuer_error(status, &text));
        }
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| IssuerError::Other(format!("unreadable answer from {path}: {e}")))
    }
}

fn decode<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, IssuerError> {
    serde_json::from_value(v).map_err(|e| IssuerError::Other(format!("unreadable answer: {e}")))
}

#[async_trait]
impl IssuerClient for HttpIssuer {
    async fn account(&self) -> Result<IssuerAccount, IssuerError> {
        decode(
            self.call(reqwest::Method::GET, "/v1/account", None, None)
                .await?,
        )
    }

    async fn connections(&self) -> Result<Vec<ServerConnection>, IssuerError> {
        let v = self
            .call(reqwest::Method::GET, "/v1/connections", None, None)
            .await?;
        decode(v.get("connections").cloned().unwrap_or(v))
    }

    async fn create_connection(
        &self,
        new: &NewConnection,
        agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        let what = ProofFor {
            connection: "",
            purpose: "connection.create",
            agent,
        };
        let body = serde_json::to_value(new).map_err(|e| IssuerError::Other(e.to_string()))?;
        decode(
            self.call(
                reqwest::Method::POST,
                "/v1/connections",
                Some(what),
                Some(body),
            )
            .await?,
        )
    }

    async fn revoke(&self, id: &str, agent: Option<&str>) -> Result<(), IssuerError> {
        let what = ProofFor {
            connection: id,
            purpose: "connection.revoke",
            agent,
        };
        self.call(
            reqwest::Method::POST,
            &format!("/v1/connections/{id}/revoke"),
            Some(what),
            None,
        )
        .await
        .map(|_| ())
    }

    async fn audit(
        &self,
        connection: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IssuerAuditEvent>, IssuerError> {
        let mut path = format!("/v1/audit?limit={}", limit.clamp(1, 1000));
        if let Some(c) = connection.filter(|c| {
            !c.is_empty()
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }) {
            path.push_str(&format!("&connection={c}"));
        }
        let v = self.call(reqwest::Method::GET, &path, None, None).await?;
        decode(v.get("events").cloned().unwrap_or(v))
    }

    async fn approve(&self, id: &str, deny: bool) -> Result<ServerConnection, IssuerError> {
        let (path, purpose) = if deny {
            ("deny", "connection.deny")
        } else {
            ("approve", "connection.approve")
        };
        let what = ProofFor {
            connection: id,
            purpose,
            agent: None,
        };
        let v = self
            .call(
                reqwest::Method::POST,
                &format!("/v1/connections/{id}/{path}"),
                Some(what),
                None,
            )
            .await?;
        decode(v)
    }

    async fn report_spend(&self, id: &str, month: &str, usd: f64) -> Result<f64, IssuerError> {
        let what = ProofFor {
            connection: id,
            purpose: "connection.spend",
            agent: None,
        };
        let v = self
            .call(
                reqwest::Method::POST,
                &format!("/v1/connections/{id}/spend"),
                Some(what),
                Some(serde_json::json!({"month": month, "usd": usd})),
            )
            .await?;
        v.get("spent_month_usd")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| IssuerError::Other("the spend answer has no spent_month_usd".into()))
    }

    async fn set_sealed_token(
        &self,
        id: &str,
        token: &SealedBlob,
        expected: Expected,
        agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        let what = ProofFor {
            connection: id,
            purpose: "connection.seal",
            agent,
        };
        let mut body =
            serde_json::to_value(expected).map_err(|e| IssuerError::Other(e.to_string()))?;
        body["sealed_token"] =
            serde_json::to_value(token).map_err(|e| IssuerError::Other(e.to_string()))?;
        decode(
            self.call(
                reqwest::Method::POST,
                &format!("/v1/connections/{id}/sealed-token"),
                Some(what),
                Some(body),
            )
            .await?,
        )
    }

    async fn set_limits(
        &self,
        id: &str,
        limits: &IssuerLimits,
        agent: Option<&str>,
    ) -> Result<(ServerConnection, bool), IssuerError> {
        let what = ProofFor {
            connection: id,
            purpose: "connection.limits",
            agent,
        };
        let body = serde_json::to_value(limits).map_err(|e| IssuerError::Other(e.to_string()))?;
        let v = self
            .call(
                reqwest::Method::POST,
                &format!("/v1/connections/{id}/limits"),
                Some(what),
                Some(body),
            )
            .await?;
        let approval = v
            .get("approval_required")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Ok((decode(v.get("connection").cloned().unwrap_or(v))?, approval))
    }

    async fn token(&self, req: &TokenRequest) -> Result<MintedToken, IssuerError> {
        let url = format!("{}/v1/token", self.base);
        let what = ProofFor {
            connection: &req.connection,
            purpose: req.purpose.as_str(),
            agent: req.agent.as_deref(),
        };
        let (bearer, proof) = self.authorize("POST", &url, what).await?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(bearer)
            .header("DPoP", proof)
            .json(req)
            .send()
            .await
            .map_err(|e| IssuerError::Unreachable(format!("{}: {e}", self.base)))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| IssuerError::Unreachable(format!("{}: {e}", self.base)))?;
        if !(200..300).contains(&status) {
            return Err(issuer_error(status, &body));
        }
        let t: TokenBody = serde_json::from_str(&body)
            .map_err(|e| IssuerError::Other(format!("unreadable token answer: {e}")))?;
        let mut minted = MintedToken {
            token: t.token,
            expires_at: 0,
        };
        minted.expires_at = t
            .expires_at
            .or_else(|| t.expires_in.map(|s| now_secs() + s))
            .or_else(|| minted.claims()["exp"].as_u64())
            .unwrap_or_else(|| now_secs() + 15 * 60);
        Ok(minted)
    }
}

/// Google Cloud: the STS exchange.
pub mod gcp;

/// Azure: the Entra client-assertion exchange.
pub mod azure;

/// Renew a cloud token this long before it expires.
pub const REFRESH_MARGIN_SECS: u64 = 120;

/// A cloud access token and when it expires, reused until
/// [`REFRESH_MARGIN_SECS`] before that.
#[derive(Default)]
pub struct CachedToken {
    slot: tokio::sync::Mutex<Option<(String, u64)>>,
}

impl CachedToken {
    /// The cached token, or `fetch`'s (token, unix expiry), cached.
    pub async fn get_or<F, Fut>(&self, fetch: F) -> Result<String, ExchangeError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(String, u64), ExchangeError>>,
    {
        let mut slot = self.slot.lock().await;
        if let Some((t, exp)) = slot.as_ref()
            && now_secs() + REFRESH_MARGIN_SECS < *exp
        {
            return Ok(t.clone());
        }
        let (t, exp) = fetch().await?;
        *slot = Some((t.clone(), exp));
        Ok(t)
    }

    /// Drops the cached token.
    pub async fn forget(&self) {
        *self.slot.lock().await = None;
    }
}

/// A cloud refused to exchange Cua's token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudRefused {
    /// `Google Cloud`, `Azure`.
    pub cloud: &'static str,
    /// What was asked for (the provider, the identity).
    pub who: String,
    /// The cloud's words.
    pub detail: String,
    /// Whether it means the cloud does not trust Cua's issuer (yet).
    pub trust: bool,
    /// What to do about a missing trust.
    pub hint: &'static str,
}

impl std::fmt::Display for CloudRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.trust {
            write!(
                f,
                "{} did not accept Cua's token for {} ({}): {}",
                self.cloud, self.who, self.detail, self.hint
            )
        } else {
            write!(f, "{}: {} ({})", self.cloud, self.detail, self.who)
        }
    }
}

/// Why a cloud token could not be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeError {
    /// Cua's issuer did not sign one.
    Issuer(IssuerError),
    /// The cloud refused the exchange.
    Refused(CloudRefused),
    /// Anything else (the cloud could not be reached).
    Other(String),
}

impl std::fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExchangeError::Issuer(e) => e.fmt(f),
            ExchangeError::Refused(e) => e.fmt(f),
            ExchangeError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ExchangeError {}

/// The AWS role session name of a token: `cua-{device}-{agent|user}-{purpose}`,
/// at most 64 characters of `[A-Za-z0-9+=,.@_-]` (what STS accepts). A long
/// agent name is shortened so the purpose stays visible in CloudTrail.
pub fn session_name(device: &str, agent: Option<&str>, purpose: &str) -> String {
    fn clean(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "+=,.@_-".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }
    let device = clean(if device.is_empty() { "device" } else { device });
    let who = clean(agent.filter(|a| !a.trim().is_empty()).unwrap_or("user"));
    let purpose = clean(purpose);
    let fixed = "cua-".len() + device.len() + 1 + 1 + purpose.len();
    let room = 64usize.saturating_sub(fixed).max(1);
    let who: String = who.chars().take(room).collect();
    let mut s = format!("cua-{device}-{who}-{purpose}");
    s.truncate(64);
    if s.len() < 2 {
        s = "cua".into();
    }
    s
}

#[cfg(feature = "aws")]
pub mod aws {
    //! AWS: STS `AssumeRoleWithWebIdentity` with an issuer token, as an
    //! aws-config credentials provider.

    use super::*;
    use aws_credential_types::Credentials;
    use aws_credential_types::provider::{self, ProvideCredentials, error::CredentialsError};

    /// The audience AWS STS expects.
    pub const AUDIENCE: &str = "sts.amazonaws.com";

    /// The role the CloudFormation stack creates (its name is fixed, so the
    /// ARN follows from the account id).
    pub const ROLE_NAME: &str = "cua-spaces";

    pub use super::REFRESH_MARGIN_SECS;

    /// `arn:aws:iam::<account>:role/cua-spaces`.
    pub fn role_arn(account: &str) -> String {
        super::aws_role_arn(account)
    }

    /// The account id of a role ARN.
    pub fn account_of(role_arn: &str) -> Option<&str> {
        super::aws_account_of(role_arn)
    }

    /// The CloudFormation stack that sets up the trust (an OIDC provider
    /// for this account's issuer and the `cua-spaces` role with Cua's
    /// least-privilege policy). The issuer serves the same file.
    pub const TEMPLATE: &str = include_str!("../templates/aws/cua-spaces.yaml");

    /// The stack's name in the quick-create link.
    pub const STACK_NAME: &str = "cua-spaces";

    /// The issuer host of `issuer_url` (`oidc.cua.ai`), the template's
    /// `IssuerHost`.
    pub fn issuer_host(issuer_url: &str) -> String {
        let s = issuer_url
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        s.split('/').next().unwrap_or(s).to_string()
    }

    /// The AWS console link that creates the trust stack in `region` from
    /// `template_url` (an S3 copy of [`TEMPLATE`]; the console takes
    /// templates from S3 only), for Cua account `cua_account` on
    /// `issuer_host`. A person opens it, ticks "I acknowledge that AWS
    /// CloudFormation might create IAM resources with custom names", and
    /// presses Create stack.
    pub fn quick_create_url(
        region: &str,
        template_url: &str,
        cua_account: &str,
        issuer_host: &str,
    ) -> String {
        format!(
            "https://console.aws.amazon.com/cloudformation/home?region={}#/stacks/quickcreate?\
             templateURL={}&stackName={STACK_NAME}&param_CuaAccount={}&param_IssuerHost={}",
            pct(region),
            pct(template_url),
            pct(cua_account),
            pct(issuer_host)
        )
    }

    /// Percent-encodes everything but RFC 3986's unreserved characters.
    fn pct(s: &str) -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }

    /// Credentials for one connection, purpose and agent: an issuer token
    /// exchanged with STS, cached until [`REFRESH_MARGIN_SECS`] before they
    /// expire.
    pub struct WebIdentityProvider {
        issuer: Arc<dyn IssuerClient>,
        sts: aws_sdk_sts::Client,
        request: TokenRequest,
        role_arn: String,
        cache: tokio::sync::Mutex<Option<(Credentials, u64)>>,
    }

    impl std::fmt::Debug for WebIdentityProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("WebIdentityProvider")
                .field("connection", &self.request.connection)
                .field("purpose", &self.request.purpose)
                .field("role_arn", &self.role_arn)
                .finish_non_exhaustive()
        }
    }

    /// An STS client that signs nothing (`AssumeRoleWithWebIdentity` is
    /// authorized by the token itself), in `region`, at `endpoint` when
    /// given (tests).
    pub fn anonymous_sts(region: &str, endpoint: Option<&str>) -> aws_sdk_sts::Client {
        let mut conf = aws_sdk_sts::Config::builder()
            .behavior_version_latest()
            .region(aws_sdk_sts::config::Region::new(region.to_string()));
        conf.set_endpoint_url(endpoint.map(str::to_string));
        aws_sdk_sts::Client::from_conf(conf.build())
    }

    impl WebIdentityProvider {
        /// Credentials for `role_arn` through connection `conn_id` for
        /// `purpose` (and `agent`), exchanged with `sts`.
        pub fn new(
            issuer: Arc<dyn IssuerClient>,
            sts: aws_sdk_sts::Client,
            conn_id: &str,
            role_arn: &str,
            purpose: Purpose,
            agent: Option<String>,
        ) -> Self {
            WebIdentityProvider {
                issuer,
                sts,
                request: TokenRequest {
                    connection: conn_id.into(),
                    purpose,
                    audience: AUDIENCE.into(),
                    agent,
                },
                role_arn: role_arn.into(),
                cache: tokio::sync::Mutex::new(None),
            }
        }

        /// Fresh (or cached) credentials.
        pub async fn credentials(&self) -> Result<Credentials, CredentialsError> {
            let mut cache = self.cache.lock().await;
            if let Some((c, exp)) = cache.as_ref()
                && now_secs() + REFRESH_MARGIN_SECS < *exp
            {
                return Ok(c.clone());
            }
            let minted = self
                .issuer
                .token(&self.request)
                .await
                .map_err(CredentialsError::provider_error)?;
            let claims = minted.claims();
            let name = session_name(
                claims["cua_device"].as_str().unwrap_or("device"),
                claims["cua_agent"]
                    .as_str()
                    .or(self.request.agent.as_deref()),
                claims["cua_purpose"]
                    .as_str()
                    .unwrap_or(self.request.purpose.as_str()),
            );
            let out = self
                .sts
                .assume_role_with_web_identity()
                .role_arn(&self.role_arn)
                .role_session_name(name)
                .web_identity_token(&minted.token)
                .duration_seconds(900)
                .send()
                .await
                .map_err(|e| {
                    use aws_sdk_sts::error::ProvideErrorMetadata as _;
                    let code = e.code().unwrap_or_default().to_string();
                    let msg = e.message().unwrap_or_default().to_string();
                    let detail = if code.is_empty() {
                        aws_sdk_sts::error::DisplayErrorContext(&e).to_string()
                    } else {
                        format!("{code}: {msg}")
                    };
                    CredentialsError::provider_error(StsRefused {
                        role_arn: self.role_arn.clone(),
                        detail,
                        trust: matches!(
                            code.as_str(),
                            "AccessDenied" | "InvalidIdentityToken" | "IDPRejectedClaim"
                        ),
                    })
                })?;
            let c = out.credentials().ok_or_else(|| {
                CredentialsError::provider_error(StsRefused {
                    role_arn: self.role_arn.clone(),
                    detail: "STS answered without credentials".into(),
                    trust: false,
                })
            })?;
            let exp = u64::try_from(c.expiration().secs()).unwrap_or(0);
            // Handed out as expiring at the refresh point, so the SDK's own
            // cache renews them in time too.
            let creds = Credentials::new(
                c.access_key_id(),
                c.secret_access_key(),
                Some(c.session_token().to_string()),
                Some(
                    UNIX_EPOCH
                        + std::time::Duration::from_secs(exp.saturating_sub(REFRESH_MARGIN_SECS)),
                ),
                "cua-oidc",
            );
            *cache = Some((creds.clone(), exp));
            Ok(creds)
        }
    }

    impl ProvideCredentials for WebIdentityProvider {
        fn provide_credentials<'a>(&'a self) -> provider::future::ProvideCredentials<'a>
        where
            Self: 'a,
        {
            provider::future::ProvideCredentials::new(self.credentials())
        }
    }

    /// STS refused the exchange.
    #[derive(Debug)]
    pub struct StsRefused {
        role_arn: String,
        detail: String,
        trust: bool,
    }

    impl std::fmt::Display for StsRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if self.trust {
                write!(
                    f,
                    "AWS did not accept Cua's token for {} ({}): the cua-spaces stack is missing, \
                     still creating, or trusts another Cua account (run `cua cloud connect aws` \
                     for the link that creates it)",
                    self.role_arn, self.detail
                )
            } else {
                write!(f, "AWS STS: {} ({})", self.detail, self.role_arn)
            }
        }
    }

    impl std::error::Error for StsRefused {}

    /// The innermost readable message of a credentials failure (what an
    /// EC2 call that could not get credentials says).
    pub fn credentials_detail(e: &CredentialsError) -> String {
        let mut s = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(inner) = src {
            s = inner.to_string();
            src = inner.source();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_fit_sts() {
        let n = session_name("dev_0123456789abcdef01234567", None, "create");
        assert_eq!(n, "cua-dev_0123456789abcdef01234567-user-create");
        let long = session_name(
            "dev_0123456789abcdef01234567",
            Some("my research agent / with spaces and ünïcode and a very long name indeed"),
            "manage",
        );
        assert!(long.len() <= 64, "{long}");
        assert!(long.ends_with("-manage"), "{long}");
        assert!(
            long.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+=,.@_-".contains(c)),
            "{long}"
        );
        assert!(long.starts_with("cua-dev_0123456789abcdef01234567-my-research"));
        assert_eq!(
            session_name("", Some("  "), "check"),
            "cua-device-user-check"
        );
    }

    #[test]
    fn the_device_proof_is_an_es256_jws_bound_to_the_call() {
        let (key, _) = cua_host::DeviceKey::generate().unwrap();
        let req = TokenRequest {
            connection: "conn_1".into(),
            purpose: Purpose::Create,
            audience: "sts.amazonaws.com".into(),
            agent: Some("researcher".into()),
        };
        fn what(r: &TokenRequest) -> ProofFor<'_> {
            ProofFor {
                connection: &r.connection,
                purpose: r.purpose.as_str(),
                agent: r.agent.as_deref(),
            }
        }
        let jws = device_proof(
            &key,
            "https://oidc.example/v1/token",
            "POST",
            "access-token",
            what(&req),
        )
        .unwrap();
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&b64().decode(parts[0]).unwrap()).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&b64().decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["jwk"]["crv"], "P-256");
        assert_eq!(claims["htu"], "https://oidc.example/v1/token");
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["connection"], "conn_1");
        assert_eq!(claims["purpose"], "create");
        assert_eq!(claims["agent"], "researcher");
        assert!(claims["iat"].as_u64().unwrap() > 0);
        assert_eq!(claims["jti"].as_str().unwrap().len(), 32);
        // ath: base64url SHA-256 of the access token.
        assert_eq!(
            claims["ath"],
            b64().encode(ring::digest::digest(&ring::digest::SHA256, b"access-token"))
        );
        // The signature verifies with the key in the header.
        let x = b64().decode(header["jwk"]["x"].as_str().unwrap()).unwrap();
        let y = b64().decode(header["jwk"]["y"].as_str().unwrap()).unwrap();
        let mut point = vec![4u8];
        point.extend(x);
        point.extend(y);
        let pk = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            point,
        );
        let sig = b64().decode(parts[2]).unwrap();
        pk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .expect("the proof verifies with its jwk");
        // No agent: no claim.
        let user = TokenRequest { agent: None, ..req };
        let jws = device_proof(&key, "u", "POST", "t", what(&user)).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&b64().decode(jws.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert!(claims.get("agent").is_none());
    }

    #[test]
    fn issuer_refusals_are_typed() {
        assert_eq!(
            issuer_error(403, r#"{"error":"pending"}"#),
            IssuerError::Pending
        );
        assert_eq!(
            issuer_error(403, r#"{"error":"budget_reached","message":"x"}"#),
            IssuerError::BudgetReached
        );
        assert_eq!(
            issuer_error(403, r#"{"error":"connection_revoked"}"#),
            IssuerError::Revoked
        );
        assert_eq!(
            issuer_error(403, r#"{"error":"connection_pending"}"#),
            IssuerError::Pending
        );
        assert_eq!(
            issuer_error(403, r#"{"error":"budget_required"}"#),
            IssuerError::BudgetRequired
        );
        assert!(matches!(
            issuer_error(401, r#"{"error":"invalid_proof","message":"m"}"#),
            IssuerError::Device(_)
        ));
        assert!(matches!(issuer_error(401, "nope"), IssuerError::SignIn(_)));
        assert!(matches!(
            issuer_error(403, r#"{"error":"device_not_enrolled","message":"dev_1"}"#),
            IssuerError::Device(m) if m == "dev_1"
        ));
        assert!(matches!(issuer_error(500, ""), IssuerError::Other(_)));
    }

    #[cfg(feature = "aws")]
    #[test]
    fn the_quick_create_link_fills_in_the_stack() {
        let u = aws::quick_create_url(
            "us-west-2",
            "https://bucket.s3.amazonaws.com/aws/cua-spaces.yaml",
            "a1b2c3d4e5f6",
            "oidc.cua.ai",
        );
        assert_eq!(
            u,
            "https://console.aws.amazon.com/cloudformation/home?region=us-west-2#/stacks/quickcreate?\
             templateURL=https%3A%2F%2Fbucket.s3.amazonaws.com%2Faws%2Fcua-spaces.yaml\
             &stackName=cua-spaces&param_CuaAccount=a1b2c3d4e5f6&param_IssuerHost=oidc.cua.ai"
        );
        assert_eq!(aws::issuer_host("https://oidc.cua.ai/a/x"), "oidc.cua.ai");
        assert_eq!(aws::issuer_host("http://127.0.0.1:8080"), "127.0.0.1:8080");
        assert_eq!(
            aws::role_arn("123456789012"),
            "arn:aws:iam::123456789012:role/cua-spaces"
        );
        assert_eq!(
            aws::account_of("arn:aws:iam::123456789012:role/cua-spaces"),
            Some("123456789012")
        );
        assert_eq!(aws::account_of("arn:aws:iam::x:role/cua-spaces"), None);
    }

    #[test]
    fn agents_are_named_by_their_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(caller_agent_with(env(&[])), None);
        assert_eq!(
            caller_agent_with(env(&[(AGENT_RUN_VAR, "run-1")])),
            Some("run-1".into())
        );
        assert_eq!(
            caller_agent_with(env(&[(AGENT_VAR, "researcher"), (AGENT_RUN_VAR, "run-1")])),
            Some("researcher".into())
        );
        assert_eq!(caller_agent_with(env(&[(AGENT_VAR, " ")])), None);
        assert_eq!(
            caller_agent_with(env(&[("CLAUDECODE", "1")])),
            Some("claude-code".into())
        );
        assert_eq!(
            caller_agent_with(env(&[("CLAUDECODE", "1"), (AGENT_VAR, "researcher")])),
            Some("researcher".into())
        );
    }
}
