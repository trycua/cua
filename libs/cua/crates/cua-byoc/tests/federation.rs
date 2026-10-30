// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! OIDC federation against a fake issuer and a fake STS and EC2 Query
//! endpoint: an OIDC connection gets its credentials from
//! `AssumeRoleWithWebIdentity` (unsigned, with the issuer's token, the
//! connection's role and a session name that says device, caller and
//! purpose), signs its EC2 calls with them, caches them until 2 minutes
//! before they expire, and says plainly when the issuer refuses; a CLI
//! connection never asks the issuer. The HTTP issuer client sends the
//! account's bearer and a device proof, and maps refusals to typed errors.
#![cfg(feature = "aws")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use base64::Engine as _;
use cua_byoc::aws::Aws;
use cua_byoc::federation::{
    HttpIssuer, IssuerAccount, IssuerClient, IssuerError, IssuerLimits, MintedToken, NewConnection,
    Purpose, ServerConnection, TokenRequest,
};
use cua_byoc::model::Auth;
use cua_byoc::{CloudApi, Connection};
use cua_sandbox_core::byoc::{CloudManager, ConnectRequest};

fn b64(v: &serde_json::Value) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
}

/// A JWT-shaped token with these claims (the fake STS does not verify it).
fn jwt(claims: serde_json::Value) -> String {
    format!(
        "{}.{}.sig",
        b64(&serde_json::json!({"alg": "RS256", "kid": "k1"})),
        b64(&claims)
    )
}

#[derive(Default)]
struct FakeIssuer {
    requests: Mutex<Vec<TokenRequest>>,
    refuse: Mutex<Option<IssuerError>>,
    /// The account's connections.
    conns: Mutex<Vec<ServerConnection>>,
    /// Every registry change: (what, agent).
    changes: Mutex<Vec<(String, Option<String>)>>,
}

#[async_trait]
impl IssuerClient for FakeIssuer {
    async fn token(&self, req: &TokenRequest) -> Result<MintedToken, IssuerError> {
        self.requests.lock().unwrap().push(req.clone());
        if let Some(e) = self.refuse.lock().unwrap().clone() {
            return Err(e);
        }
        let n = self.requests.lock().unwrap().len();
        Ok(MintedToken {
            token: jwt(serde_json::json!({
                "sub": "account:a1b2c3d4e5f6",
                "aud": req.audience,
                "cua_conn": req.connection,
                "cua_device": "dev_0123456789abcdef01234567",
                "cua_purpose": req.purpose.as_str(),
                "n": n,
            })),
            expires_at: cua_byoc::model::now() + 900,
        })
    }

    async fn account(&self) -> Result<IssuerAccount, IssuerError> {
        Ok(IssuerAccount {
            account: "a1b2c3d4e5f6".into(),
            issuer: "https://oidc.cua.ai/a/a1b2c3d4e5f6".into(),
            subject: "account:a1b2c3d4e5f6".into(),
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
            account: "a1b2c3d4e5f6".into(),
            cloud: new.cloud.clone(),
            auth: "oidc".into(),
            identifiers: new.identifiers.clone(),
            region: new.region.clone(),
            limits: new.limits.clone(),
            pending_limits: None,
            created_by: Some(cua_byoc::federation::ServerCreatedBy {
                agent: agent.map(String::from),
            }),
            pending_agent: None,
            status: if agent.is_some() {
                "pending"
            } else {
                "connected"
            }
            .into(),
            spent_month_usd: None,
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
        let raise = agent.is_some()
            && limits.budget_usd_month.unwrap_or(0.0) > c.limits.budget_usd_month.unwrap_or(0.0);
        if raise {
            c.pending_limits = Some(limits.clone());
            c.pending_agent = agent.map(String::from);
        } else {
            c.limits = limits.clone();
        }
        Ok((c.clone(), raise))
    }

    async fn approve(&self, id: &str, deny: bool) -> Result<ServerConnection, IssuerError> {
        self.changes
            .lock()
            .unwrap()
            .push((if deny { "deny" } else { "approve" }.into(), None));
        let mut conns = self.conns.lock().unwrap();
        let c = conns
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| IssuerError::NotFound(id.into()))?;
        if deny {
            if c.status == "pending" {
                c.status = "revoked".into();
            }
        } else {
            c.status = "connected".into();
            if let Some(p) = c.pending_limits.take() {
                c.limits = p;
            }
        }
        c.pending_limits = None;
        c.pending_agent = None;
        Ok(c.clone())
    }

    async fn revoke(&self, id: &str, agent: Option<&str>) -> Result<(), IssuerError> {
        self.changes
            .lock()
            .unwrap()
            .push(("revoke".into(), agent.map(String::from)));
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

#[derive(Default)]
struct Sts {
    /// Every call: (Action, parameters, the Authorization header).
    calls: Vec<(String, BTreeMap<String, String>, String)>,
    /// Seconds until the credentials STS hands out expire.
    lifetime: i64,
    next: u32,
    /// AWS does not trust the token (no stack yet).
    untrusted: bool,
}

type S = Arc<Mutex<Sts>>;

fn xml(body: String) -> axum::response::Response {
    (StatusCode::OK, [("content-type", "text/xml")], body).into_response()
}

async fn handle(State(s): State<S>, headers: HeaderMap, body: Bytes) -> axum::response::Response {
    let p: BTreeMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let action = p.get("Action").cloned().unwrap_or_default();
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut st = s.lock().unwrap();
    st.calls.push((action.clone(), p.clone(), auth.clone()));
    match action.as_str() {
        "AssumeRoleWithWebIdentity" if st.untrusted => (
            StatusCode::FORBIDDEN,
            [("content-type", "text/xml")],
            "<ErrorResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><Error><Type>Sender</Type><Code>InvalidIdentityToken</Code><Message>No OpenIDConnect provider found in your account for https://oidc.cua.ai/a/a1b2c3d4e5f6</Message></Error><RequestId>r</RequestId></ErrorResponse>",
        )
            .into_response(),
        _ if p.get("DryRun").map(String::as_str) == Some("true") => (
            StatusCode::PRECONDITION_FAILED,
            [("content-type", "text/xml")],
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Errors><Error><Code>DryRunOperation</Code><Message>would succeed</Message></Error></Errors><RequestID>r-1</RequestID></Response>",
        )
            .into_response(),
        "DescribeInstanceTypeOfferings" => xml(format!("<{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r</requestId><instanceTypeOfferingSet><item><instanceType>t4g.medium</instanceType><locationType>region</locationType><location>us-west-2</location></item></instanceTypeOfferingSet></{action}Response>")),
        "DescribeImages" => xml(format!("<{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r</requestId><imagesSet><item><imageId>ami-new</imageId><name>ubuntu-noble</name><creationDate>2026-09-01T00:00:00.000Z</creationDate><rootDeviceName>/dev/sda1</rootDeviceName></item></imagesSet></{action}Response>")),
        "DescribeVpcs" => xml(format!("<{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r</requestId><vpcSet><item><vpcId>vpc-default</vpcId><isDefault>true</isDefault></item></vpcSet></{action}Response>")),
        "AssumeRoleWithWebIdentity" => {
            st.next += 1;
            let exp = humantime::format_rfc3339_seconds(
                std::time::SystemTime::now()
                    + std::time::Duration::from_secs(st.lifetime.max(0) as u64),
            );
            xml(format!(
                "<AssumeRoleWithWebIdentityResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><AssumeRoleWithWebIdentityResult><Credentials><AccessKeyId>ASIAFAKE{n}</AccessKeyId><SecretAccessKey>secret</SecretAccessKey><SessionToken>session</SessionToken><Expiration>{exp}</Expiration></Credentials><SubjectFromWebIdentityToken>account:a1b2c3d4e5f6</SubjectFromWebIdentityToken><AssumedRoleUser><Arn>arn:aws:sts::123456789012:assumed-role/cua-spaces/{name}</Arn><AssumedRoleId>AROAFAKE:{name}</AssumedRoleId></AssumedRoleUser></AssumeRoleWithWebIdentityResult><ResponseMetadata><RequestId>r</RequestId></ResponseMetadata></AssumeRoleWithWebIdentityResponse>",
                n = st.next,
                name = p.get("RoleSessionName").cloned().unwrap_or_default(),
            ))
        }
        "GetCallerIdentity" => xml("<GetCallerIdentityResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><GetCallerIdentityResult><Arn>arn:aws:sts::123456789012:assumed-role/cua-spaces/x</Arn><UserId>X</UserId><Account>123456789012</Account></GetCallerIdentityResult><ResponseMetadata><RequestId>r</RequestId></ResponseMetadata></GetCallerIdentityResponse>".into()),
        "DescribeInstances" => xml(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r-1</requestId><reservationSet/></{action}Response>"
        )),
        "DescribeSecurityGroups" => xml(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><{action}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>r-1</requestId><securityGroupInfo/></{action}Response>"
        )),
        _ => (
            StatusCode::BAD_REQUEST,
            [("content-type", "text/xml")],
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Errors><Error><Code>Unsupported</Code><Message>not faked</Message></Error></Errors><RequestID>r-1</RequestID></Response>",
        )
            .into_response(),
    }
}

async fn serve(lifetime: i64) -> (String, S) {
    let s: S = Arc::new(Mutex::new(Sts {
        lifetime,
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/", axum::routing::post(handle))
        .with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, s)
}

fn oidc_conn() -> Connection {
    Connection {
        provider: "aws".into(),
        region: "us-west-2".into(),
        account: "123456789012".into(),
        auth: Auth::aws("conn_1", "arn:aws:iam::123456789012:role/cua-spaces"),
        ..Default::default()
    }
}

/// The access key id an AWS SigV4 Authorization header names.
fn key_of(auth: &str) -> &str {
    auth.split("Credential=")
        .nth(1)
        .and_then(|c| c.split('/').next())
        .unwrap_or_default()
}

#[tokio::test]
async fn an_oidc_connection_assumes_its_role_with_the_issuers_token() {
    let (url, sts) = serve(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let aws = Aws::with_endpoint(&url, "AKIASTATIC", "static-secret").with_issuer(issuer.clone());

    let tested = aws.test(&oidc_conn()).await;
    let creds = &tested.checks[0];
    assert_eq!(creds.name, "credentials");
    assert!(creds.ok, "{creds:?}");
    assert_eq!(tested.account, "123456789012");

    // The issuer was asked for a check token for this connection, for STS.
    let reqs = issuer.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].connection, "conn_1");
    assert_eq!(reqs[0].purpose, Purpose::Check);
    assert_eq!(reqs[0].audience, "sts.amazonaws.com");

    let calls = sts.lock().unwrap().calls.clone();
    let (action, p, auth) = &calls[0];
    assert_eq!(action, "AssumeRoleWithWebIdentity");
    // Unsigned: the token is the only credential.
    assert!(
        auth.is_empty(),
        "AssumeRoleWithWebIdentity was signed: {auth}"
    );
    assert_eq!(p["RoleArn"], "arn:aws:iam::123456789012:role/cua-spaces");
    // The caller: a person, or the agent whose shell runs these tests.
    let who = cua_byoc::federation::caller_agent().unwrap_or_else(|| "user".into());
    assert_eq!(
        p["RoleSessionName"],
        format!("cua-dev_0123456789abcdef01234567-{who}-check")
    );
    assert_eq!(p["DurationSeconds"], "900");
    let token = &p["WebIdentityToken"];
    assert_eq!(token.split('.').count(), 3);
    // Then the account's calls are signed with the assumed role's keys,
    // never the static ones.
    let (action, _, auth) = &calls[1];
    assert_eq!(action, "GetCallerIdentity");
    assert_eq!(key_of(auth), "ASIAFAKE1");
    assert!(
        calls.iter().all(|(_, _, a)| !a.contains("AKIASTATIC")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn credentials_are_cached_until_two_minutes_before_they_expire() {
    // An hour: one exchange serves many calls.
    let (url, sts) = serve(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let aws = Aws::with_endpoint(&url, "k", "s").with_issuer(issuer.clone());
    let conn = oidc_conn();
    for _ in 0..3 {
        aws.list_owned(&conn, "owner-1").await.unwrap();
    }
    assert_eq!(issuer.requests.lock().unwrap().len(), 1);
    assert_eq!(issuer.requests.lock().unwrap()[0].purpose, Purpose::Manage);
    let assumed = sts
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.0 == "AssumeRoleWithWebIdentity")
        .count();
    assert_eq!(assumed, 1);

    // 90 seconds: inside the 2-minute margin, so every call exchanges anew.
    let (url, sts) = serve(90).await;
    let issuer = Arc::new(FakeIssuer::default());
    let aws = Aws::with_endpoint(&url, "k", "s").with_issuer(issuer.clone());
    for _ in 0..2 {
        aws.list_owned(&conn, "owner-1").await.unwrap();
    }
    assert!(issuer.requests.lock().unwrap().len() >= 2);
    let keys: Vec<String> = sts
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.0 != "AssumeRoleWithWebIdentity")
        .map(|c| key_of(&c.2).to_string())
        .collect();
    assert!(keys.contains(&"ASIAFAKE2".to_string()), "{keys:?}");
}

#[tokio::test]
async fn an_issuer_refusal_says_what_to_do_and_reaches_no_cloud() {
    let (url, sts) = serve(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    *issuer.refuse.lock().unwrap() = Some(IssuerError::Pending);
    let aws = Aws::with_endpoint(&url, "k", "s").with_issuer(issuer.clone());
    let tested = aws.test(&oidc_conn()).await;
    assert!(!tested.ok());
    let c = &tested.checks[0];
    assert_eq!(c.name, "credentials");
    assert!(
        c.detail.contains("waits for a person to approve it"),
        "{c:?}"
    );
    // The CLI sign-in hint does not apply to an OIDC link.
    assert!(!c.detail.contains("aws sso login"), "{c:?}");

    *issuer.refuse.lock().unwrap() = Some(IssuerError::BudgetReached);
    let err = aws.list_owned(&oidc_conn(), "o").await.unwrap_err();
    assert!(err.to_string().contains("budget"), "{err}");

    // Nothing reached EC2 or STS.
    assert!(sts.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn a_cli_connection_never_asks_the_issuer() {
    let (url, sts) = serve(3600).await;
    let issuer = Arc::new(FakeIssuer::default());
    let aws = Aws::with_endpoint(&url, "AKIASTATIC", "s").with_issuer(issuer.clone());
    let conn = Connection {
        auth: Auth::Cli,
        ..oidc_conn()
    };
    aws.list_owned(&conn, "o").await.unwrap();
    assert!(issuer.requests.lock().unwrap().is_empty());
    let calls = sts.lock().unwrap().calls.clone();
    assert!(calls.iter().all(|c| c.0 != "AssumeRoleWithWebIdentity"));
    assert!(calls.iter().all(|c| key_of(&c.2) == "AKIASTATIC"));
}

/// What the fake HTTP issuer saw.
#[derive(Default)]
struct Seen {
    bearer: String,
    dpop: String,
    body: serde_json::Value,
}

#[tokio::test]
async fn the_http_issuer_sends_the_sign_in_and_a_device_proof() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let s2 = seen.clone();
    let app = axum::Router::new().route(
        "/v1/token",
        axum::routing::post(move |headers: HeaderMap, body: Bytes| {
            let seen = s2.clone();
            async move {
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let mut s = seen.lock().unwrap();
                s.bearer = headers["authorization"].to_str().unwrap().to_string();
                s.dpop = headers["dpop"].to_str().unwrap().to_string();
                s.body = body.clone();
                if body["connection"] == "conn_budget" {
                    return (
                        StatusCode::FORBIDDEN,
                        axum::Json(
                            serde_json::json!({"error": "budget_exhausted", "message": "used up"}),
                        ),
                    )
                        .into_response();
                }
                axum::Json(serde_json::json!({"token": "a.b.c", "expires_in": 900})).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (key, _) = cua_host::DeviceKey::generate().unwrap();
    let issuer = HttpIssuer::new(
        format!("{url}/"),
        Arc::new(cua_host::relay::StaticToken("kc-access".into())),
        Arc::new(key),
    );
    let req = TokenRequest {
        connection: "conn_1".into(),
        purpose: Purpose::Create,
        audience: "sts.amazonaws.com".into(),
        agent: Some("researcher".into()),
    };
    let t = issuer.token(&req).await.unwrap();
    assert_eq!(t.token, "a.b.c");
    let now = cua_byoc::model::now();
    assert!(t.expires_at > now + 800 && t.expires_at <= now + 900);
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.bearer, "Bearer kc-access");
        assert_eq!(
            s.body,
            serde_json::json!({
                "connection": "conn_1",
                "purpose": "create",
                "audience": "sts.amazonaws.com",
                "agent": "researcher",
            })
        );
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s.dpop.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["htu"], format!("{url}/v1/token"));
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["connection"], "conn_1");
        assert_eq!(claims["purpose"], "create");
        assert_eq!(claims["agent"], "researcher");
        // Bound to the access token sent with it.
        use sha2::Digest as _;
        assert_eq!(
            claims["ath"],
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(b"kc-access"))
        );
    }

    let refused = issuer
        .token(&TokenRequest {
            connection: "conn_budget".into(),
            ..req
        })
        .await
        .unwrap_err();
    assert_eq!(refused, IssuerError::BudgetReached);

    // An issuer that is down fails closed, plainly.
    let down = HttpIssuer::new(
        "http://127.0.0.1:9",
        Arc::new(cua_host::relay::StaticToken("t".into())),
        Arc::new(cua_host::DeviceKey::generate().unwrap().0),
    );
    let e = down
        .token(&TokenRequest {
            connection: "c".into(),
            purpose: Purpose::Check,
            audience: "sts.amazonaws.com".into(),
            agent: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, IssuerError::Unreachable(_)), "{e:?}");
    assert!(
        e.to_string().starts_with("Can't reach Cua's sign-in"),
        "{e}"
    );
}

// --- cloud_connect with auth: oidc ------------------------------------------

struct Linked {
    _dir: tempfile::TempDir,
    clouds: cua_byoc::Clouds,
    issuer: Arc<FakeIssuer>,
    sts: S,
    store: cua_byoc::Store,
    aws: Arc<dyn CloudApi>,
}

async fn linked() -> Linked {
    let (url, sts) = serve(3600).await;
    let dir = tempfile::tempdir().unwrap();
    let issuer = Arc::new(FakeIssuer::default());
    let aws: Arc<dyn CloudApi> =
        Arc::new(Aws::with_endpoint(&url, "AKIASTATIC", "s").with_issuer(issuer.clone()));
    let gcp: Arc<dyn CloudApi> = cua_byoc::testing::FakeProvider::new("gcp", cua_byoc::Tier::Vm);
    let hetzner: Arc<dyn CloudApi> =
        cua_byoc::testing::FakeProvider::new("hetzner", cua_byoc::Tier::Vm);
    let store = Arc::new(cua_byoc::Store::new(dir.path()));
    let clouds = cua_byoc::Clouds::new(dir.path(), vec![aws.clone(), gcp, hetzner], store)
        .with_issuer(issuer.clone());
    Linked {
        aws,
        store: cua_byoc::Store::new(dir.path()),
        _dir: dir,
        clouds,
        issuer,
        sts,
    }
}

fn ask(f: impl FnOnce(&mut ConnectRequest)) -> ConnectRequest {
    let mut r = ConnectRequest {
        target: cua_sandbox_core::byoc::CloudTarget {
            provider: "aws".into(),
            region: Some("us-west-2".into()),
            ..Default::default()
        },
        auth: Some("oidc".into()),
        ..Default::default()
    };
    f(&mut r);
    r
}

#[tokio::test]
async fn a_person_links_aws_step_by_step_and_each_run_says_what_is_next() {
    let w = linked().await;

    // No budget: the agent asks for one.
    let c = w.clouds.connect_with(&ask(|_| {})).await.unwrap();
    assert_eq!(c.provider.status, "failed");
    let next = c.next.clone().unwrap();
    assert!(
        next.run.as_deref().unwrap().contains("--budget <USD>"),
        "{next:?}"
    );
    assert!(next.open.is_none());
    assert!(w.issuer.conns.lock().unwrap().is_empty());

    // A budget, no account id yet: a person sets up the trust.
    let req = ask(|r| {
        r.budget_usd_month = Some(200.0);
        r.max_size = Some("4x16".into());
    });
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    let next = c.next.clone().unwrap();
    assert_eq!(
        next.open.as_deref(),
        Some("https://us-west-2.console.aws.amazon.com/cloudshell/home?region=us-west-2")
    );
    let paste = next.paste.unwrap();
    assert!(
        paste.contains("/v1/templates/aws.yaml?acct=a1b2c3d4e5f6"),
        "{paste}"
    );
    assert!(paste.contains("--stack-name cua-spaces"), "{paste}");
    assert!(
        next.why.contains(
            "cua cloud connect aws --oidc --region us-west-2 --budget 200 --max-size 4x16 \
         --account-id <12-digit AWS account id> --check"
        ),
        "{}",
        next.why
    );
    assert!(w.issuer.conns.lock().unwrap().is_empty());
    assert!(w.store.connection("aws").unwrap().is_none());

    // The account id: recorded on the issuer, then the check is next.
    let req = ask(|r| {
        r.budget_usd_month = Some(200.0);
        r.max_size = Some("4x16".into());
        r.account_id = Some("123456789012".into());
    });
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "checking");
    assert_eq!(
        c.next.as_ref().unwrap().run.as_deref(),
        Some(
            "cua cloud connect aws --oidc --region us-west-2 --budget 200 --max-size 4x16 \
             --account-id 123456789012 --check"
        )
    );
    {
        let conns = w.issuer.conns.lock().unwrap();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].status, "connected");
        assert_eq!(
            conns[0].identifiers["role_arn"],
            "arn:aws:iam::123456789012:role/cua-spaces"
        );
        assert_eq!(conns[0].limits.budget_usd_month, Some(200.0));
        assert_eq!(conns[0].limits.max_size.unwrap().memory_gib, 16);
    }
    assert!(w.store.connection("aws").unwrap().is_none());
    assert!(w.issuer.requests.lock().unwrap().is_empty());

    // The check: a check token, the exchange, a dry run. Then saved.
    let checked = ConnectRequest {
        check: true,
        ..req.clone()
    };
    let c = w.clouds.connect_with(&checked).await.unwrap();
    assert_eq!(c.provider.status, "connected", "{:?}", c.checks);
    assert!(c.next.is_none());
    assert_eq!(c.verify, "cua cloud status aws");
    assert!(c.provider.connected);
    assert_eq!(c.provider.auth, "oidc");
    assert_eq!(c.provider.budget_usd_month, Some(200.0));
    assert_eq!(c.provider.max_size, "4x16");
    assert!(c.checks.iter().all(|c| c.ok), "{:?}", c.checks);
    let reqs = w.issuer.requests.lock().unwrap().clone();
    assert!(
        reqs.iter()
            .all(|r| r.purpose == Purpose::Check && r.connection == "conn_1")
    );
    let saved = w.store.connection("aws").unwrap().unwrap();
    assert_eq!(
        saved.auth,
        Auth::aws("conn_1", "arn:aws:iam::123456789012:role/cua-spaces")
    );
    assert_eq!(saved.account, "123456789012");
    // Signed with the assumed role's keys only.
    assert!(
        w.sts
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|c| !c.2.contains("AKIASTATIC"))
    );

    // Idempotent: the same run again changes nothing on the issuer.
    let c = w.clouds.connect_with(&checked).await.unwrap();
    assert_eq!(c.provider.status, "connected");
    assert_eq!(w.issuer.conns.lock().unwrap().len(), 1);
    assert_eq!(w.issuer.changes.lock().unwrap().len(), 1);
    // A later run without the flags finds them on the issuer.
    let bare = ask(|r| r.check = true);
    let c = w.clouds.connect_with(&bare).await.unwrap();
    assert_eq!(c.provider.status, "connected");
    assert_eq!(c.provider.budget_usd_month, Some(200.0));

    // cloud_status reports it.
    use cua_sandbox_core::byoc::CloudManager as _;
    let s = w.clouds.status(Some("aws")).await.unwrap();
    let row = &s.providers[0];
    assert_eq!(
        (
            row.status.as_str(),
            row.auth.as_str(),
            row.approval.as_str()
        ),
        ("connected", "oidc", "")
    );
    assert_eq!(row.budget_usd_month, Some(200.0));
    assert_eq!(row.running, 0);

    // Disconnect stops the issuer minting for it, then forgets it here.
    let d = w.clouds.disconnect("aws", false).await.unwrap();
    assert!(d.disconnected);
    assert_eq!(w.issuer.conns.lock().unwrap()[0].status, "revoked");
    assert!(w.store.connection("aws").unwrap().is_none());
}

#[tokio::test]
async fn an_agents_link_waits_for_a_persons_approval() {
    let w = linked().await;
    let req = ask(|r| {
        r.budget_usd_month = Some(50.0);
        r.account_id = Some("123456789012".into());
        r.agent = Some("researcher".into());
        r.check = true;
    });
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "waiting-for-approval");
    assert_eq!(c.provider.approval, "connection");
    let next = c.next.unwrap();
    assert!(next.run.is_none() && next.open.is_none());
    assert!(next.why.contains("a person approves it"), "{}", next.why);
    assert_eq!(
        w.issuer.changes.lock().unwrap()[0],
        ("create".to_string(), Some("researcher".to_string()))
    );
    use cua_sandbox_core::byoc::CloudManager as _;
    let s = w.clouds.status(Some("aws")).await.unwrap();
    assert_eq!(s.providers[0].status, "waiting-for-approval");
    assert_eq!(s.providers[0].approval, "connection");

    // A person approves; the agent then raises the budget: that waits too.
    w.issuer.conns.lock().unwrap()[0].status = "connected".into();
    let raise = ConnectRequest {
        budget_usd_month: Some(500.0),
        ..req
    };
    let c = w.clouds.connect_with(&raise).await.unwrap();
    assert_eq!(c.provider.status, "waiting-for-approval");
    assert_eq!(c.provider.approval, "limits");
    // The budget in force is still the approved one.
    assert_eq!(c.provider.budget_usd_month, Some(50.0));
    assert_eq!(
        w.store.connection("aws").unwrap().unwrap().budget_usd_month,
        Some(50.0)
    );
}

#[tokio::test]
async fn an_account_without_the_trust_stack_needs_a_person() {
    let w = linked().await;
    w.sts.lock().unwrap().untrusted = true;
    let req = ask(|r| {
        r.budget_usd_month = Some(50.0);
        r.account_id = Some("123456789012".into());
        r.check = true;
    });
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    assert!(
        c.checks
            .iter()
            .any(|c| !c.ok && c.detail.contains("did not accept"))
    );
    let next = c.next.unwrap();
    assert!(next.open.unwrap().contains("cloudshell"));
    assert!(
        next.why.ends_with("--account-id 123456789012 --check"),
        "{}",
        next.why
    );
    assert!(w.store.connection("aws").unwrap().is_none());
}

#[tokio::test]
async fn other_clouds_and_sealed_tokens_say_what_works_today() {
    let w = linked().await;
    // Hetzner has no OIDC (Google Cloud and Azure do: see
    // tests/federation_gcp_azure.rs).
    let c = w
        .clouds
        .connect_with(&ask(|r| r.target.provider = "hetzner".into()))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "failed");
    // Without a budget and pool keys a token stays on this device (`--no-sealed`).
    assert_eq!(
        c.next.unwrap().run.as_deref(),
        Some("cua cloud connect hetzner --no-sealed --token-stdin")
    );
    let c = w
        .clouds
        .connect_with(&ask(|r| r.auth = Some("sealed".into())))
        .await
        .unwrap();
    // AWS has no token to seal: it says how AWS links instead.
    assert_eq!(c.provider.status, "failed");
    let next = c.next.unwrap();
    assert!(next.why.contains("--oidc"), "{}", next.why);
    assert!(next.run.unwrap().contains("--oidc"));
    // Bad input is an error, not a status.
    let e = w
        .clouds
        .connect_with(&ask(|r| r.max_size = Some("big".into())))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not <vCPUs>x<GB>"), "{e}");
    let e = w
        .clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(5.0);
            r.account_id = Some("12345".into());
        }))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("12 digits"), "{e}");
}

#[tokio::test]
async fn a_cli_link_keeps_working_and_takes_limits() {
    let w = linked().await;
    let req = ConnectRequest {
        target: cua_sandbox_core::byoc::CloudTarget {
            provider: "gcp".into(),
            region: Some("us-central1".into()),
            ..Default::default()
        },
        budget_usd_month: Some(20.0),
        max_size: Some("2x8".into()),
        ..Default::default()
    };
    let c = w.clouds.connect_with(&req).await.unwrap();
    assert_eq!(c.provider.status, "connected");
    assert_eq!(c.provider.auth, "cli");
    assert_eq!(c.provider.max_size, "2x8");
    let saved = w.store.connection("gcp").unwrap().unwrap();
    assert_eq!(saved.auth, Auth::Cli);
    assert_eq!(saved.budget_usd_month, Some(20.0));
    assert!(w.issuer.changes.lock().unwrap().is_empty());
}

// --- upgrades, re-runs and approval -----------------------------------------

/// A link made through the AWS CLI sign-in before federation existed.
fn cli_aws(account: &str) -> Connection {
    Connection {
        provider: "aws".into(),
        profile: "work".into(),
        region: "eu-west-1".into(),
        account: account.into(),
        ttl_hours: 4,
        connected_at: "2026-10-01T10:00:00Z".into(),
        budget_usd_month: Some(30.0),
        max_size: Some(cua_byoc::MaxSize {
            vcpu: 2,
            mem_gib: 8,
        }),
        ..Default::default()
    }
}

fn machine(id: &str) -> cua_byoc::Resource {
    let mut r = cua_byoc::Resource {
        provider: "aws".into(),
        id: id.into(),
        resource_type: "instance".into(),
        name: format!("cua-aws-{id}"),
        region: "eu-west-1".into(),
        sandbox: "aws:research".into(),
        created: cua_byoc::model::now() - 3600,
        state: "running".into(),
        ..Default::default()
    };
    r.extra.insert("usd_per_hour".into(), "0.5".into());
    r.extra.insert("started_by".into(), "researcher".into());
    r
}

/// Links AWS through the issuer and proves it (a person).
async fn link_oidc(w: &Linked, budget: f64) {
    let c = w
        .clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(budget);
            r.account_id = Some("123456789012".into());
            r.check = true;
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "connected", "{:?}", c.checks);
}

#[tokio::test]
async fn a_run_without_auth_keeps_an_oidc_link_and_only_changes_its_limits() {
    let w = linked().await;
    link_oidc(&w, 200.0).await;
    let sts_calls = w.sts.lock().unwrap().calls.len();
    // `cua cloud connect aws --budget 300` (no --oidc, no --check).
    let c = w
        .clouds
        .connect_with(&ConnectRequest {
            target: cua_sandbox_core::byoc::CloudTarget {
                provider: "aws".into(),
                ..Default::default()
            },
            budget_usd_month: Some(300.0),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(c.provider.status, "connected", "{c:?}");
    assert_eq!(c.provider.auth, "oidc");
    let saved = w.store.connection("aws").unwrap().unwrap();
    assert!(matches!(saved.auth, Auth::Oidc { .. }), "{saved:?}");
    assert!(saved.profile.is_empty(), "never a profile: {saved:?}");
    assert_eq!(saved.budget_usd_month, Some(300.0));
    assert_eq!(saved.region, "us-west-2");
    assert_eq!(
        w.issuer.conns.lock().unwrap()[0].limits.budget_usd_month,
        Some(300.0)
    );
    // Proven before: no new exchange, nothing asked of AWS.
    assert_eq!(w.sts.lock().unwrap().calls.len(), sts_calls);
}

#[tokio::test]
async fn switching_a_cli_link_to_oidc_carries_its_settings_and_keeps_it_until_proven() {
    let w = linked().await;
    w.store.connect(cli_aws("123456789012")).unwrap();
    // The suggestion, in one word on the row.
    let s = w.clouds.status(Some("aws")).await.unwrap();
    assert_eq!(s.providers[0].suggest_auth, "oidc");
    assert_eq!(s.providers[0].auth, "cli");

    // `cua cloud connect aws --oidc --account-id …`: no flags repeated.
    let c = w
        .clouds
        .connect_with(&ConnectRequest {
            target: cua_sandbox_core::byoc::CloudTarget {
                provider: "aws".into(),
                ..Default::default()
            },
            auth: Some("oidc".into()),
            account_id: Some("123456789012".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(c.provider.status, "checking", "{c:?}");
    {
        let conns = w.issuer.conns.lock().unwrap();
        assert_eq!(conns[0].region.as_deref(), Some("eu-west-1"));
        assert_eq!(conns[0].limits.budget_usd_month, Some(30.0));
        assert_eq!(conns[0].limits.ttl_hours, Some(4));
        assert_eq!(conns[0].limits.max_size.unwrap().vcpu, 2);
    }
    // Until the check passes, the CLI sign-in link is what runs.
    assert_eq!(
        w.store.connection("aws").unwrap().unwrap(),
        cli_aws("123456789012")
    );
    w.sts.lock().unwrap().untrusted = true;
    let checked = ConnectRequest {
        target: cua_sandbox_core::byoc::CloudTarget {
            provider: "aws".into(),
            ..Default::default()
        },
        auth: Some("oidc".into()),
        account_id: Some("123456789012".into()),
        check: true,
        ..Default::default()
    };
    let c = w.clouds.connect_with(&checked).await.unwrap();
    assert_eq!(c.provider.status, "needs-trust");
    assert_eq!(
        w.store.connection("aws").unwrap().unwrap(),
        cli_aws("123456789012")
    );

    // Proven: switched, with the same region, limits and lifetime.
    w.sts.lock().unwrap().untrusted = false;
    let c = w.clouds.connect_with(&checked).await.unwrap();
    assert_eq!(c.provider.status, "connected", "{:?}", c.checks);
    let saved = w.store.connection("aws").unwrap().unwrap();
    assert!(matches!(saved.auth, Auth::Oidc { .. }));
    assert_eq!(
        (
            saved.region.as_str(),
            saved.ttl_hours,
            saved.budget_usd_month
        ),
        ("eu-west-1", 4, Some(30.0))
    );
    assert!(saved.profile.is_empty());
    let s = w.clouds.status(Some("aws")).await.unwrap();
    assert_eq!(s.providers[0].suggest_auth, "");
}

#[tokio::test]
async fn switching_to_another_account_is_refused_while_machines_run_in_the_old_one() {
    let w = linked().await;
    w.store.connect(cli_aws("111111111111")).unwrap();
    w.store.record(machine("i-0old")).unwrap();
    let c = w
        .clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(50.0);
            r.account_id = Some("123456789012".into());
            r.check = true;
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "failed");
    let next = c.next.unwrap();
    assert!(
        next.why.contains("account 111111111111") && next.why.contains("1 machine"),
        "{}",
        next.why
    );
    assert_eq!(
        next.run.as_deref(),
        Some("cua cloud sweep aws --all --delete")
    );
    // Nothing changed: no issuer link, the CLI link in force.
    assert!(w.issuer.conns.lock().unwrap().is_empty());
    assert_eq!(
        w.store.connection("aws").unwrap().unwrap().account,
        "111111111111"
    );
    // The same account is fine (its machines stay reachable).
    let c = w
        .clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(50.0);
            r.account_id = Some("111111111111".into());
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "checking");
}

#[tokio::test]
async fn a_link_revoked_elsewhere_says_how_to_clean_up_what_runs() {
    let w = linked().await;
    link_oidc(&w, 100.0).await;
    w.store.record(machine("i-0abc")).unwrap();
    w.issuer.conns.lock().unwrap()[0].status = "revoked".into();
    let s = w.clouds.status(Some("aws")).await.unwrap();
    let row = &s.providers[0];
    assert_eq!(row.status, "failed");
    assert!(
        row.issue.contains("disconnected from your Cua account")
            && row.issue.contains("1 machine")
            && row.issue.contains("cua cloud connect aws --oidc")
            && row.issue.contains("cua cloud disconnect aws --delete"),
        "{}",
        row.issue
    );
    // The resource row says who started it and what it cost so far.
    let r = &s.resources[0];
    assert_eq!(r.started_by, "researcher");
    assert!((r.cost_usd - 0.5).abs() < 0.02, "{}", r.cost_usd);

    // Removed from the account altogether: the same.
    w.issuer.conns.lock().unwrap().clear();
    let s = w.clouds.status(Some("aws")).await.unwrap();
    assert!(s.providers[0].issue.contains("1 machine"));
}

#[tokio::test]
async fn a_person_approves_or_denies_what_an_agent_asked_and_an_agent_cannot() {
    let w = linked().await;
    link_oidc(&w, 50.0).await;
    // An agent raises the budget and the largest machine.
    let c = w
        .clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(500.0);
            r.max_size = Some("8x32".into());
            r.agent = Some("researcher".into());
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "waiting-for-approval");

    let person = cua_byoc::Clouds::new(
        w._dir.path(),
        vec![w.aws.clone()],
        Arc::new(cua_byoc::Store::new(w._dir.path())),
    )
    .with_issuer(w.issuer.clone())
    .with_caller(None);
    let s = person.status(Some("aws")).await.unwrap();
    let row = &s.providers[0];
    assert_eq!(row.approval, "limits");
    assert_eq!(row.approval_agent, "researcher");
    assert_eq!(row.pending_budget_usd_month, Some(500.0));
    assert_eq!(row.pending_max_size, "8x32");
    assert!(row.can_approve);

    // An agent can neither approve nor see the buttons.
    let agent = cua_byoc::Clouds::new(
        w._dir.path(),
        vec![w.aws.clone()],
        Arc::new(cua_byoc::Store::new(w._dir.path())),
    )
    .with_issuer(w.issuer.clone())
    .with_caller(Some("researcher"));
    assert!(!agent.status(Some("aws")).await.unwrap().providers[0].can_approve);
    let e = agent.approve("aws", false).await.unwrap_err();
    assert!(e.to_string().contains("only a person"), "{e}");
    assert!(
        !w.issuer
            .changes
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.0 == "approve")
    );

    // A person denies: the approved budget stays.
    let row = person.approve("aws", true).await.unwrap();
    assert_eq!(row.approval, "");
    assert_eq!(row.budget_usd_month, Some(50.0));
    // The agent asks again; the person approves: it is in force here too.
    w.clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(500.0);
            r.agent = Some("researcher".into());
        }))
        .await
        .unwrap();
    let row = person.approve("aws", false).await.unwrap();
    assert_eq!(
        (row.status.as_str(), row.approval.as_str()),
        ("connected", "")
    );
    assert_eq!(row.budget_usd_month, Some(500.0));
    assert_eq!(
        w.store.connection("aws").unwrap().unwrap().budget_usd_month,
        Some(500.0)
    );
    let audit = w.store.audit_entries(10).unwrap();
    assert!(audit.iter().any(|a| a.action == "deny"));
    assert!(audit.iter().any(|a| a.action == "approve"));
}

#[tokio::test]
async fn a_denied_agent_link_is_forgotten_here() {
    let w = linked().await;
    let c = w
        .clouds
        .connect_with(&ask(|r| {
            r.budget_usd_month = Some(50.0);
            r.account_id = Some("123456789012".into());
            r.agent = Some("researcher".into());
            r.check = true;
        }))
        .await
        .unwrap();
    assert_eq!(c.provider.status, "waiting-for-approval");
    let person = cua_byoc::Clouds::new(
        w._dir.path(),
        vec![w.aws.clone()],
        Arc::new(cua_byoc::Store::new(w._dir.path())),
    )
    .with_issuer(w.issuer.clone())
    .with_caller(None);
    let row = person.approve("aws", true).await.unwrap();
    assert!(!row.connected);
    assert_eq!(w.issuer.conns.lock().unwrap()[0].status, "revoked");
    assert!(w.store.connection("aws").unwrap().is_none());
}
