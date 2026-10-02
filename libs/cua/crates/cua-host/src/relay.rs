//! Client for the `cua-relay` machine directory (account mode).
//!
//! The relay authenticates people with their cua.ai account token (an OIDC
//! access token from `cua auth login` or the app's sign-in) and hosts with
//! the machine token it issued at registration. Wire JSON is snake_case.
//!
//! ```text
//! POST   /v1/machines                     register (account)  -> Registration
//! GET    /v1/machines                     owned + shared      -> {"machines": [Machine]}
//! GET    /v1/machines/{id}                account or machine  -> Machine
//! PATCH  /v1/machines/{id}                owner               -> Machine
//! DELETE /v1/machines/{id}                owner or machine    -> 204
//! POST   /v1/machines/{id}/stop-sharing   owner or machine    -> Machine
//! POST   /v1/machines/{id}/start-sharing  owner or machine    -> Machine
//! POST   /v1/devices                      register device     -> Enrollment
//! POST   /v1/devices/session              device proof        -> session token
//! POST   /v1/devices/approve              enrolled device     -> {device}
//! GET    /v1/devices                      account             -> {devices}
//! PATCH  /v1/devices/{id}                 enrolled device     -> DeviceView
//! DELETE /v1/devices/{id}                 enrolled device     -> DeviceView
//! GET    /v1/audit                        account             -> {events}
//! ```
//!
//! Account calls other than registering a machine carry the client
//! device's session ([`DEVICE_SESSION_HEADER`], see [`crate::device`]).

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The relay the app and CLI use unless configured otherwise.
pub const DEFAULT_RELAY_URL: &str = "https://relay.cua.ai";

/// `CUA_RELAY_URL` when set and non-empty, else [`DEFAULT_RELAY_URL`].
pub fn relay_url_from_env() -> String {
    std::env::var("CUA_RELAY_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_RELAY_URL.to_string())
}

/// Supplies a fresh cua.ai account access token (refreshing as needed).
#[async_trait::async_trait]
pub trait AccountTokens: Send + Sync {
    /// A valid access token (without the `Bearer ` prefix).
    async fn access_token(&self) -> Result<String>;
}

/// A fixed account token (tests, bindings that pass a token per call).
#[derive(Clone)]
pub struct StaticToken(pub String);

#[async_trait::async_trait]
impl AccountTokens for StaticToken {
    async fn access_token(&self) -> Result<String> {
        if self.0.trim().is_empty() {
            return Err(Error::Unauthenticated(
                "no cua.ai account token; sign in first".into(),
            ));
        }
        Ok(self.0.clone())
    }
}

/// No account (direct mode, or not signed in).
pub struct NoAccount;

#[async_trait::async_trait]
impl AccountTokens for NoAccount {
    async fn access_token(&self) -> Result<String> {
        Err(Error::Unauthenticated(
            "not signed in to cua.ai (run `cua auth login` or sign in from the app)".into(),
        ))
    }
}

/// Machine owner / connected client identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Account (or user) id.
    pub id: String,
    /// Email, when the identity provider shares it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Somebody connected to a machine through the relay right now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedClient {
    /// User id.
    pub id: String,
    /// Email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Open streams.
    #[serde(default)]
    pub streams: u32,
    /// Unix seconds of the first open stream.
    #[serde(default)]
    pub since: u64,
}

/// A machine row of the directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// Machine id (`[a-z0-9-]{8,64}`).
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Owner.
    #[serde(default)]
    pub owner: Identity,
    /// `owner` or `shared` (the caller's role).
    #[serde(default)]
    pub role: String,
    /// Connected to the relay now.
    #[serde(default)]
    pub online: bool,
    /// Accepting clients (false after "Stop sharing").
    #[serde(default)]
    pub sharing: bool,
    /// spacesd version reported at connect.
    #[serde(default)]
    pub version: String,
    /// Unix seconds of the current session.
    #[serde(default)]
    pub connected_at: Option<u64>,
    /// `<relay>/m/<id>`: the spacesd URL clients connect to.
    #[serde(default)]
    pub url: String,
    /// Allowlist (owner view only): accounts with full use (editors).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Accounts that may only watch (owner view only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewers: Vec<String>,
    /// Clients connected right now.
    #[serde(default)]
    pub clients: Vec<ConnectedClient>,
    /// For a Space a host provides: the machine id of that host (set at
    /// registration). Absent for hosts and every other machine, and from
    /// relays that predate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// What the registering client said about the machine (a Space in the
    /// owner's own cloud: `cua.cloud.provider`, `cua.cloud.place`, ...); no
    /// secrets. Empty from relays that predate it.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub meta: std::collections::BTreeMap<String, String>,
    /// Registered with an enrolled device's signature or a sign-in that
    /// proved MFA, or confirmed since by an enrolled device (S5). `true`
    /// from a relay that predates this check, so an existing machine is
    /// never retroactively flagged "new".
    #[serde(default = "confirmed_default")]
    pub confirmed: bool,
}

fn confirmed_default() -> bool {
    true
}

/// `POST /v1/machines` body.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterRequest {
    /// Machine id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Accounts (ids or emails) allowed besides the owner.
    #[serde(default)]
    pub allow: Vec<String>,
    /// For a Space a host provides: the host's machine id (the caller must
    /// be able to use that host). Lists group such Spaces under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// [`Machine::meta`] to record (older relays ignore it).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub meta: std::collections::BTreeMap<String, String>,
}

/// `POST /v1/machines` response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Registration {
    /// The machine.
    pub machine: Machine,
    /// Long-lived credential the host joins with (`cmt_…`).
    pub machine_token: String,
    /// The relay's signing keys (JWKS), pinned by the host.
    #[serde(default)]
    pub jwks: serde_json::Value,
}

/// `PATCH /v1/machines/{id}` body.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachinePatch {
    /// New name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement allowlist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    /// Replacement view-only list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewers: Option<Vec<String>>,
    /// Sharing on/off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharing: Option<bool>,
}

/// `GET /v1/info`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayInfo {
    /// Public base URL.
    #[serde(default)]
    pub public_url: String,
    /// Assertion issuer.
    #[serde(default)]
    pub issuer: String,
    /// Accepts account tokens.
    #[serde(default)]
    pub account_auth: bool,
    /// Accepts static relay tokens.
    #[serde(default)]
    pub static_tokens: bool,
}

#[derive(Deserialize)]
struct MachineList {
    #[serde(default)]
    machines: Vec<Machine>,
}

/// Header carrying the client device's session token.
pub const DEVICE_SESSION_HEADER: &str = "x-cua-device-session";
/// Prefix of a device id.
pub const DEVICE_ID_PREFIX: &str = "dev_";
/// Response header the relay sets while it lets an unenrolled device
/// through during its migration grace period.
pub const ENROLLMENT_HEADER: &str = "x-cua-device-enrollment";
/// Header a host re-running setup sends its current machine token in.
pub const MACHINE_AUTHORIZATION_HEADER: &str = "x-cua-machine-authorization";

/// Where a client device stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    /// Waiting for an approval.
    #[default]
    Pending,
    /// Enrolled.
    Enrolled,
    /// Re-verification is due.
    Expired,
    /// Revoked.
    Revoked,
}

/// A client device of the account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceView {
    /// `dev_…`.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// State.
    #[serde(default)]
    pub state: DeviceState,
    /// Registration time (Unix seconds).
    #[serde(default)]
    pub created_at: u64,
    /// Last enrollment.
    #[serde(default)]
    pub enrolled_at: Option<u64>,
    /// Re-verification due at.
    #[serde(default)]
    pub enrolled_until: Option<u64>,
    /// Approving device id, or `bootstrap:…`.
    #[serde(default)]
    pub enrolled_by: Option<String>,
    /// Last session.
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// Revocation time.
    #[serde(default)]
    pub revoked_at: Option<u64>,
    /// When a pending code expires.
    #[serde(default)]
    pub code_expires: Option<u64>,
    /// The caller's own device.
    #[serde(default)]
    pub current: bool,
    /// Operating system the device reported when it registered (`macos`,
    /// `windows`, `linux`); empty from relays that predate it.
    #[serde(default)]
    pub platform: String,
}

/// `GET /v1/devices` response: the account's devices and when the relay
/// stops letting unenrolled devices through.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceListing {
    /// Devices.
    #[serde(default)]
    pub devices: Vec<DeviceView>,
    /// End of the relay's grace period (Unix seconds; 0 when unknown).
    #[serde(default)]
    pub enforce_after: u64,
}

/// `POST /v1/devices` response.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrollment {
    /// The device.
    pub device: DeviceView,
    /// One-time code to confirm from an enrolled device (pending only).
    #[serde(default)]
    pub code: Option<String>,
    /// When the relay stops letting unenrolled devices through.
    #[serde(default)]
    pub enforce_after: u64,
    /// Enrollment lifetime.
    #[serde(default)]
    pub ttl_secs: u64,
    /// Older devices of this machine the enrollment replaced (a re-key);
    /// empty from relays that predate it.
    #[serde(default)]
    pub superseded: Vec<String>,
}

/// One audit event of the account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unix seconds.
    pub ts: u64,
    /// Kind (`machine_access`, `device_enrolled`, `share_added`, …).
    pub kind: String,
    /// Acting device.
    #[serde(default)]
    pub device: Option<String>,
    /// Machine.
    #[serde(default)]
    pub machine: Option<String>,
    /// Other party.
    #[serde(default)]
    pub subject: Option<String>,
    /// Detail.
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Deserialize)]
struct DeviceReply {
    device: DeviceView,
}

#[derive(Deserialize)]
struct AuditList {
    #[serde(default)]
    events: Vec<AuditEvent>,
}

/// `POST /v1/devices/session` response.
#[derive(Clone, Debug, Deserialize)]
pub struct DeviceSession {
    /// Session token.
    pub session: String,
    /// Expiry (Unix seconds).
    pub expires_at: u64,
}

/// HTTP client for one relay.
#[derive(Clone, Debug)]
pub struct RelayClient {
    base: String,
    http: reqwest::Client,
    device_session: Option<String>,
}

impl RelayClient {
    /// A client for `base` (`https://relay.cua.ai`, `http://127.0.0.1:8080`,
    /// `wss://…` is accepted and mapped to `https://…`).
    pub fn new(base: &str) -> Result<Self> {
        let base = normalize_base(base)?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Relay(e.to_string()))?;
        Ok(Self {
            base,
            http,
            device_session: None,
        })
    }

    /// Sends `session` (a client device session) with every call.
    pub fn with_device_session(mut self, session: Option<String>) -> Self {
        self.device_session = session.filter(|s| !s.is_empty());
        self
    }

    /// Normalized base URL (no trailing slash).
    pub fn base(&self) -> &str {
        &self.base
    }

    /// `<base>/m/<id>`: where clients reach machine `id`.
    pub fn machine_url(&self, id: &str) -> String {
        format!("{}/m/{id}", self.base)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        bearer: &str,
    ) -> Result<T> {
        let mut req = req.bearer_auth(bearer);
        if let Some(session) = &self.device_session {
            req = req.header(DEVICE_SESSION_HEADER, session);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Relay(format!("{}: {e}", self.base)))?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| Error::Relay(e.to_string()))?;
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &body));
        }
        let body = if body.trim().is_empty() {
            "null"
        } else {
            &body
        };
        serde_json::from_str(body).map_err(|e| Error::Relay(format!("bad relay response: {e}")))
    }

    /// `GET /v1/info` (no auth).
    pub async fn info(&self) -> Result<RelayInfo> {
        let resp = self
            .http
            .get(self.url("/v1/info"))
            .send()
            .await
            .map_err(|e| Error::Relay(format!("{}: {e}", self.base)))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|e| Error::Relay(format!("bad relay response: {e}")))
    }

    /// `GET /.well-known/jwks.json` (no auth): the relay's assertion
    /// signing keys, as a registration returns them (a machine pins them).
    pub async fn jwks(&self) -> Result<serde_json::Value> {
        let resp = self
            .http
            .get(self.url("/.well-known/jwks.json"))
            .send()
            .await
            .map_err(|e| Error::Relay(format!("{}: {e}", self.base)))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|e| Error::Relay(format!("bad relay response: {e}")))
    }

    /// Registers (or re-registers, rotating the machine token) a machine
    /// owned by the account behind `account_token`.
    pub async fn register(
        &self,
        account_token: &str,
        req: &RegisterRequest,
    ) -> Result<Registration> {
        self.send(
            self.http.post(self.url("/v1/machines")).json(req),
            account_token,
        )
        .await
    }

    /// Like [`RelayClient::register`] for a host re-running setup: sends
    /// its current machine token so the relay lets it rotate the token
    /// without an enrolled client device.
    pub async fn register_as_machine(
        &self,
        account_token: &str,
        machine_token: Option<&str>,
        req: &RegisterRequest,
    ) -> Result<Registration> {
        let mut builder = self.http.post(self.url("/v1/machines")).json(req);
        if let Some(t) = machine_token.filter(|t| !t.is_empty()) {
            builder = builder.header(MACHINE_AUTHORIZATION_HEADER, format!("Bearer {t}"));
        }
        self.send(builder, account_token).await
    }

    /// Registers the client device with `public_key` (see
    /// [`crate::device`]).
    pub async fn register_device(
        &self,
        account_token: &str,
        body: &serde_json::Value,
    ) -> Result<Enrollment> {
        self.send(
            self.http.post(self.url("/v1/devices")).json(body),
            account_token,
        )
        .await
    }

    /// Opens a device session with a signed timestamp.
    pub async fn device_session(
        &self,
        account_token: &str,
        body: &serde_json::Value,
    ) -> Result<DeviceSession> {
        self.send(
            self.http.post(self.url("/v1/devices/session")).json(body),
            account_token,
        )
        .await
    }

    /// Approves a pending or expired device by code or id (from an enrolled
    /// device: needs [`RelayClient::with_device_session`]). A "code" that
    /// is a device id (`dev_…`) is sent as the id.
    pub async fn approve_device(
        &self,
        account_token: &str,
        code: Option<&str>,
        device_id: Option<&str>,
    ) -> Result<DeviceView> {
        let (code, device_id) = match (code.map(str::trim), device_id) {
            (Some(c), None) if c.starts_with(DEVICE_ID_PREFIX) => (None, Some(c)),
            other => other,
        };
        let reply: DeviceReply = self
            .send(
                self.http
                    .post(self.url("/v1/devices/approve"))
                    .json(&serde_json::json!({"code": code, "device_id": device_id})),
                account_token,
            )
            .await
            .map_err(|e| match (e, code) {
                // Relays before the explicit error answer "no such device".
                (Error::NotFound(m), Some(code)) if m.ends_with("no such device") => {
                    Error::NotFound(format!(
                        "no device is waiting with the code {code} (codes expire after 10 minutes); approve by id instead: `cua devices approve <device id>` (ids in `cua devices ls`)"
                    ))
                }
                (e, _) => e,
            })?;
        Ok(reply.device)
    }

    /// The account's devices.
    pub async fn devices(&self, account_token: &str) -> Result<Vec<DeviceView>> {
        Ok(self.device_listing(account_token).await?.devices)
    }

    /// The account's devices and the end of the relay's grace period.
    pub async fn device_listing(&self, account_token: &str) -> Result<DeviceListing> {
        self.send(self.http.get(self.url("/v1/devices")), account_token)
            .await
    }

    /// Renames a device (from an enrolled device).
    pub async fn rename_device(
        &self,
        account_token: &str,
        id: &str,
        name: &str,
    ) -> Result<DeviceView> {
        self.send(
            self.http
                .patch(self.url(&format!("/v1/devices/{id}")))
                .json(&serde_json::json!({ "name": name })),
            account_token,
        )
        .await
    }

    /// Revokes a device (from an enrolled device).
    pub async fn revoke_device(&self, account_token: &str, id: &str) -> Result<DeviceView> {
        self.send(
            self.http.delete(self.url(&format!("/v1/devices/{id}"))),
            account_token,
        )
        .await
    }

    /// The account's audit log, newest last.
    pub async fn audit(&self, account_token: &str, limit: usize) -> Result<Vec<AuditEvent>> {
        let list: AuditList = self
            .send(
                self.http.get(self.url(&format!("/v1/audit?limit={limit}"))),
                account_token,
            )
            .await?;
        Ok(list.events)
    }

    /// Machines the account owns or that are shared with it.
    pub async fn machines(&self, account_token: &str) -> Result<Vec<Machine>> {
        let list: MachineList = self
            .send(self.http.get(self.url("/v1/machines")), account_token)
            .await?;
        Ok(list.machines)
    }

    /// One machine (`credential` = account token or the machine token).
    pub async fn machine(&self, credential: &str, id: &str) -> Result<Machine> {
        self.send(
            self.http.get(self.url(&format!("/v1/machines/{id}"))),
            credential,
        )
        .await
    }

    /// Renames / changes the allowlist / toggles sharing (owner).
    pub async fn patch(
        &self,
        account_token: &str,
        id: &str,
        patch: &MachinePatch,
    ) -> Result<Machine> {
        self.send(
            self.http
                .patch(self.url(&format!("/v1/machines/{id}")))
                .json(patch),
            account_token,
        )
        .await
    }

    /// Vouches for a machine that registered without proof (S5): shows it
    /// as "new" no longer. Owner only, from an enrolled device
    /// ([`RelayClient::with_device_session`]); a machine cannot confirm
    /// itself.
    pub async fn confirm(&self, account_token: &str, id: &str) -> Result<Machine> {
        self.send(
            self.http
                .post(self.url(&format!("/v1/machines/{id}/confirm")))
                .json(&serde_json::json!({})),
            account_token,
        )
        .await
    }

    /// Deletes the machine and revokes its token (owner or machine token).
    pub async fn delete(&self, credential: &str, id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send(
                self.http.delete(self.url(&format!("/v1/machines/{id}"))),
                credential,
            )
            .await?;
        Ok(())
    }

    /// Cuts every client and refuses new ones (owner or machine token).
    pub async fn stop_sharing(&self, credential: &str, id: &str) -> Result<Machine> {
        self.send(
            self.http
                .post(self.url(&format!("/v1/machines/{id}/stop-sharing"))),
            credential,
        )
        .await
    }

    /// Accepts clients again (owner or machine token).
    pub async fn start_sharing(&self, credential: &str, id: &str) -> Result<Machine> {
        self.send(
            self.http
                .post(self.url(&format!("/v1/machines/{id}/start-sharing"))),
            credential,
        )
        .await
    }
}

fn http_error(status: u16, body: &str) -> Error {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        401 => Error::Unauthenticated(format!("relay: {message}")),
        403 => Error::PermissionDenied(format!("relay: {message}")),
        404 => Error::NotFound(format!("relay: {message}")),
        409 => Error::Conflict(format!("relay: {message}")),
        _ => Error::Relay(format!("HTTP {status}: {message}")),
    }
}

/// `wss://x/` → `https://x`, `ws://` → `http://`, trims the trailing slash.
pub fn normalize_base(base: &str) -> Result<String> {
    let base = base.trim();
    let mut url = url::Url::parse(base)
        .map_err(|e| Error::InvalidArgument(format!("relay URL {base:?}: {e}")))?;
    let scheme = match url.scheme() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        other => {
            return Err(Error::InvalidArgument(format!(
                "relay URL scheme {other:?} (use https:// or http://)"
            )));
        }
    };
    if url.scheme() != scheme {
        // `set_scheme` refuses special↔special changes only for file etc.
        let rest = &base[base.find("://").map(|i| i + 3).unwrap_or(0)..];
        url = url::Url::parse(&format!("{scheme}://{rest}"))
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
    }
    // The account and machine tokens travel to the relay: plain HTTP only
    // to this machine (local relays in development and tests).
    if scheme == "http" && !is_loopback_host(&url) {
        return Err(Error::InvalidArgument(format!(
            "relay URL {base:?}: use https:// (plain http is only allowed for localhost)"
        )));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relay_urls() {
        assert_eq!(
            normalize_base("https://relay.cua.ai/").unwrap(),
            "https://relay.cua.ai"
        );
        assert_eq!(
            normalize_base("wss://r.example").unwrap(),
            "https://r.example"
        );
        assert_eq!(
            normalize_base("ws://127.0.0.1:8080/base/").unwrap(),
            "http://127.0.0.1:8080/base"
        );
        assert_eq!(
            normalize_base("http://localhost:9000").unwrap(),
            "http://localhost:9000"
        );
        // Tokens never travel in clear text off this machine.
        assert!(normalize_base("http://relay.example").is_err());
        assert!(normalize_base("ws://10.0.0.5:8080").is_err());
        assert!(normalize_base("http://127.0.0.1.nip.io").is_err());
        assert!(normalize_base("ftp://x").is_err());
        assert!(normalize_base("not a url").is_err());
    }

    #[test]
    fn machine_rows_parse_with_missing_optional_fields() {
        let m: Machine = serde_json::from_str(
            r#"{"id":"abcd1234","name":"mini","owner":{"id":"u1"},"role":"owner","online":true,
                "sharing":true,"version":"0.1.0","connected_at":null,"url":"https://r/m/abcd1234",
                "allow":[],"clients":[{"id":"u2","email":"b@x","streams":2,"since":5}]}"#,
        )
        .unwrap();
        assert_eq!(m.clients[0].streams, 2);
        assert_eq!(m.connected_at, None);
        let bare: Machine = serde_json::from_str(r#"{"id":"abcd1234"}"#).unwrap();
        assert!(!bare.online);
    }
}
