// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Google Cloud and Azure without a stored key, against a fake issuer, a
//! fake Google STS, IAM Credentials and Compute Engine, and a fake Entra
//! token endpoint and Azure Resource Manager (all hermetic):
//!
//! - an OIDC connection's token is minted for the right audience, exchanged
//!   (Google STS; Entra `client_credentials` with the token as the client
//!   assertion), used for every cloud call, and cached until 2 minutes
//!   before it expires; a refusal says plainly whether the trust is missing;
//!   an issuer refusal reaches no cloud; an audience that does not match
//!   the connection's identifiers is refused;
//! - `cloud_connect` walks budget, needs-trust (a page for a person, the
//!   exact command for an agent signed in to the cloud's CLI), checking,
//!   connected, and an agent's link waits for approval; reruns change
//!   nothing;
//! - Azure tags everything it creates, never lists or deletes what is not
//!   its own, and conforms to the elastic pool's contract.
#![cfg(all(feature = "gcp", feature = "azure"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use cua_byoc::azure::Azure;
use cua_byoc::federation::{
    IssuerAccount, IssuerClient, IssuerError, IssuerLimits, MintedToken, NewConnection, Purpose,
    ServerConnection, TokenRequest,
};
use cua_byoc::gcp::{Gcp, TokenSource};
use cua_byoc::model::{self, Auth, Connection, ProvisionSpec, Resource};
use cua_byoc::trust;
use cua_byoc::{CloudApi, Clouds, Store};
use cua_sandbox_core::byoc::{CloudManager, CloudTarget, ConnectRequest};
use serde_json::{Value, json};

const ACCT: &str = "a1b2c3d4e5f6";
const PROJECT: &str = "cua-test-1";
const NUMBER: &str = "123456789012";
const SUB: &str = "00000000-0000-0000-0000-000000000001";
const TENANT: &str = "11111111-1111-1111-1111-111111111111";
const CLIENT: &str = "22222222-2222-2222-2222-222222222222";

fn b64(v: &Value) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
}

fn gcp_provider() -> String {
    trust::gcp::provider_name(NUMBER, "cua", "cua")
}

// --- a fake issuer ------------------------------------------------------------

#[derive(Default)]
struct FakeIssuer {
    requests: Mutex<Vec<TokenRequest>>,
    refuse: Mutex<Option<IssuerError>>,
    conns: Mutex<Vec<ServerConnection>>,
    changes: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeIssuer {
    /// The audience the real issuer allows for a connection: the cloud's,
    /// and for Google Cloud the provider its identifiers name.
    fn allows(c: &ServerConnection, aud: &str) -> bool {
        match c.cloud.as_str() {
            "azure" => aud == "api://AzureADTokenExchange",
            "gcp" => match (
                c.identifiers.get("project_number"),
                c.identifiers.get("pool"),
                c.identifiers.get("provider"),
            ) {
                (Some(n), Some(p), Some(q)) => aud == trust::gcp::provider_name(n, p, q),
                _ => aud.starts_with("//iam.googleapis.com/projects/"),
            },
            _ => true,
        }
    }
}

#[async_trait]
impl IssuerClient for FakeIssuer {
    async fn token(&self, req: &TokenRequest) -> Result<MintedToken, IssuerError> {
        self.requests.lock().unwrap().push(req.clone());
        if let Some(e) = self.refuse.lock().unwrap().clone() {
            return Err(e);
        }
        if let Some(c) = self
            .conns
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == req.connection)
            && !Self::allows(c, &req.audience)
        {
            return Err(IssuerError::Other(format!(
                "audience_not_allowed: {} is not this connection's audience",
                req.audience
            )));
        }
        let n = self.requests.lock().unwrap().len();
        Ok(MintedToken {
            token: format!(
                "{}.{}.sig",
                b64(&json!({"alg": "RS256", "kid": "k1"})),
                b64(&json!({
                    "iss": format!("https://oidc.cua.ai/a/{ACCT}"),
                    "sub": format!("account:{ACCT}"),
                    "aud": req.audience,
                    "cua_conn": req.connection,
                    "cua_device": "dev_0123456789abcdef01234567",
                    "cua_purpose": req.purpose.as_str(),
                    "n": n,
                }))
            ),
            expires_at: model::now() + 900,
        })
    }

    async fn account(&self) -> Result<IssuerAccount, IssuerError> {
        Ok(IssuerAccount {
            account: ACCT.into(),
            issuer: format!("https://oidc.cua.ai/a/{ACCT}"),
            subject: format!("account:{ACCT}"),
        })
    }

    async fn connections(&self) -> Result<Vec<ServerConnection>, IssuerError> {
        Ok(self.conns.lock().unwrap().clone())
    }

    async fn create_connection(
        &self,
        new: &NewConnection,
        agent: Option<&str>,
    ) -> Result<ServerConnection, IssuerError> {
        self.changes
            .lock()
            .unwrap()
            .push(("create".into(), agent.map(String::from)));
        let mut conns = self.conns.lock().unwrap();
        let c = ServerConnection {
            id: format!("conn_{}", conns.len() + 1),
            account: ACCT.into(),
            cloud: new.cloud.clone(),
            auth: "oidc".into(),
            identifiers: new.identifiers.clone(),
            region: new.region.clone(),
            limits: new.limits.clone(),
            pending_limits: None,
            status: if agent.is_some() {
                "pending"
            } else {
                "connected"
            }
            .into(),
            ..Default::default()
        };
        conns.push(c.clone());
        Ok(c)
    }

    async fn set_limits(
        &self,
        id: &str,
        limits: &IssuerLimits,
        agent: Option<&str>,
    ) -> Result<(ServerConnection, bool), IssuerError> {
        self.changes
            .lock()
            .unwrap()
            .push(("limits".into(), agent.map(String::from)));
        let mut conns = self.conns.lock().unwrap();
        let c = conns.iter_mut().find(|c| c.id == id).unwrap();
        c.limits = limits.clone();
        Ok((c.clone(), false))
    }

    async fn revoke(&self, id: &str, _agent: Option<&str>) -> Result<(), IssuerError> {
        let mut conns = self.conns.lock().unwrap();
        match conns.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                c.status = "revoked".into();
                Ok(())
            }
            None => Err(IssuerError::NotFound(id.into())),
        }
    }
}

/// A gcloud that must never be asked (OIDC links do not use it).
struct NoGcloud;

#[async_trait]
impl TokenSource for NoGcloud {
    async fn token(&self, _: Option<&str>) -> cua_byoc::Result<String> {
        panic!("an OIDC link asked gcloud for a token");
    }
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn bearer(h: &HeaderMap) -> String {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

// --- a fake Google ------------------------------------------------------------

#[derive(Default)]
struct Google {
    /// STS token exchanges (the form).
    sts: Vec<BTreeMap<String, String>>,
    /// IAM Credentials calls: (service account, bearer).
    impersonations: Vec<(String, String)>,
    /// Compute and Resource Manager calls: (method path, bearer).
    calls: Vec<(String, String)>,
    /// Seconds the STS token lives.
    lifetime: u64,
    /// The pool does not trust the token.
    untrusted: bool,
}

type G = Arc<Mutex<Google>>;

async fn google(
    State(g): State<G>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let mut st = g.lock().unwrap();
    let ok = |v: Value| (StatusCode::OK, axum::Json(v)).into_response();
    if path == "/sts/v1/token" {
        let form: BTreeMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        st.sts.push(form);
        if st.untrusted {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "invalid_grant", "error_description": "The audience in ID Token does not match the expected audience."})),
            )
                .into_response();
        }
        let n = st.sts.len();
        return ok(
            json!({"access_token": format!("ya29.fed-{n}"), "expires_in": st.lifetime, "token_type": "Bearer"}),
        );
    }
    if let Some(rest) = path.strip_prefix("/iam/v1/projects/-/serviceAccounts/") {
        let sa = rest.trim_end_matches(":generateAccessToken").to_string();
        st.impersonations.push((sa, bearer(&headers)));
        let exp = humantime::format_rfc3339_seconds(
            std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
        );
        return ok(json!({"accessToken": "ya29.sa-1", "expireTime": exp.to_string()}));
    }
    st.calls
        .push((format!("{method} {path}"), bearer(&headers)));
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), parts.as_slice()) {
        ("POST", ["crm", "v1", "projects", p]) if p.ends_with(":testIamPermissions") => {
            ok(json!({"permissions": body["permissions"]}))
        }
        ("GET", ["compute", "v1", "projects", PROJECT]) => ok(json!({"name": PROJECT})),
        ("GET", ["compute", "v1", "projects", "ubuntu-os-cloud", ..]) => {
            ok(json!({"name": "ubuntu-2404-noble"}))
        }
        (
            "GET",
            [
                "compute",
                "v1",
                "projects",
                PROJECT,
                "zones",
                _,
                "machineTypes",
                _,
            ],
        ) => ok(json!({"name": "e2"})),
        ("GET", ["compute", "v1", "projects", PROJECT, "regions", _]) => ok(json!({"quotas": [
            {"metric": "CPUS", "limit": 24.0, "usage": 0.0},
            {"metric": "IN_USE_ADDRESSES", "limit": 8.0, "usage": 0.0}]})),
        (
            "GET",
            [
                "compute",
                "v1",
                "projects",
                PROJECT,
                "zones",
                _,
                "instances",
            ],
        ) => ok(json!({})),
        _ => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": {"message": "not found"}})),
        )
            .into_response(),
    }
}

struct GoogleFake {
    url: String,
    g: G,
}

async fn fake_google(lifetime: u64) -> GoogleFake {
    let g: G = Arc::new(Mutex::new(Google {
        lifetime,
        ..Default::default()
    }));
    let app = axum::Router::new().fallback(google).with_state(g.clone());
    GoogleFake {
        url: serve(app).await,
        g,
    }
}

impl GoogleFake {
    fn gcp(&self, issuer: Arc<FakeIssuer>, gcloud: Option<std::path::PathBuf>) -> Gcp {
        Gcp::with_endpoints(
            &format!("{}/compute/v1", self.url),
            &format!("{}/crm/v1", self.url),
            Arc::new(NoGcloud),
            gcloud,
        )
        .with_federation(
            issuer,
            &format!("{}/sts/v1/token", self.url),
            &format!("{}/iam/v1", self.url),
        )
    }
}

fn gcp_conn(sa: &str) -> Connection {
    Connection {
        provider: "gcp".into(),
        region: "us-central1".into(),
        zone: "us-central1-a".into(),
        project: PROJECT.into(),
        account: NUMBER.into(),
        auth: Auth::gcp("conn_g", gcp_provider(), sa),
        ..Default::default()
    }
}

#[tokio::test]
async fn gcp_exchanges_the_issuers_token_at_google_sts_and_uses_it_directly() {
    let f = fake_google(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let gcp = f.gcp(issuer.clone(), None);
    let t = gcp.test(&gcp_conn("")).await;
    assert!(t.ok(), "{:?}", t.checks);
    assert!(
        t.checks[0].detail.contains(&gcp_provider()),
        "{:?}",
        t.checks[0]
    );

    // One check token, for the provider's full name.
    let reqs = issuer.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].purpose, Purpose::Check);
    assert_eq!(reqs[0].connection, "conn_g");
    assert_eq!(reqs[0].audience, gcp_provider());

    let g = f.g.lock().unwrap();
    assert_eq!(g.sts.len(), 1);
    let form = &g.sts[0];
    assert_eq!(
        form["grant_type"],
        "urn:ietf:params:oauth:grant-type:token-exchange"
    );
    assert_eq!(form["audience"], gcp_provider());
    assert_eq!(
        form["scope"],
        "https://www.googleapis.com/auth/cloud-platform"
    );
    assert_eq!(
        form["requested_token_type"],
        "urn:ietf:params:oauth:token-type:access_token"
    );
    assert_eq!(
        form["subject_token_type"],
        "urn:ietf:params:oauth:token-type:jwt"
    );
    assert_eq!(form["subject_token"].split('.').count(), 3);
    // No service account: the federated token is used as is, everywhere.
    assert!(g.impersonations.is_empty());
    assert!(!g.calls.is_empty());
    assert!(
        g.calls.iter().all(|(_, b)| b == "Bearer ya29.fed-1"),
        "{:?}",
        g.calls
    );
}

#[tokio::test]
async fn gcp_tokens_are_cached_until_two_minutes_before_they_expire() {
    let f = fake_google(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let gcp = f.gcp(issuer.clone(), None);
    for _ in 0..3 {
        gcp.list_owned(&gcp_conn(""), "owner-1").await.unwrap();
    }
    assert_eq!(issuer.requests.lock().unwrap().len(), 1);
    assert_eq!(issuer.requests.lock().unwrap()[0].purpose, Purpose::Manage);
    assert_eq!(f.g.lock().unwrap().sts.len(), 1);

    // 90 seconds: inside the margin, so every call exchanges anew.
    let f = fake_google(90).await;
    let issuer = Arc::new(FakeIssuer::default());
    let gcp = f.gcp(issuer.clone(), None);
    for _ in 0..2 {
        gcp.list_owned(&gcp_conn(""), "owner-1").await.unwrap();
    }
    assert!(f.g.lock().unwrap().sts.len() >= 2);
    let bearers: Vec<String> =
        f.g.lock()
            .unwrap()
            .calls
            .iter()
            .map(|c| c.1.clone())
            .collect();
    assert!(
        bearers.contains(&"Bearer ya29.fed-2".to_string()),
        "{bearers:?}"
    );
}

#[tokio::test]
async fn gcp_acts_as_a_service_account_only_when_the_link_names_one() {
    let f = fake_google(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let gcp = f.gcp(issuer, None);
    let sa = "cua@cua-test-1.iam.gserviceaccount.com";
    gcp.list_owned(&gcp_conn(sa), "owner-1").await.unwrap();
    let g = f.g.lock().unwrap();
    assert_eq!(
        g.impersonations,
        vec![(sa.to_string(), "Bearer ya29.fed-1".to_string())]
    );
    assert!(
        g.calls.iter().all(|(_, b)| b == "Bearer ya29.sa-1"),
        "{:?}",
        g.calls
    );
}

#[tokio::test]
async fn gcp_refusals_say_whether_the_trust_is_missing_and_reach_no_cloud() {
    let f = fake_google(3600).await;
    f.g.lock().unwrap().untrusted = true;
    let issuer = Arc::new(FakeIssuer::default());
    let gcp = f.gcp(issuer.clone(), None);
    let t = gcp.test(&gcp_conn("")).await;
    assert!(!t.ok());
    let c = &t.checks[0];
    assert_eq!(c.name, "credentials");
    assert!(c.detail.contains("did not accept Cua's token"), "{c:?}");
    assert!(c.detail.contains("invalid_grant"), "{c:?}");
    assert!(f.g.lock().unwrap().calls.is_empty());

    // The issuer refuses: no exchange at all.
    let f = fake_google(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    *issuer.refuse.lock().unwrap() = Some(IssuerError::Pending);
    let gcp = f.gcp(issuer, None);
    let t = gcp.test(&gcp_conn("")).await;
    assert!(
        t.checks[0].detail.contains("waits for a person"),
        "{:?}",
        t.checks
    );
    let e = gcp.list_owned(&gcp_conn(""), "o").await.unwrap_err();
    assert!(e.to_string().contains("approve"), "{e}");
    let g = f.g.lock().unwrap();
    assert!(g.sts.is_empty() && g.calls.is_empty());
}

#[tokio::test]
async fn a_provider_that_is_not_the_connections_is_refused_by_the_issuer() {
    let f = fake_google(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    issuer.conns.lock().unwrap().push(ServerConnection {
        id: "conn_g".into(),
        cloud: "gcp".into(),
        status: "connected".into(),
        identifiers: [
            ("project_number".to_string(), NUMBER.to_string()),
            ("pool".to_string(), "cua".to_string()),
            ("provider".to_string(), "cua".to_string()),
        ]
        .into(),
        ..Default::default()
    });
    let gcp = f.gcp(issuer, None);
    let mut conn = gcp_conn("");
    conn.auth = Auth::gcp("conn_g", trust::gcp::provider_name("999", "cua", "cua"), "");
    let t = gcp.test(&conn).await;
    assert!(!t.ok());
    assert!(
        t.checks[0].detail.contains("audience_not_allowed"),
        "{:?}",
        t.checks
    );
    assert!(f.g.lock().unwrap().sts.is_empty());
    // The connection's own provider works.
    assert!(gcp.test(&gcp_conn("")).await.ok());
}

// --- a fake Azure ---------------------------------------------------------------

#[derive(Default)]
struct Arm {
    /// Entra token requests (the form) and the tenant they went to.
    entra: Vec<(String, BTreeMap<String, String>)>,
    /// ARM calls: (method path, bearer).
    calls: Vec<(String, String)>,
    /// Resources by lowercase ARM path.
    resources: BTreeMap<String, Value>,
    /// Every PUT: (path, body).
    puts: Vec<(String, Value)>,
    /// Every validate body.
    validated: Vec<Value>,
    lifetime: u64,
    untrusted: bool,
    ops: u32,
}

type Z = Arc<Mutex<Arm>>;

const RG: &str = "/subscriptions/00000000-0000-0000-0000-000000000001/resourcegroups/cua-spaces";

async fn azure(
    State(z): State<Z>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    // Every subscription has the group (an agent may link another one).
    let lower = match path.to_ascii_lowercase().strip_prefix("/subscriptions/") {
        Some(after) => match after.split_once('/') {
            Some((_, rest)) => format!("/subscriptions/{SUB}/{rest}"),
            None => format!("/subscriptions/{SUB}"),
        },
        None => path.to_ascii_lowercase(),
    };
    let query = uri.query().unwrap_or_default().to_string();
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let base = format!("http://{host}");
    let mut st = z.lock().unwrap();
    let ok = |v: Value| (StatusCode::OK, axum::Json(v)).into_response();
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": {"code": "ResourceNotFound", "message": "not found"}})),
        )
            .into_response()
    };
    if let Some(rest) = path.strip_prefix("/login/") {
        let tenant = rest.split('/').next().unwrap_or_default().to_string();
        let form: BTreeMap<String, String> =
            url::form_urlencoded::parse(&body).into_owned().collect();
        st.entra.push((tenant, form));
        if st.untrusted {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"error": "invalid_client", "error_description": "AADSTS70021: No matching federated identity record found for presented assertion.\r\nTrace ID: x"})),
            )
                .into_response();
        }
        let n = st.entra.len();
        return ok(
            json!({"access_token": format!("arm-{n}"), "expires_in": st.lifetime, "token_type": "Bearer"}),
        );
    }
    if path.starts_with("/ops/") {
        return if query.contains("done") {
            ok(json!({}))
        } else {
            ok(json!({"status": "Succeeded"}))
        };
    }
    if let Some(name) = path.strip_prefix("/blob/") {
        return (StatusCode::OK, format!("boot log of {name}")).into_response();
    }
    st.calls
        .push((format!("{method} {path}"), bearer(&headers)));
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let op = |st: &mut Arm, location: bool| {
        st.ops += 1;
        let url = format!("{base}/ops/{}", st.ops);
        if location {
            (
                StatusCode::ACCEPTED,
                [("location", format!("{url}?done"))],
                axum::Json(json!({})),
            )
                .into_response()
        } else {
            (
                StatusCode::CREATED,
                [("azure-asyncoperation", url)],
                axum::Json(json!({})),
            )
                .into_response()
        }
    };
    if lower == RG {
        return ok(json!({"name": "cua-spaces", "location": "westus2"}));
    }
    if !lower.starts_with(RG) {
        return not_found();
    }
    let sub = &lower[RG.len()..];
    if sub == "/providers/microsoft.authorization/permissions" {
        return ok(
            json!({"value": [{"actions": ["Microsoft.Compute/*", "Microsoft.Network/*", "Microsoft.Resources/deployments/*"], "notActions": []}]}),
        );
    }
    if sub.ends_with("/validate") {
        st.validated.push(body);
        return ok(json!({"properties": {"provisioningState": "Succeeded"}}));
    }
    if sub == "/resources" {
        // Not filtered here: the provider must check the tags itself.
        let rows: Vec<Value> = st
            .resources
            .values()
            .map(|r| json!({"name": r["name"], "type": r["type"], "tags": r["tags"], "location": r["location"]}))
            .collect();
        return ok(json!({"value": rows}));
    }
    for action in ["/deallocate", "/start"] {
        if let Some(vm) = sub.strip_suffix(action) {
            let key = format!("{RG}{vm}");
            let Some(r) = st.resources.get_mut(&key) else {
                return not_found();
            };
            r["power"] = json!(if action == "/start" {
                "running"
            } else {
                "deallocated"
            });
            return op(&mut st, false);
        }
    }
    if let Some(vm) = sub.strip_suffix("/retrievebootdiagnosticsdata") {
        let name = vm.rsplit('/').next().unwrap_or_default().to_string();
        return ok(json!({"serialConsoleLogBlobUri": format!("{base}/blob/{name}")}));
    }
    let key = lower.clone();
    match method.as_str() {
        "PUT" => {
            st.puts.push((path.clone(), body.clone()));
            let segs: Vec<&str> = path.split('/').collect();
            let n = segs.len();
            let mut r = body.clone();
            r["name"] = json!(segs[n - 1]);
            r["type"] = json!(format!("{}/{}", segs[n - 3], segs[n - 2]));
            r["power"] = json!("running");
            st.resources.insert(key, r);
            op(&mut st, false)
        }
        "GET" => match st.resources.get(&key) {
            Some(r) => {
                let mut r = r.clone();
                r["properties"]["instanceView"] = json!({"statuses": [
                    {"code": "ProvisioningState/succeeded"},
                    {"code": format!("PowerState/{}", r["power"].as_str().unwrap_or("running"))}]});
                ok(r)
            }
            None => not_found(),
        },
        "DELETE" => {
            if st.resources.remove(&key).is_none() {
                return (StatusCode::NO_CONTENT, "").into_response();
            }
            // A VM takes its interface along (deleteOption).
            if key.contains("/virtualmachines/") {
                let nic = key.replace(
                    "microsoft.compute/virtualmachines",
                    "microsoft.network/networkinterfaces",
                ) + "-nic";
                st.resources.remove(&nic);
            }
            op(&mut st, true)
        }
        _ => not_found(),
    }
}

struct AzureFake {
    url: String,
    z: Z,
}

async fn fake_azure(lifetime: u64) -> AzureFake {
    let z: Z = Arc::new(Mutex::new(Arm {
        lifetime,
        ..Default::default()
    }));
    let app = axum::Router::new().fallback(azure).with_state(z.clone());
    AzureFake {
        url: serve(app).await,
        z,
    }
}

impl AzureFake {
    fn azure(&self, issuer: Arc<FakeIssuer>, az: Option<std::path::PathBuf>) -> Azure {
        Azure::with_endpoints(&self.url, &format!("{}/login", self.url), Some(issuer), az)
    }

    /// A resource someone else made in the group (no Cua tags, or another
    /// owner's).
    fn foreign(&self, kind: &str, name: &str, tags: Value) {
        let path = format!(
            "{RG}/providers/{}/{name}",
            if kind == "vm" {
                "microsoft.compute/virtualmachines"
            } else {
                "microsoft.network/publicipaddresses"
            }
        );
        let ty = if kind == "vm" {
            "Microsoft.Compute/virtualMachines"
        } else {
            "Microsoft.Network/publicIPAddresses"
        };
        self.z.lock().unwrap().resources.insert(
            path.to_ascii_lowercase(),
            json!({"name": name, "type": ty, "tags": tags, "location": "westus2", "power": "running"}),
        );
    }
}

fn azure_conn() -> Connection {
    Connection {
        provider: "azure".into(),
        region: "westus2".into(),
        account: SUB.into(),
        project: "cua-spaces".into(),
        auth: Auth::azure("conn_z", TENANT, CLIENT),
        ..Default::default()
    }
}

fn spec(owner: &str, name: &str) -> ProvisionSpec {
    ProvisionSpec {
        name: name.into(),
        family: "linux".into(),
        kind: "container".into(),
        machine_type: "Standard_B2s".into(),
        disk_gb: 30,
        user_data: "#cloud-config\nruncmd: [echo hi]\n".into(),
        arch: "amd64".into(),
        tags: model::tags(owner, name, "cloud-1", model::now() + 3600),
        ttl_secs: 3600,
        ..Default::default()
    }
}

#[tokio::test]
async fn azure_signs_in_with_the_issuers_token_as_a_client_assertion() {
    let f = fake_azure(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let az = f.azure(issuer.clone(), None);
    let t = az.test(&azure_conn()).await;
    assert!(t.ok(), "{:?}", t.checks);
    let names: Vec<&str> = t.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["credentials", "resource group", "permissions", "validate"]
    );
    assert_eq!(t.account, SUB);

    let reqs = issuer.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].purpose, Purpose::Check);
    assert_eq!(reqs[0].audience, "api://AzureADTokenExchange");

    let z = f.z.lock().unwrap();
    assert_eq!(z.entra.len(), 1);
    let (tenant, form) = &z.entra[0];
    assert_eq!(tenant, TENANT);
    assert_eq!(form["grant_type"], "client_credentials");
    assert_eq!(form["client_id"], CLIENT);
    assert_eq!(
        form["client_assertion_type"],
        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer"
    );
    assert_eq!(form["client_assertion"].split('.').count(), 3);
    assert_eq!(form["scope"], "https://management.azure.com/.default");
    assert!(
        z.calls.iter().all(|(_, b)| b == "Bearer arm-1"),
        "{:?}",
        z.calls
    );
    // The check creates nothing: a validate, no PUT.
    assert!(z.puts.is_empty());
    assert_eq!(z.validated.len(), 1);
    let resources = z.validated[0]["properties"]["template"]["resources"]
        .as_array()
        .unwrap();
    assert!(
        resources
            .iter()
            .any(|r| r["type"] == "Microsoft.Compute/virtualMachines")
    );
}

#[tokio::test]
async fn azure_tokens_are_cached_and_refusals_say_why() {
    let f = fake_azure(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let az = f.azure(issuer.clone(), None);
    for _ in 0..3 {
        az.list_owned(&azure_conn(), "owner-1").await.unwrap();
    }
    assert_eq!(issuer.requests.lock().unwrap().len(), 1);
    assert_eq!(f.z.lock().unwrap().entra.len(), 1);

    let f = fake_azure(90).await;
    let az = f.azure(Arc::new(FakeIssuer::default()), None);
    for _ in 0..2 {
        az.list_owned(&azure_conn(), "owner-1").await.unwrap();
    }
    assert!(f.z.lock().unwrap().entra.len() >= 2);

    // Entra does not trust the token: said plainly, nothing else called.
    let f = fake_azure(3600).await;
    f.z.lock().unwrap().untrusted = true;
    let az = f.azure(Arc::new(FakeIssuer::default()), None);
    let t = az.test(&azure_conn()).await;
    assert!(!t.ok());
    assert!(
        t.checks[0].detail.contains("did not accept Cua's token"),
        "{:?}",
        t.checks
    );
    assert!(t.checks[0].detail.contains("AADSTS70021"), "{:?}", t.checks);
    assert!(f.z.lock().unwrap().calls.is_empty());

    // The issuer refuses: Entra is never asked.
    let f = fake_azure(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    *issuer.refuse.lock().unwrap() = Some(IssuerError::BudgetReached);
    let az = f.azure(issuer, None);
    let e = az.list_owned(&azure_conn(), "o").await.unwrap_err();
    assert!(e.to_string().contains("budget"), "{e}");
    assert!(f.z.lock().unwrap().entra.is_empty());

    // A CLI link is not a way to reach Azure.
    let cli = Connection {
        auth: Auth::Cli,
        ..azure_conn()
    };
    let e = f
        .azure(Arc::new(FakeIssuer::default()), None)
        .list_owned(&cli, "o")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("Cua's issuer only"), "{e}");
}

#[tokio::test]
async fn azure_tags_everything_it_creates_and_opens_nothing_inbound() {
    let f = fake_azure(3600).await;
    let az = f.azure(Arc::new(FakeIssuer::default()), None);
    let made = az
        .provision(&azure_conn(), &spec("owner-1", "cua-space-1"))
        .await
        .unwrap();
    let kinds: Vec<&str> = made.iter().map(|r| r.resource_type.as_str()).collect();
    assert_eq!(kinds, ["instance", "network", "security_group"]);
    assert_eq!(made[0].extra["nic"], "cua-space-1-nic");
    {
        let z = f.z.lock().unwrap();
        // NSG, VNet, public IP, NIC, VM: every create carries Cua's tags.
        assert_eq!(
            z.puts.len(),
            5,
            "{:?}",
            z.puts.iter().map(|p| &p.0).collect::<Vec<_>>()
        );
        for (path, body) in &z.puts {
            assert_eq!(body["tags"]["cua-managed"], "true", "{path}");
            assert_eq!(body["tags"]["cua-owner"], "owner-1", "{path}");
            assert!(path.contains("/resourceGroups/cua-spaces/"), "{path}");
        }
        let of = |t: &str| {
            z.puts
                .iter()
                .find(|(p, _)| p.contains(t))
                .map(|(_, b)| b.clone())
                .unwrap()
        };
        assert_eq!(
            of("networkSecurityGroups")["properties"]["securityRules"],
            json!([])
        );
        let vm = of("virtualMachines");
        let os = &vm["properties"]["osProfile"];
        assert!(os["linuxConfiguration"]["ssh"].is_null(), "no SSH key");
        let data = base64::engine::general_purpose::STANDARD
            .decode(os["customData"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            String::from_utf8(data).unwrap(),
            "#cloud-config\nruncmd: [echo hi]\n"
        );
        assert!(vm["identity"].is_null(), "no managed identity on the VM");
        assert_eq!(
            vm["properties"]["networkProfile"]["networkInterfaces"][0]["properties"]["deleteOption"],
            "Delete"
        );
        assert_eq!(
            vm["properties"]["storageProfile"]["osDisk"]["deleteOption"],
            "Delete"
        );
    }
    // The shared network is reused, not made again.
    az.provision(&azure_conn(), &spec("owner-1", "cua-space-2"))
        .await
        .unwrap();
    assert_eq!(f.z.lock().unwrap().puts.len(), 8);
    // Boot log through the signed link.
    let log = az.boot_log(&azure_conn(), &made[0]).await.unwrap();
    assert_eq!(log.as_deref(), Some("boot log of cua-space-1"));
}

#[tokio::test]
async fn azure_never_lists_or_touches_what_is_not_its_own() {
    let f = fake_azure(3600).await;
    let az = f.azure(Arc::new(FakeIssuer::default()), None);
    let conn = azure_conn();
    let made = az
        .provision(&conn, &spec("owner-1", "cua-space-1"))
        .await
        .unwrap();
    f.foreign("vm", "their-vm", json!({"env": "prod"}));
    f.foreign(
        "vm",
        "other-owner",
        json!({"cua-managed": "true", "cua-owner": "owner-2"}),
    );
    f.foreign("ip", "their-ip", json!({}));

    let mine = az.list_owned(&conn, "owner-1").await.unwrap();
    let names: Vec<&str> = mine.iter().map(|r| r.id.as_str()).collect();
    assert!(names.contains(&"cua-space-1"), "{names:?}");
    for n in ["their-vm", "other-owner", "their-ip"] {
        assert!(!names.contains(&n), "{n} listed: {names:?}");
    }
    assert!(mine.iter().all(|r| model::is_ours(&r.tags, "owner-1")));
    assert_eq!(mine[0].resource_type, "instance");
    assert_eq!(mine[0].state, "running");

    // Delete, stop and start refuse what is not tagged as ours.
    for r in [
        Resource {
            id: "their-vm".into(),
            resource_type: "instance".into(),
            ..Default::default()
        },
        Resource {
            id: "other-owner".into(),
            resource_type: "instance".into(),
            ..Default::default()
        },
    ] {
        let e = az.delete(&conn, &r, "owner-1").await.unwrap_err();
        assert!(
            e.to_string().contains("not tagged as this Cua owner's"),
            "{e}"
        );
        assert!(az.stop(&conn, &r, "owner-1").await.is_err());
    }
    assert_eq!(
        f.z.lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(c, _)| c.starts_with("DELETE") || c.contains("/deallocate"))
            .count(),
        0
    );
    assert_eq!(f.z.lock().unwrap().resources.len(), 8);

    // Ours: stop deallocates, start starts, delete takes the VM, its
    // interface and its address.
    az.stop(&conn, &made[0], "owner-1").await.unwrap();
    let (state, _) = az.describe(&conn, &made[0]).await.unwrap().unwrap();
    assert_eq!(state, "stopped");
    az.start(&conn, &made[0], "owner-1").await.unwrap();
    az.delete(&conn, &made[0], "owner-1").await.unwrap();
    let left: Vec<String> = f.z.lock().unwrap().resources.keys().cloned().collect();
    assert!(left.iter().all(|k| !k.contains("cua-space-1")), "{left:?}");
    // Again: fine.
    az.delete(&conn, &made[0], "owner-1").await.unwrap();
}

#[tokio::test]
async fn azure_conforms_to_the_pool_contract() {
    let f = fake_azure(3600).await;
    let az = f.azure(Arc::new(FakeIssuer::default()), None);
    let conn = azure_conn();
    let bad = cua_byoc::conformance::static_checks(&az, &conn).await;
    assert!(bad.is_empty(), "{bad:#?}");
    let z = f.z.clone();
    let seen = move || {
        z.lock()
            .unwrap()
            .puts
            .iter()
            .filter(|(p, _)| p.contains("/virtualMachines/"))
            .map(|(_, b)| {
                let p = &b["properties"];
                cua_byoc::conformance::CreateCall {
                    machine_type: p["hardwareProfile"]["vmSize"].as_str().unwrap().into(),
                    region: b["location"].as_str().unwrap().into(),
                    user_data: String::from_utf8(
                        base64::engine::general_purpose::STANDARD
                            .decode(p["osProfile"]["customData"].as_str().unwrap())
                            .unwrap(),
                    )
                    .unwrap(),
                    tags: serde_json::from_value(b["tags"].clone()).unwrap(),
                    ssh_key: !p["osProfile"]["linuxConfiguration"]["ssh"].is_null(),
                }
            })
            .collect()
    };
    let bad = cua_byoc::conformance::lifecycle_checks(&az, &conn, "owner-1", &seen).await;
    assert!(bad.is_empty(), "{bad:#?}");
}

// --- cloud_connect ------------------------------------------------------------------

struct Linked {
    _dir: tempfile::TempDir,
    clouds: Clouds,
    issuer: Arc<FakeIssuer>,
    store: Store,
    google: GoogleFake,
    arm: AzureFake,
}

/// `cli`: the gcloud and az CLIs are signed in on this machine.
async fn linked(cli: bool) -> Linked {
    let dir = tempfile::tempdir().unwrap();
    let issuer = Arc::new(FakeIssuer::default());
    let google = fake_google(3600).await;
    let arm = fake_azure(3600).await;
    let (gcloud, az) = if cli {
        let g = dir.path().join("gcloud");
        std::fs::create_dir_all(g.join("configurations")).unwrap();
        std::fs::write(g.join("active_config"), "default").unwrap();
        std::fs::write(
            g.join("configurations/config_default"),
            "[core]\naccount = ada@example.com\nproject = cua-test-1\n",
        )
        .unwrap();
        let a = dir.path().join("azure");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(
            a.join("azureProfile.json"),
            format!("{{\"subscriptions\": [{{\"id\": \"{SUB}\", \"name\": \"Dev\", \"isDefault\": true}}]}}"),
        )
        .unwrap();
        (Some(g), Some(a))
    } else {
        (None, None)
    };
    let apis: Vec<Arc<dyn CloudApi>> = vec![
        Arc::new(google.gcp(issuer.clone(), gcloud)),
        Arc::new(arm.azure(issuer.clone(), az)),
    ];
    let store = Arc::new(Store::new(dir.path()));
    let clouds = Clouds::new(dir.path(), apis, store).with_issuer(issuer.clone());
    Linked {
        store: Store::new(dir.path()),
        _dir: dir,
        clouds,
        issuer,
        google,
        arm,
    }
}

fn ask(provider: &str, f: impl FnOnce(&mut ConnectRequest)) -> ConnectRequest {
    let mut r = ConnectRequest {
        target: CloudTarget {
            provider: provider.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    if provider == "gcp" {
        r.auth = Some("oidc".into());
        r.target.project = Some(PROJECT.into());
        r.target.region = Some("us-central1".into());
    } else {
        r.target.region = Some("westus2".into());
    }
    f(&mut r);
    r
}

#[tokio::test]
async fn a_person_links_google_cloud_step_by_step() {
    let w = linked(false).await;

    // No budget: ask for one.
    let c = w.clouds.connect_with(&ask("gcp", |_| {})).await.unwrap();
    assert_eq!(c.provider.status, "failed");
    assert!(c.next.unwrap().run.unwrap().contains("--budget <USD>"));

    // A budget, no project number: a person runs the trust script.
    let c = w
        .clouds
        .connect_with(&ask("gcp", |r| {
            r.budget_usd_month = Some(200.0);
            r.max_size = Some("4x16".into());
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    let next = c.next.unwrap();
    assert_eq!(
        next.open.as_deref(),
        Some(trust::gcp::cloud_shell_url(trust::REPO, trust::REF).as_str())
    );
    let paste = next.paste.unwrap();
    assert!(
        paste.starts_with(&format!(
            "bash cua-trust.sh --project {PROJECT} --account {ACCT} --issuer-host "
        )),
        "{paste}"
    );
    assert!(next.run.is_none());
    assert!(
        next.why.ends_with(
            "cua cloud connect gcp --oidc --region us-central1 --budget 200 --max-size 4x16 \
             --project cua-test-1 --project-number <project number> --check"
        ),
        "{}",
        next.why
    );
    assert!(w.issuer.conns.lock().unwrap().is_empty());

    // The number: recorded on the issuer with the pool and provider.
    let req = ask("gcp", |r| {
        r.budget_usd_month = Some(200.0);
        r.max_size = Some("4x16".into());
        r.project_number = Some(NUMBER.into());
    });
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "checking");
    assert_eq!(
        c.next.unwrap().run.as_deref(),
        Some(
            "cua cloud connect gcp --oidc --region us-central1 --budget 200 --max-size 4x16 \
             --project cua-test-1 --project-number 123456789012 --check"
        )
    );
    {
        let conns = w.issuer.conns.lock().unwrap();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].cloud, "gcp");
        let ids: Vec<(&str, &str)> = conns[0]
            .identifiers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("pool", "cua"),
                ("project_number", NUMBER),
                ("provider", "cua")
            ]
        );
    }
    assert!(w.issuer.requests.lock().unwrap().is_empty());

    // The check: saved once Google accepts the token.
    let checked = ConnectRequest {
        check: true,
        ..req.clone()
    };
    let c = w.clouds.connect_with(&checked).await.unwrap();
    assert_eq!(c.provider.status, "connected", "{:?}", c.checks);
    assert_eq!(c.verify, "cua cloud status gcp");
    let saved = w.store.connection("gcp").unwrap().unwrap();
    assert_eq!(saved.auth, Auth::gcp("conn_1", gcp_provider(), ""));
    assert_eq!(
        (
            saved.project.as_str(),
            saved.zone.as_str(),
            saved.account.as_str()
        ),
        (PROJECT, "us-central1-a", NUMBER)
    );
    assert!(
        w.issuer
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.purpose == Purpose::Check && r.audience == gcp_provider())
    );
    assert_eq!(w.google.g.lock().unwrap().sts.len(), 1);

    // Idempotent; a later run without the flags finds them.
    let c = w.clouds.connect_with(&checked).await.unwrap();
    assert_eq!(c.provider.status, "connected");
    let c = w
        .clouds
        .connect_with(&ask("gcp", |r| r.check = true))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "connected");
    assert_eq!(c.provider.budget_usd_month, Some(200.0));
    assert_eq!(w.issuer.conns.lock().unwrap().len(), 1);
    assert_eq!(w.issuer.changes.lock().unwrap().len(), 1);

    let s = w.clouds.status(Some("gcp")).await.unwrap();
    assert_eq!(
        (s.providers[0].status.as_str(), s.providers[0].auth.as_str()),
        ("connected", "oidc")
    );
    let d = w.clouds.disconnect("gcp", false).await.unwrap();
    assert!(d.disconnected);
    assert_eq!(w.issuer.conns.lock().unwrap()[0].status, "revoked");
}

#[tokio::test]
async fn google_cloud_without_the_pool_needs_a_person_again() {
    let w = linked(false).await;
    w.google.g.lock().unwrap().untrusted = true;
    let c = w
        .clouds
        .connect_with(&ask("gcp", |r| {
            r.budget_usd_month = Some(50.0);
            r.project_number = Some(NUMBER.into());
            r.check = true;
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    assert!(
        c.checks
            .iter()
            .any(|c| !c.ok && c.detail.contains("did not accept"))
    );
    let next = c.next.unwrap();
    assert!(
        next.open
            .unwrap()
            .starts_with("https://shell.cloud.google.com/")
    );
    assert!(
        next.why.ends_with("--project-number 123456789012 --check"),
        "{}",
        next.why
    );
    assert!(w.store.connection("gcp").unwrap().is_none());
}

#[tokio::test]
async fn an_agent_signed_in_to_the_clouds_cli_gets_the_exact_command() {
    let w = linked(true).await;
    let c = w
        .clouds
        .connect_with(&ask("gcp", |r| {
            r.budget_usd_month = Some(50.0);
            r.agent = Some("researcher".into());
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    let next = c.next.unwrap();
    assert!(next.open.is_none());
    assert_eq!(
        next.run.unwrap(),
        format!(
            "cua cloud trust gcp | bash -s -- --project {PROJECT} --account {ACCT} --issuer-host {}",
            trust::issuer_host(&cua_byoc::federation::issuer_url())
        )
    );

    let c = w
        .clouds
        .connect_with(&ask("azure", |r| {
            r.budget_usd_month = Some(50.0);
            r.agent = Some("researcher".into());
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    let run = c.next.unwrap().run.unwrap();
    assert!(run.starts_with("cua cloud trust azure > cua-spaces.json && az group create --name cua-spaces --location westus2"), "{run}");
    assert!(
        run.contains(&format!("--parameters cuaAccount={ACCT} ")),
        "{run}"
    );

    // A person (no agent) still gets the page, CLI or not.
    let c = w
        .clouds
        .connect_with(&ask("azure", |r| r.budget_usd_month = Some(50.0)))
        .await
        .unwrap();
    assert!(c.next.unwrap().open.is_some());
}

#[tokio::test]
async fn a_person_links_azure_and_an_agents_link_waits_for_approval() {
    let w = linked(false).await;
    // Azure needs no --oidc: it has no other way.
    let c = w
        .clouds
        .connect_with(&ask("azure", |r| {
            r.budget_usd_month = Some(100.0);
            r.max_size = Some("2x8".into());
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    let next = c.next.unwrap();
    assert_eq!(
        next.open.as_deref(),
        Some(trust::azure::deploy_url(&trust::azure::template_url()).as_str())
    );
    assert!(
        next.why.contains(&format!("Cua Account {ACCT}")),
        "{}",
        next.why
    );
    assert!(
        next.why.ends_with(
            "cua cloud connect azure --region westus2 --budget 100 --max-size 2x8 \
             --subscription <subscription id> --tenant <tenant id> --client-id <client id> \
             --resource-group cua-spaces --check"
        ),
        "{}",
        next.why
    );

    // Bad ids are errors, not statuses.
    let e = w
        .clouds
        .connect_with(&ask("azure", |r| {
            r.budget_usd_month = Some(100.0);
            r.tenant_id = Some("not-a-guid".into());
        }))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("Microsoft Entra tenant id"), "{e}");

    let req = ask("azure", |r| {
        r.budget_usd_month = Some(100.0);
        r.subscription_id = Some(SUB.into());
        r.tenant_id = Some(TENANT.into());
        r.client_id = Some(CLIENT.into());
        r.check = true;
    });
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "connected", "{:?}", c.checks);
    {
        let conns = w.issuer.conns.lock().unwrap();
        let ids: Vec<(&str, &str)> = conns[0]
            .identifiers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("client_id", CLIENT),
                ("resource_group", "cua-spaces"),
                ("subscription_id", SUB),
                ("tenant_id", TENANT)
            ]
        );
    }
    let saved = w.store.connection("azure").unwrap().unwrap();
    assert_eq!(saved.auth, Auth::azure("conn_1", TENANT, CLIENT));
    assert_eq!(
        (saved.account.as_str(), saved.project.as_str()),
        (SUB, "cua-spaces")
    );
    assert!(
        w.arm.z.lock().unwrap().puts.is_empty(),
        "the check created something"
    );

    // Rerun: nothing changes on the issuer.
    w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(w.issuer.changes.lock().unwrap().len(), 1);

    // An agent links another subscription: it waits for a person.
    let other = "00000000-0000-0000-0000-000000000002";
    let c = w
        .clouds
        .connect_with(&ConnectRequest {
            subscription_id: Some(other.into()),
            agent: Some("researcher".into()),
            ..req.clone()
        })
        .await
        .unwrap();
    assert_eq!(c.provider.status, "waiting-for-approval");
    assert_eq!(c.provider.approval, "connection");
    assert!(c.next.unwrap().why.contains("a person approves it"));
    assert_eq!(
        w.issuer.changes.lock().unwrap()[1],
        ("create".to_string(), Some("researcher".to_string()))
    );
    assert_eq!(w.issuer.conns.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn azure_without_the_identity_needs_a_person_again() {
    let w = linked(false).await;
    w.arm.z.lock().unwrap().untrusted = true;
    let c = w
        .clouds
        .connect_with(&ask("azure", |r| {
            r.budget_usd_month = Some(100.0);
            r.subscription_id = Some(SUB.into());
            r.tenant_id = Some(TENANT.into());
            r.client_id = Some(CLIENT.into());
            r.check = true;
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    assert!(
        c.next
            .unwrap()
            .open
            .unwrap()
            .starts_with("https://portal.azure.com/")
    );
    assert!(w.store.connection("azure").unwrap().is_none());
}
