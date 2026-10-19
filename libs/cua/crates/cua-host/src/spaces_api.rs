//! The Spaces API (`https://api.cua.ai`, `https://api.staging.cua.ai`
//! under `CUA_ENV=staging`): the account's identity and entitlements
//! (`GET /v1/me`), its organizations (Teams) with members and pending
//! invites, and pairing codes. Account, billing and team administration
//! (invites, roles, plans, the Stripe portal) live in the account app
//! (`https://app.cua.ai`); the apps only read the team and link out.
//!
//! The contract is trycua/cloud `src/services/spaces-control-plane`. The
//! bearer is the account's access token (audience `cua-spaces-api`).

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One organization (a Team) the account belongs to, from `GET /v1/me`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiOrg {
    #[serde(default)]
    pub id: String,
    /// The org's slug (always set by the API; the apps' fallback name).
    #[serde(default)]
    pub alias: String,
    /// The display name, when the org has one.
    #[serde(default)]
    pub name: String,
    /// `owner`, `admin` or `member`.
    #[serde(default)]
    pub role: String,
    /// `free`, `pro`, `teams`, ...
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub seats: i64,
    /// Paid features the org has (`team_machines`, `space_sharing`,
    /// `device_approval`, ...).
    #[serde(default)]
    pub features: Vec<String>,
}

impl ApiOrg {
    /// The name to show: the display name, else the alias, else the id.
    pub fn label(&self) -> &str {
        [&self.name, &self.alias, &self.id]
            .into_iter()
            .find(|s| !s.is_empty())
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Whether the org's plan includes `feature`.
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// Whether the account may administer the org (add machines, approve
    /// devices, invite).
    pub fn manages(&self) -> bool {
        matches!(self.role.as_str(), "owner" | "admin")
    }
}

/// `GET /v1/orgs/{org}`: one org with its plan and seats.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiOrgDetail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub seats_paid: i64,
    #[serde(default)]
    pub seats_used: i64,
}

/// A machine bound to an org (`GET /v1/orgs/{org}/machines`): every
/// member opens it from an approved device. `machine_id` is the relay
/// record id (`relay:<machine_id>`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiOrgMachine {
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub added_by: String,
    #[serde(default)]
    pub added_at: String,
}

/// A member device's approval in an org (`GET /v1/orgs/{org}/devices`).
/// `device_id` is the relay device id (`dev_…`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiDeviceApproval {
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub device_id: String,
    /// `pending`, `approved` or `denied`.
    #[serde(default)]
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
}

/// `GET /v1/pairing-codes/{id}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiPairingState {
    #[serde(default)]
    pub id: String,
    /// `pending`, `redeemed` or `expired`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub expires_at: String,
}

/// What the clients say when the Spaces API has no Teams (org) routes, or
/// cannot be reached. The clients never fall back to the relay's retired
/// `team-…` teams.
pub const TEAMS_NOT_AVAILABLE: &str =
    "Teams are not available on this server yet. Try again later, or update Cua.";

/// What a team operation says while it waits for a just-paid plan to
/// reach the API (see [`SpacesApiClient::finishing_setup`]).
pub const FINISHING_SETUP: &str = "Finishing setup…";

/// How long a team operation keeps retrying a `402 feature_required` (the
/// plan was just bought and its entitlements are on their way).
pub const SETUP_RETRY_WINDOW: Duration = Duration::from_secs(60);

/// Whether `e` is the API's `402 feature_required` (the org's plan lacks a
/// feature, or a just-bought plan has not reached the API yet).
pub fn is_feature_required(e: &Error) -> bool {
    e.to_string().contains("HTTP 402 (feature_required")
}

/// Whether `e` says Teams are not available on this server (see
/// [`TEAMS_NOT_AVAILABLE`]).
pub fn is_teams_unavailable(e: &Error) -> bool {
    e.to_string().starts_with(TEAMS_NOT_AVAILABLE)
}

/// Whether `e` is worth retrying: a just-paid plan (402 feature_required)
/// or a dependency of the API that is down for a moment (503).
fn is_retryable(e: &Error) -> bool {
    is_feature_required(e) || e.to_string().contains("HTTP 503")
}

/// `GET /v1/me`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiMe {
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub orgs: Vec<ApiOrg>,
    #[serde(default)]
    pub entitlements: ApiEntitlements,
    /// The opaque account id usage data is linked to (`acct_` and 32 hex,
    /// never the email or the subject). Absent while linking is off on the
    /// server (`SPACES_CP_LINK_USAGE`), so no client links anything then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics_id: Option<String>,
}

/// The parts of the entitlements the apps read. Unknown plans mean free
/// and missing features mean not entitled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiEntitlements {
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// A member of an org.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiMember {
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub role: String,
}

/// A pending invite.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiInvite {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub email: String,
}

/// A one-time pairing code (`POST /v1/pairing-codes`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiPairing {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub expires_at: String,
}

/// One org with its people.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TeamDetail {
    pub org: ApiOrg,
    pub members: Vec<ApiMember>,
    /// Empty for plain members: only owners and admins may list invites.
    pub invites: Vec<ApiInvite>,
}

/// The account's Teams: who it is, its plan, and every org it is in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TeamsSnapshot {
    pub email: String,
    pub plan: String,
    pub teams: Vec<TeamDetail>,
}

/// A path segment (org ids, aliases, device and machine ids are plain, but
/// never trust them into a URL as is).
fn seg(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.trim().as_bytes()).collect()
}

/// Whether `id` is a team id of the relay's retired teams
/// (`team-<16 hex>`), which the clients no longer use.
pub fn is_legacy_team_id(id: &str) -> bool {
    id.strip_prefix("team-")
        .is_some_and(|h| h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether `id` can name an org (Team): its Keycloak id (a UUID) or its
/// alias (lowercase letters, digits and dashes). Not a retired `team-…` id.
pub fn is_org_ref(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !is_legacy_team_id(id)
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A Spaces API client.
#[derive(Clone)]
pub struct SpacesApiClient {
    base: String,
    http: reqwest::Client,
}

/// The API's error code (`{"error":"org_not_found"}`), when it sent one.
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

/// The error of an org (Teams) route: [`http_error`], except that a route
/// the server does not have (404 without one of the API's own codes, 410
/// Gone, 501) means Teams are not available there, and `not_owner` gets
/// the words the apps show.
fn org_error(status: u16, body: &str) -> Error {
    let code = error_code(body);
    let unavailable = match status {
        410 | 501 => true,
        404 => !code.as_deref().is_some_and(|c| c.ends_with("_not_found")),
        _ => false,
    };
    if unavailable {
        return Error::Internal(format!("{TEAMS_NOT_AVAILABLE} (HTTP {status})"));
    }
    if status == 403 && code.as_deref() == Some("not_owner") {
        return Error::PermissionDenied("Only the machine's owner can add it to a team.".into());
    }
    http_error(status, body)
}

fn http_error(status: u16, body: &str) -> Error {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            let e = v.get("error")?.as_str()?.to_string();
            let d = v.get("detail").and_then(|d| d.as_str()).unwrap_or("");
            Some(if d.is_empty() { e } else { format!("{e}: {d}") })
        })
        .unwrap_or_else(|| body.trim().chars().take(200).collect());
    let msg = format!(
        "Spaces API: HTTP {status}{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        }
    );
    match status {
        401 => Error::Unauthenticated(msg),
        402 | 403 => Error::PermissionDenied(msg),
        404 => Error::NotFound(msg),
        409 => Error::Conflict(msg),
        _ => Error::Internal(msg),
    }
}

impl SpacesApiClient {
    /// A client for the API at `base` (`https://api.cua.ai`).
    pub fn new(base: &str) -> Result<Self> {
        let url = url::Url::parse(base.trim())
            .map_err(|e| Error::InvalidArgument(format!("Spaces API URL {base:?}: {e}")))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
        if url.scheme() != "https" && !loopback {
            return Err(Error::InvalidArgument(format!(
                "Spaces API URL {base:?}: use https"
            )));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(Self {
            base: base.trim().trim_end_matches('/').to_string(),
            http,
        })
    }

    /// The client for the current environment (`CUA_SPACES_API_URL`, else
    /// api.cua.ai, or api.staging.cua.ai under `CUA_ENV=staging`).
    pub fn current() -> Result<Self> {
        Self::new(&cua_home::env::spaces_api_url())
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        token: &str,
    ) -> Result<T> {
        self.send_as(req, token, false).await
    }

    /// Sends an org (Teams) request: see [`org_error`]. An API that cannot
    /// be reached at all also means Teams are not available.
    async fn send_org<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        token: &str,
    ) -> Result<T> {
        self.send_as(req, token, true).await
    }

    async fn send_as<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        token: &str,
        org: bool,
    ) -> Result<T> {
        let resp = req.bearer_auth(token).send().await.map_err(|e| {
            if org {
                Error::Internal(format!(
                    "{TEAMS_NOT_AVAILABLE} (the Spaces API at {} cannot be reached: {e})",
                    self.base
                ))
            } else {
                Error::Internal(format!("Spaces API {}: {e}", self.base))
            }
        })?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if !status.is_success() {
            return Err(if org {
                org_error(status.as_u16(), &body)
            } else {
                http_error(status.as_u16(), &body)
            });
        }
        serde_json::from_str(if body.trim().is_empty() {
            "null"
        } else {
            &body
        })
        .map_err(|e| Error::Internal(format!("bad Spaces API response: {e}")))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str, token: &str) -> Result<T> {
        self.send(self.http.get(format!("{}{path}", self.base)), token)
            .await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn org_get<T: serde::de::DeserializeOwned>(&self, path: &str, token: &str) -> Result<T> {
        self.send_org(self.http.get(self.url(path)), token).await
    }

    async fn org_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        self.send_org(self.http.post(self.url(path)).json(&body), token)
            .await
    }

    async fn org_delete(&self, path: &str, token: &str) -> Result<()> {
        self.send_org::<serde_json::Value>(self.http.delete(self.url(path)), token)
            .await
            .map(|_| ())
    }

    /// `GET /v1/me`: the account, its plan and its orgs.
    pub async fn me(&self, token: &str) -> Result<ApiMe> {
        self.org_get("/v1/me", token).await
    }

    /// `GET /v1/me/entitlements` (the raw answer).
    pub async fn entitlements(&self, token: &str) -> Result<serde_json::Value> {
        self.get("/v1/me/entitlements", token).await
    }

    /// `GET /v1/orgs/{org}/members`.
    pub async fn members(&self, token: &str, org: &str) -> Result<Vec<ApiMember>> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            members: Vec<ApiMember>,
        }
        Ok(self
            .org_get::<R>(&format!("/v1/orgs/{}/members", seg(org)), token)
            .await?
            .members)
    }

    /// `GET /v1/orgs/{org}/invites` (owners and admins).
    pub async fn invites(&self, token: &str, org: &str) -> Result<Vec<ApiInvite>> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            invites: Vec<ApiInvite>,
        }
        Ok(self
            .org_get::<R>(&format!("/v1/orgs/{}/invites", seg(org)), token)
            .await?
            .invites)
    }

    /// `POST /v1/pairing-codes`: a one-time code to enroll a machine, for
    /// the account or (`org` set) for one of its teams.
    pub async fn create_pairing(&self, token: &str, org: Option<&str>) -> Result<ApiPairing> {
        let body = match org {
            Some(o) => serde_json::json!({ "org": o }),
            None => serde_json::json!({}),
        };
        self.send(
            self.http
                .post(format!("{}/v1/pairing-codes", self.base))
                .json(&body),
            token,
        )
        .await
    }

    /// The org (Team) `query` names, from `GET /v1/me`: its id, its alias,
    /// or its display name (case-insensitive). The account's orgs come
    /// from its token: one made a moment ago shows up after a new sign-in.
    pub async fn resolve_org(&self, token: &str, query: &str) -> Result<ApiOrg> {
        let q = query.trim();
        let q = q.strip_prefix("team:").unwrap_or(q);
        if is_legacy_team_id(q) {
            return Err(Error::InvalidArgument(format!(
                "{q} is a team id of the old Teams, which this version no longer uses: \
                 use the team's id from `cua team ls`"
            )));
        }
        let me: ApiMe = self.org_get("/v1/me", token).await?;
        if let Some(o) = me.orgs.iter().find(|o| o.id == q || o.alias == q) {
            return Ok(o.clone());
        }
        let named: Vec<_> = me
            .orgs
            .iter()
            .filter(|o| o.name.eq_ignore_ascii_case(q) || o.alias.eq_ignore_ascii_case(q))
            .collect();
        match named.as_slice() {
            [o] => Ok((*o).clone()),
            [] => Err(Error::NotFound(format!(
                "no team of yours is called {q:?} (`cua team ls`; a team made or joined a \
                 moment ago shows up after `cua auth login`)"
            ))),
            _ => Err(Error::InvalidArgument(format!(
                "several of your teams are called {q:?}: use the team id (`cua team ls`)"
            ))),
        }
    }

    /// `POST /v1/orgs`: makes a team; the account is its owner. The
    /// account's token lists it after the next sign-in.
    pub async fn create_org(&self, token: &str, name: &str) -> Result<ApiOrg> {
        self.org_post("/v1/orgs", token, serde_json::json!({ "name": name }))
            .await
    }

    /// `GET /v1/orgs/{org}`.
    pub async fn org(&self, token: &str, org: &str) -> Result<ApiOrgDetail> {
        self.org_get(&format!("/v1/orgs/{}", seg(org)), token).await
    }

    /// `DELETE /v1/orgs/{org}` (owner; only without a live subscription).
    pub async fn delete_org(&self, token: &str, org: &str) -> Result<()> {
        self.org_delete(&format!("/v1/orgs/{}", seg(org)), token)
            .await
    }

    /// `POST /v1/orgs/{org}/leave`.
    pub async fn leave_org(&self, token: &str, org: &str) -> Result<()> {
        self.org_post::<serde_json::Value>(
            &format!("/v1/orgs/{}/leave", seg(org)),
            token,
            serde_json::json!({}),
        )
        .await
        .map(|_| ())
    }

    /// `POST /v1/orgs/{org}/invites` (owner or admin): Keycloak mails the
    /// invitation.
    pub async fn invite(&self, token: &str, org: &str, email: &str, role: &str) -> Result<()> {
        self.org_post::<serde_json::Value>(
            &format!("/v1/orgs/{}/invites", seg(org)),
            token,
            serde_json::json!({ "email": email, "role": role }),
        )
        .await
        .map(|_| ())
    }

    /// `DELETE /v1/orgs/{org}/invites/{id}`.
    pub async fn revoke_invite(&self, token: &str, org: &str, id: &str) -> Result<()> {
        self.org_delete(&format!("/v1/orgs/{}/invites/{}", seg(org), seg(id)), token)
            .await
    }

    /// `POST /v1/orgs/{org}/invites/claim`: after joining from the mailed
    /// invitation, applies the role the invite promised. Returns the role.
    pub async fn claim_invite(&self, token: &str, org: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            role: String,
        }
        Ok(self
            .org_post::<R>(
                &format!("/v1/orgs/{}/invites/claim", seg(org)),
                token,
                serde_json::json!({}),
            )
            .await?
            .role)
    }

    /// `PATCH /v1/orgs/{org}/members/{sub}` (owner): `admin` or `member`.
    pub async fn set_member_role(
        &self,
        token: &str,
        org: &str,
        sub: &str,
        role: &str,
    ) -> Result<()> {
        self.send_org::<serde_json::Value>(
            self.http
                .patch(self.url(&format!("/v1/orgs/{}/members/{}", seg(org), seg(sub))))
                .json(&serde_json::json!({ "role": role })),
            token,
        )
        .await
        .map(|_| ())
    }

    /// `DELETE /v1/orgs/{org}/members/{sub}` (owner or admin).
    pub async fn remove_member(&self, token: &str, org: &str, sub: &str) -> Result<()> {
        self.org_delete(
            &format!("/v1/orgs/{}/members/{}", seg(org), seg(sub)),
            token,
        )
        .await
    }

    /// `GET /v1/orgs/{org}/machines`: the team's machines.
    pub async fn org_machines(&self, token: &str, org: &str) -> Result<Vec<ApiOrgMachine>> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            machines: Vec<ApiOrgMachine>,
        }
        Ok(self
            .org_get::<R>(&format!("/v1/orgs/{}/machines", seg(org)), token)
            .await?
            .machines)
    }

    /// `POST /v1/orgs/{org}/machines` (owner or admin): makes the relay
    /// machine (or Space) `machine_id` a team machine. Already a team
    /// machine is not an error. A `402 feature_required` is returned as is;
    /// [`Self::finishing_setup`] retries it after a checkout.
    pub async fn add_org_machine(
        &self,
        token: &str,
        org: &str,
        machine_id: &str,
        platform: &str,
        label: &str,
    ) -> Result<()> {
        let label: String = label.chars().take(120).collect();
        match self
            .org_post::<serde_json::Value>(
                &format!("/v1/orgs/{}/machines", seg(org)),
                token,
                serde_json::json!({ "machine_id": machine_id, "platform": platform, "label": label }),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::Conflict(m)) if m.contains("already_added") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// `DELETE /v1/orgs/{org}/machines/{id}` (owner or admin).
    pub async fn remove_org_machine(&self, token: &str, org: &str, machine_id: &str) -> Result<()> {
        self.org_delete(
            &format!("/v1/orgs/{}/machines/{}", seg(org), seg(machine_id)),
            token,
        )
        .await
    }

    /// `GET /v1/orgs/{org}/devices`: admins see every member device,
    /// members their own.
    pub async fn org_devices(&self, token: &str, org: &str) -> Result<Vec<ApiDeviceApproval>> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            devices: Vec<ApiDeviceApproval>,
        }
        Ok(self
            .org_get::<R>(&format!("/v1/orgs/{}/devices", seg(org)), token)
            .await?
            .devices)
    }

    /// `POST /v1/orgs/{org}/devices/request`: asks the team's admins to
    /// approve the relay device `device_id` (this device).
    pub async fn request_org_device(
        &self,
        token: &str,
        org: &str,
        device_id: &str,
    ) -> Result<ApiDeviceApproval> {
        self.org_post(
            &format!("/v1/orgs/{}/devices/request", seg(org)),
            token,
            serde_json::json!({ "device_id": device_id }),
        )
        .await
    }

    /// `POST /v1/orgs/{org}/devices/{id}/approve|deny` (owner or admin):
    /// `sub` is the member whose device it is.
    pub async fn decide_org_device(
        &self,
        token: &str,
        org: &str,
        device_id: &str,
        sub: &str,
        approve: bool,
    ) -> Result<ApiDeviceApproval> {
        self.org_post(
            &format!(
                "/v1/orgs/{}/devices/{}/{}",
                seg(org),
                seg(device_id),
                if approve { "approve" } else { "deny" }
            ),
            token,
            serde_json::json!({ "sub": sub }),
        )
        .await
    }

    /// `GET /v1/pairing-codes/{id}`.
    pub async fn pairing(&self, token: &str, id: &str) -> Result<ApiPairingState> {
        self.get(&format!("/v1/pairing-codes/{}", seg(id)), token)
            .await
    }

    /// Runs the team operation `op`, and while it answers `402
    /// feature_required` (a plan bought a moment ago whose entitlements
    /// have not reached the API) or a passing `503`, refreshes the
    /// entitlements (`GET /v1/me/entitlements`) and retries with backoff
    /// for up to `window`. `waiting` is called before each retry so the
    /// caller can show [`FINISHING_SETUP`]. Any other answer, or the last
    /// one after `window`, is returned.
    pub async fn finishing_setup<T, F, Fut>(
        &self,
        token: &str,
        window: Duration,
        mut waiting: impl FnMut(Duration),
        mut op: F,
    ) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let start = std::time::Instant::now();
        let mut pause = Duration::from_millis(500);
        loop {
            let res = op().await;
            match &res {
                Err(e) if is_retryable(e) => {
                    let spent = start.elapsed();
                    if spent >= window {
                        return res;
                    }
                    waiting(spent);
                    let _ = self.entitlements(token).await;
                    tokio::time::sleep(pause.min(window.saturating_sub(start.elapsed()))).await;
                    pause = (pause * 2).min(Duration::from_secs(8));
                }
                _ => return res,
            }
        }
    }

    /// The account's Teams: `/v1/me`, then each org's members and pending
    /// invites. A plain member cannot list invites (403): they read as
    /// none. An org whose people cannot be read keeps its row without them.
    pub async fn teams(&self, token: &str) -> Result<TeamsSnapshot> {
        let me = self.me(token).await?;
        let mut teams = Vec::with_capacity(me.orgs.len());
        for org in me.orgs.iter().cloned() {
            let key = if org.alias.is_empty() {
                org.id.clone()
            } else {
                org.alias.clone()
            };
            let members = match self.members(token, &key).await {
                Ok(m) => m,
                Err(e @ Error::Unauthenticated(_)) => return Err(e),
                Err(e) => {
                    tracing::warn!("team {key}: members unavailable: {e}");
                    Vec::new()
                }
            };
            let invites = match self.invites(token, &key).await {
                Ok(i) => i,
                Err(e @ Error::Unauthenticated(_)) => return Err(e),
                Err(Error::PermissionDenied(_)) => Vec::new(),
                Err(e) => {
                    tracing::warn!("team {key}: invites unavailable: {e}");
                    Vec::new()
                }
            };
            teams.push(TeamDetail {
                org,
                members,
                invites,
            });
        }
        Ok(TeamsSnapshot {
            email: me.email,
            plan: me.entitlements.plan,
            teams,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::Path,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use serde_json::{Value, json};

    async fn serve(app: Router) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn authed(h: &HeaderMap) -> bool {
        h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer tok")
    }

    fn api() -> Router {
        Router::new()
            .route(
                "/v1/me",
                get(|h: HeaderMap| async move {
                    if !authed(&h) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"unauthorized"})));
                    }
                    (
                        StatusCode::OK,
                        Json(json!({
                            "sub":"u1","email":"ada@example.com",
                            "orgs":[
                                {"id":"o1","alias":"acme","name":"Acme Inc","role":"owner","plan":"teams","status":"active","seats":3},
                                {"id":"o2","alias":"side","role":"member","plan":"free","status":"none","seats":0}
                            ],
                            "entitlements":{"sub":"u1","plan":"pro","status":"active","warnings":[],"later":1}
                        })),
                    )
                }),
            )
            .route(
                "/v1/orgs/{org}/members",
                get(|Path(org): Path<String>| async move {
                    Json(json!({"members":[
                        {"sub":"u1","email":"ada@example.com","role":"owner"},
                        {"sub":format!("m-{org}"),"email":"bob@example.com","role":"member"}
                    ]}))
                }),
            )
            .route(
                "/v1/orgs/{org}/invites",
                get(|Path(org): Path<String>| async move {
                    if org == "side" {
                        return (StatusCode::FORBIDDEN, Json(json!({"error":"forbidden"})));
                    }
                    (StatusCode::OK, Json(json!({"invites":[{"id":"i1","email":"cy@example.com"}]})))
                }),
            )
            .route(
                "/v1/pairing-codes",
                post(|Json(b): Json<Value>| async move {
                    (
                        StatusCode::CREATED,
                        Json(json!({"id":"p1","code":"ABCD","expires_at":"2026-10-14T00:00:00Z","org":b["org"]})),
                    )
                }),
            )
    }

    #[tokio::test]
    async fn teams_read_orgs_members_and_pending_invites() {
        let c = SpacesApiClient::new(&serve(api()).await).unwrap();
        let t = c.teams("tok").await.unwrap();
        assert_eq!(
            (t.email.as_str(), t.plan.as_str()),
            ("ada@example.com", "pro")
        );
        assert_eq!(t.teams.len(), 2);
        let acme = &t.teams[0];
        assert_eq!(
            (
                acme.org.name.as_str(),
                acme.org.alias.as_str(),
                acme.org.seats
            ),
            ("Acme Inc", "acme", 3)
        );
        assert_eq!(acme.members.len(), 2);
        assert_eq!(
            acme.invites,
            vec![ApiInvite {
                id: "i1".into(),
                email: "cy@example.com".into()
            }]
        );
        // A plain member is refused the invite list: none, not an error.
        assert!(t.teams[1].invites.is_empty());
        assert_eq!(t.teams[1].org.name, "");
    }

    #[tokio::test]
    async fn a_refused_token_is_unauthenticated() {
        let c = SpacesApiClient::new(&serve(api()).await).unwrap();
        let e = c.teams("nope").await.unwrap_err();
        assert!(
            matches!(e, Error::Unauthenticated(ref m) if m.contains("HTTP 401")),
            "{e}"
        );
    }

    #[tokio::test]
    async fn pairing_codes_can_target_a_team() {
        let c = SpacesApiClient::new(&serve(api()).await).unwrap();
        let p = c.create_pairing("tok", Some("acme")).await.unwrap();
        assert_eq!((p.id.as_str(), p.code.as_str()), ("p1", "ABCD"));
    }

    #[test]
    fn only_https_or_loopback_bases() {
        assert!(SpacesApiClient::new("https://api.cua.ai/").is_ok());
        assert!(SpacesApiClient::new("http://api.cua.ai").is_err());
        assert!(SpacesApiClient::new("nonsense").is_err());
    }

    #[tokio::test]
    async fn org_machines_and_devices_round_trip() {
        let api = crate::testing::FakeSpacesApi::start().await;
        api.add_account("ada", "sub-ada", "ada@example.com");
        api.add_account("bob", "sub-bob", "bob@example.com");
        let org = api.add_org(
            "acme",
            "Acme",
            "sub-ada",
            &["team_machines", "device_approval"],
        );
        api.add_member(&org, "sub-bob", "member");
        let c = SpacesApiClient::new(&api.url).unwrap();

        // Teams resolve by id, alias or name; the old team- ids are refused.
        assert_eq!(c.resolve_org("ada", "Acme").await.unwrap().id, org);
        assert_eq!(
            c.resolve_org("ada", &format!("team:{org}"))
                .await
                .unwrap()
                .alias,
            "acme"
        );
        let e = c
            .resolve_org("ada", "team-0123456789abcdef")
            .await
            .unwrap_err();
        assert!(e.to_string().contains("old Teams"), "{e}");

        c.add_org_machine("ada", &org, "m1", "linux", "box")
            .await
            .unwrap();
        // Adding it again is not an error.
        c.add_org_machine("ada", &org, "m1", "linux", "box")
            .await
            .unwrap();
        assert_eq!(api.machines(&org), vec!["m1".to_string()]);
        // A plain member cannot.
        let e = c
            .add_org_machine("bob", &org, "m2", "", "")
            .await
            .unwrap_err();
        assert!(matches!(e, Error::PermissionDenied(_)), "{e}");
        assert_eq!(c.org_machines("bob", &org).await.unwrap().len(), 1);

        let a = c.request_org_device("bob", &org, "dev_b").await.unwrap();
        assert_eq!(a.state, "pending");
        let a = c
            .decide_org_device("ada", &org, "dev_b", "sub-bob", true)
            .await
            .unwrap();
        assert_eq!(a.state, "approved");
        assert_eq!(
            api.approvals(&org),
            vec![("dev_b".into(), "approved".into())]
        );

        c.remove_org_machine("ada", &org, "m1").await.unwrap();
        let e = c.remove_org_machine("ada", &org, "m1").await.unwrap_err();
        assert!(matches!(e, Error::NotFound(_)), "{e}");
    }

    #[tokio::test]
    async fn a_just_paid_plan_is_retried_until_it_arrives() {
        let api = crate::testing::FakeSpacesApi::start().await;
        api.add_account("ada", "sub-ada", "ada@example.com");
        let org = api.add_org("acme", "Acme", "sub-ada", &["team_machines"]);
        api.feature_lag(2);
        let c = SpacesApiClient::new(&api.url).unwrap();
        let mut waits = 0;
        c.finishing_setup(
            "ada",
            Duration::from_secs(10),
            |_| waits += 1,
            || c.add_org_machine("ada", &org, "m1", "linux", ""),
        )
        .await
        .unwrap();
        assert_eq!(waits, 2);
        assert_eq!(api.entitlement_reads(), 2);
        assert_eq!(api.machines(&org), vec!["m1".to_string()]);

        // A plan that never arrives surfaces the 402 after the window.
        let free = api.add_org("free", "Free", "sub-ada", &[]);
        let e = c
            .finishing_setup(
                "ada",
                Duration::from_millis(600),
                |_| {},
                || c.add_org_machine("ada", &free, "m1", "linux", ""),
            )
            .await
            .unwrap_err();
        assert!(is_feature_required(&e), "{e}");
    }

    #[tokio::test]
    async fn a_server_without_teams_says_so() {
        let api = crate::testing::FakeSpacesApi::start().await;
        api.add_account("ada", "sub-ada", "ada@example.com");
        let org = api.add_org("acme", "Acme", "sub-ada", &["team_machines"]);
        api.without_teams();
        let c = SpacesApiClient::new(&api.url).unwrap();
        let e = c
            .add_org_machine("ada", &org, "m1", "", "")
            .await
            .unwrap_err();
        assert!(is_teams_unavailable(&e), "{e}");
        let e = c.resolve_org("ada", "acme").await.unwrap_err();
        assert!(is_teams_unavailable(&e), "{e}");
        // Unreachable is the same answer.
        let c = SpacesApiClient::new("http://127.0.0.1:1").unwrap();
        let e = c.org_machines("ada", "acme").await.unwrap_err();
        assert!(is_teams_unavailable(&e), "{e}");
    }

    #[test]
    fn org_errors_name_the_owner_rule_and_unknown_routes() {
        let e = org_error(403, r#"{"error":"not_owner"}"#);
        assert_eq!(
            e.to_string(),
            "Only the machine's owner can add it to a team."
        );
        assert!(is_teams_unavailable(&org_error(404, "404 page not found")));
        assert!(!is_teams_unavailable(&org_error(
            404,
            r#"{"error":"org_not_found"}"#
        )));
        assert!(is_feature_required(&org_error(
            402,
            r#"{"error":"feature_required","detail":"team_machines"}"#
        )));
        assert!(!is_feature_required(&org_error(
            402,
            r#"{"error":"machine_limit"}"#
        )));
        assert!(is_org_ref("acme") && is_org_ref("0f1e2d3c-0000-4000-8000-000000000001"));
        assert!(!is_org_ref("team-0123456789abcdef") && !is_org_ref("Acme"));
    }

    #[test]
    fn errors_carry_the_api_detail() {
        let e = http_error(410, r#"{"error":"gone","detail":"retired"}"#);
        assert!(e.to_string().contains("HTTP 410 (gone: retired)"), "{e}");
    }
}
