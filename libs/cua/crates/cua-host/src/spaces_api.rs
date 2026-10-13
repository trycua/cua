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

/// A Spaces API client.
#[derive(Clone)]
pub struct SpacesApiClient {
    base: String,
    http: reqwest::Client,
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
        403 => Error::PermissionDenied(msg),
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
        let resp = req
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Spaces API {}: {e}", self.base)))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &body));
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

    /// `GET /v1/me`: the account, its plan and its orgs.
    pub async fn me(&self, token: &str) -> Result<ApiMe> {
        self.get("/v1/me", token).await
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
            .get::<R>(&format!("/v1/orgs/{org}/members"), token)
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
            .get::<R>(&format!("/v1/orgs/{org}/invites"), token)
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

    #[test]
    fn errors_carry_the_api_detail() {
        let e = http_error(410, r#"{"error":"gone","detail":"retired"}"#);
        assert!(e.to_string().contains("HTTP 410 (gone: retired)"), "{e}");
    }
}
