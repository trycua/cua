// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The issuer end to end over HTTP: discovery and JWKS, who may get a
//! token (access token + device proof of an enrolled device of the same
//! account), the issuance policy, the token's shape as the clouds need it,
//! connections and approval, and the audit log.

mod common;

use base64::Engine as _;
use common::{aws_connection, fresh_jti, Env};
use cua_oidc::proof::testing::DeviceKey;
use cua_oidc::signer::Signer as _;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::{json, Value};

const STS: &str = "sts.amazonaws.com";

/// Verifies a minted token against the published JWKS with a standard JWT
/// library, the way a cloud does.
async fn verify_with_jwks(env: &Env, token: &str, issuer: &str, aud: &str) -> Value {
    let header = jsonwebtoken::decode_header(token).unwrap();
    let kid = header.kid.expect("kid");
    // The issuer's own JWKS, as discovery names it.
    let path = format!("{}/.well-known/jwks.json", &issuer[env.base.len()..]);
    let jwks: jsonwebtoken::jwk::JwkSet =
        serde_json::from_value(env.get(&path, None).await.body).unwrap();
    let jwk = jwks.find(&kid).expect("kid in the JWKS");
    let mut v = Validation::new(Algorithm::RS256);
    v.set_issuer(&[issuer]);
    v.set_audience(&[aud]);
    v.set_required_spec_claims(&["exp", "iss", "aud", "sub", "nbf", "iat"]);
    jsonwebtoken::decode::<Value>(token, &DecodingKey::from_jwk(jwk).unwrap(), &v)
        .unwrap()
        .claims
}

#[tokio::test]
async fn discovery_document_shape() {
    let env = Env::start().await;
    let acct = env.acct("ada");
    let r = env
        .get(&format!("/a/{acct}/.well-known/openid-configuration"), None)
        .await;
    assert_eq!(r.status, 200);
    let iss = format!("{}/a/{acct}", env.base);
    assert_eq!(r.body["issuer"], iss);
    assert_eq!(r.body["jwks_uri"], format!("{iss}/.well-known/jwks.json"));
    assert_eq!(r.body["response_types_supported"], json!(["id_token"]));
    assert_eq!(r.body["subject_types_supported"], json!(["public"]));
    assert_eq!(
        r.body["id_token_signing_alg_values_supported"],
        json!(["RS256"])
    );
    let claims: Vec<&str> = r.body["claims_supported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    for c in [
        "iss",
        "sub",
        "aud",
        "exp",
        "iat",
        "cua_conn",
        "cua_device",
        "cua_agent",
        "cua_purpose",
    ] {
        assert!(claims.contains(&c), "{c}");
    }
    // Not an account id: no document (and no path tricks).
    for bad in ["ABC", "abcdefgh2341", "..%2F..%2Fx"] {
        let r = env
            .get(&format!("/a/{bad}/.well-known/openid-configuration"), None)
            .await;
        assert_eq!(r.status, 404, "{bad}");
    }
}

#[tokio::test]
async fn jwks_publishes_active_next_and_previous_keys() {
    let env = Env::start().await;
    let acct = env.acct("ada");
    let root = env.get("/.well-known/jwks.json", None).await;
    let per_account = env
        .get(&format!("/a/{acct}/.well-known/jwks.json"), None)
        .await;
    assert_eq!(root.status, 200);
    assert_eq!(root.body, per_account.body);
    let keys = root.body["keys"].as_array().unwrap();
    let kids: Vec<&str> = keys.iter().map(|k| k["kid"].as_str().unwrap()).collect();
    let expect: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|n| common::signer(n).public_key().thumbprint())
        .collect();
    assert_eq!(kids, expect.iter().map(String::as_str).collect::<Vec<_>>());
    for k in keys {
        assert_eq!(k["kty"], "RSA");
        assert_eq!(k["alg"], "RS256");
        assert_eq!(k["use"], "sig");
        assert!(k.get("d").is_none(), "no private parts");
    }
}

#[tokio::test]
async fn happy_path_token_verifies_with_the_jwks_and_suits_aws_sts() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let conn = env.connect(&ada, &device, aws_connection()).await;
    assert_eq!(conn["status"], "connected");
    let id = conn["id"].as_str().unwrap();

    let r = env
        .mint(&ada, &device, id, "create", STS, Some("nightly-build"))
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let acct = env.acct("ada");
    let iss = format!("{}/a/{acct}", env.base);
    assert_eq!(r.body["issuer"], iss);
    assert_eq!(r.body["subject"], format!("account:{acct}"));
    assert_eq!(r.body["expires_in"], 900);
    let token = r.body["token"].as_str().unwrap();

    let claims = verify_with_jwks(&env, token, &iss, STS).await;
    assert_eq!(claims["sub"], format!("account:{acct}"));
    assert_eq!(claims["aud"], STS);
    assert_eq!(claims["cua_conn"], id);
    assert_eq!(claims["cua_device"], device.id());
    assert_eq!(claims["cua_agent"], "nightly-build");
    assert_eq!(claims["cua_purpose"], "create");
    assert_eq!(claims["jti"], r.body["jti"]);

    // What AWS STS AssumeRoleWithWebIdentity needs: RS256, a kid it can
    // find, an issuer without a trailing slash that matches the provider
    // URL exactly, a string aud, and a 15-minute lifetime.
    let header = jsonwebtoken::decode_header(token).unwrap();
    assert_eq!(header.alg, Algorithm::RS256);
    assert_eq!(
        header.kid.as_deref(),
        Some(common::signer("a").public_key().thumbprint().as_str())
    );
    let iss_claim = claims["iss"].as_str().unwrap();
    assert!(!iss_claim.ends_with('/'));
    assert!(claims["aud"].is_string());
    let (iat, nbf, exp) = (
        claims["iat"].as_u64().unwrap(),
        claims["nbf"].as_u64().unwrap(),
        claims["exp"].as_u64().unwrap(),
    );
    assert_eq!(exp - iat, 900);
    assert_eq!(nbf, iat);

    // A person's token carries no agent claim.
    let r = env.mint(&ada, &device, id, "manage", STS, None).await;
    let claims = verify_with_jwks(&env, r.body["token"].as_str().unwrap(), &iss, STS).await;
    assert!(claims.get("cua_agent").is_none());

    // The issuance is audited with its jti.
    let audit = env.get("/v1/audit", Some(&ada)).await;
    let events = audit.body["events"].as_array().unwrap();
    let issued: Vec<&Value> = events
        .iter()
        .filter(|e| e["kind"] == "token_issued")
        .collect();
    assert_eq!(issued.len(), 2);
    assert_eq!(issued[0]["jti"], r.body["jti"]);
    assert_eq!(issued[0]["device"], device.id());
}

#[tokio::test]
async fn forged_or_wrong_access_tokens_mint_nothing() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let id = env.connect(&ada, &device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Signed by another key, wrong audience, expired.
    let forger = common::FakeKeycloak::new(common::KEYCLOAK);
    for token in [
        forger.token("ada"),
        env.keycloak.token_for("ada", "cua-relay", 300),
        env.keycloak.token_for("ada", "cua-oidc", -120),
        "not-a-jwt".to_owned(),
    ] {
        let r = env.mint(&token, &device, &id, "create", STS, None).await;
        assert_eq!(r.status, 401, "{}", r.body);
        assert_eq!(r.code(), "invalid_token");
    }
    // No access token at all.
    let r = env
        .http
        .post(format!("{}/v1/token", env.base))
        .json(&json!({"connection": id, "purpose": "create", "audience": STS}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // An access token without a device proof.
    let r = env
        .send(
            "POST",
            "/v1/token",
            &ada,
            None,
            Some(json!({"connection": id, "purpose": "create", "audience": STS})),
        )
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.code(), "invalid_proof");
}

#[tokio::test]
async fn a_device_of_another_account_is_refused() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let ada_device = env.device("ada");
    let id = env.connect(&ada, &ada_device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Bob's enrolled device with Ada's access token.
    let bob_device = env.device("bob");
    let r = env.mint(&ada, &bob_device, &id, "create", STS, None).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "device_not_enrolled");
    // Bob's own token and device can't see Ada's connection.
    let bob = env.keycloak.token("bob");
    let r = env.mint(&bob, &bob_device, &id, "create", STS, None).await;
    assert_eq!(r.status, 404);
    assert_eq!(r.code(), "connection_not_found");
    assert_eq!(
        env.get(&format!("/v1/connections/{id}"), Some(&bob))
            .await
            .status,
        404
    );
    assert_eq!(
        env.get("/v1/connections", Some(&bob)).await.body["connections"],
        json!([])
    );
}

#[tokio::test]
async fn unknown_unenrolled_and_revoked_devices_are_refused() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let id = env.connect(&ada, &device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let stranger = DeviceKey::generate();
    let r = env.mint(&ada, &stranger, &id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (403, "device_not_enrolled"));

    let pending = DeviceKey::generate();
    env.put_device(&pending, "ada", false, false);
    let r = env.mint(&ada, &pending, &id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (403, "device_not_enrolled"));

    env.put_device(&device, "ada", true, true);
    let r = env.mint(&ada, &device, &id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (403, "device_revoked"));

    // The refusals are audited.
    let audit = env.get("/v1/audit", Some(&ada)).await;
    let codes: Vec<&str> = audit.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "token_refused")
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "device_revoked",
            "device_not_enrolled",
            "device_not_enrolled"
        ]
    );
}

#[tokio::test]
async fn a_replayed_or_misbound_proof_is_refused() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let id = env.connect(&ada, &device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let htu = format!("{}/v1/token", env.base);
    let body = json!({"connection": id, "purpose": "create", "audience": STS});
    let proof = device.proof(
        "POST",
        &htu,
        &ada,
        &id,
        "create",
        None,
        cua_oidc::now_secs(),
        &fresh_jti(),
    );
    let first = env
        .send("POST", "/v1/token", &ada, Some(&proof), Some(body.clone()))
        .await;
    assert_eq!(first.status, 200, "{}", first.body);
    let again = env
        .send("POST", "/v1/token", &ada, Some(&proof), Some(body.clone()))
        .await;
    assert_eq!((again.status, again.code()), (401, "proof_replayed"));

    // Signed for another purpose, connection or agent than the body asks.
    for (conn, purpose, agent) in [
        (id.as_str(), "check", None),
        ("conn_other", "create", None),
        (id.as_str(), "create", Some("sneaky")),
    ] {
        let p = device.proof(
            "POST",
            &htu,
            &ada,
            conn,
            purpose,
            agent,
            cua_oidc::now_secs(),
            &fresh_jti(),
        );
        let r = env
            .send("POST", "/v1/token", &ada, Some(&p), Some(body.clone()))
            .await;
        assert_eq!(
            (r.status, r.code()),
            (401, "invalid_proof"),
            "{conn} {purpose}"
        );
    }
    // Bound to another access token (a stolen proof is useless).
    let other = env.keycloak.token("ada");
    let p = device.proof(
        "POST",
        &htu,
        &other,
        &id,
        "create",
        None,
        cua_oidc::now_secs(),
        &fresh_jti(),
    );
    let r = env
        .send("POST", "/v1/token", &ada, Some(&p), Some(body.clone()))
        .await;
    assert_eq!((r.status, r.code()), (401, "invalid_proof"));
    // Stale.
    let p = device.proof(
        "POST",
        &htu,
        &ada,
        &id,
        "create",
        None,
        cua_oidc::now_secs() - 300,
        &fresh_jti(),
    );
    let r = env
        .send("POST", "/v1/token", &ada, Some(&p), Some(body))
        .await;
    assert_eq!((r.status, r.code()), (401, "invalid_proof"));
}

#[tokio::test]
async fn a_token_for_one_account_never_validates_as_another() {
    let env = Env::start().await;
    let (ada, bob) = (env.keycloak.token("ada"), env.keycloak.token("bob"));
    let (ada_device, bob_device) = (env.device("ada"), env.device("bob"));
    let ada_conn = env.connect(&ada, &ada_device, aws_connection()).await;
    let bob_conn = env.connect(&bob, &bob_device, aws_connection()).await;
    let (a, b) = (env.acct("ada"), env.acct("bob"));
    assert_ne!(a, b);
    let r = env
        .mint(
            &ada,
            &ada_device,
            ada_conn["id"].as_str().unwrap(),
            "create",
            STS,
            None,
        )
        .await;
    let token = r.body["token"].as_str().unwrap().to_owned();
    let ada_iss = format!("{}/a/{a}", env.base);
    let bob_iss = format!("{}/a/{b}", env.base);
    assert_eq!(r.body["issuer"], ada_iss);
    // Bob's cloud trusts only Bob's issuer: Ada's token fails there even
    // though both are signed by the same key.
    let jwks: jsonwebtoken::jwk::JwkSet =
        serde_json::from_value(env.get("/.well-known/jwks.json", None).await.body).unwrap();
    let kid = jsonwebtoken::decode_header(&token).unwrap().kid.unwrap();
    let key = DecodingKey::from_jwk(jwks.find(&kid).unwrap()).unwrap();
    let mut v = Validation::new(Algorithm::RS256);
    v.set_issuer(&[&bob_iss]);
    v.set_audience(&[STS]);
    assert!(jsonwebtoken::decode::<Value>(&token, &key, &v).is_err());
    // And the second line of defence, sub.
    let claims = verify_with_jwks(&env, &token, &ada_iss, STS).await;
    assert_ne!(claims["sub"], format!("account:{b}"));
    assert!(bob_conn["id"] != ada_conn["id"]);
}

#[tokio::test]
async fn the_audience_must_be_the_connections_clouds() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let aws = env.connect(&ada, &device, aws_connection()).await;
    let aws_id = aws["id"].as_str().unwrap();
    let provider = "//iam.googleapis.com/projects/123456/locations/global/workloadIdentityPools/cua/providers/cua";
    for aud in [
        "api://AzureADTokenExchange",
        provider,
        "https://evil.example",
        "",
    ] {
        let r = env.mint(&ada, &device, aws_id, "create", aud, None).await;
        assert_eq!((r.status, r.code()), (400, "audience_not_allowed"), "{aud}");
    }
    let gcp = env
        .connect(
            &ada,
            &device,
            json!({"cloud": "gcp", "identifiers": {"project_number": "123456", "pool": "cua", "provider": "cua"},
                   "limits": {"budget_usd_month": 10.0}}),
        )
        .await;
    let gcp_id = gcp["id"].as_str().unwrap();
    assert_eq!(
        env.mint(&ada, &device, gcp_id, "create", provider, None)
            .await
            .status,
        200
    );
    let other_provider = provider.replace("123456", "999999");
    let r = env
        .mint(&ada, &device, gcp_id, "create", &other_provider, None)
        .await;
    assert_eq!((r.status, r.code()), (400, "audience_not_allowed"));
    let r = env.mint(&ada, &device, gcp_id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (400, "audience_not_allowed"));
    let azure = env
        .connect(
            &ada,
            &device,
            json!({"cloud": "azure", "identifiers": {"tenant_id": "t", "client_id": "c", "subscription_id": "s", "resource_group": "cua-spaces"},
                   "limits": {"budget_usd_month": 10.0}}),
        )
        .await;
    let azure_id = azure["id"].as_str().unwrap();
    assert_eq!(
        env.mint(
            &ada,
            &device,
            azure_id,
            "create",
            "api://AzureADTokenExchange",
            None
        )
        .await
        .status,
        200
    );
    // A sealed-token cloud gets no tokens at all.
    let hetzner = env
        .connect(
            &ada,
            &device,
            json!({"cloud": "hetzner", "identifiers": {"project": "p"},
                   "sealed_token": {"ciphertext": "AAECAw==", "generation": 3}}),
        )
        .await;
    assert_eq!(hetzner["auth"], "sealed");
    assert_eq!(
        hetzner["sealed_token"],
        json!({"ciphertext": "AAECAw==", "generation": 3})
    );
    let r = env
        .mint(
            &ada,
            &device,
            hetzner["id"].as_str().unwrap(),
            "manage",
            STS,
            None,
        )
        .await;
    assert_eq!((r.status, r.code()), (400, "connection_not_oidc"));
}

#[tokio::test]
async fn pending_revoked_and_budget_refusals() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");

    // An agent links a cloud: it waits for a person.
    let r = env
        .call(
            "POST",
            "/v1/connections",
            &ada,
            &device,
            "",
            "connection.create",
            Some("helper"),
            Some(aws_connection()),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.body);
    assert_eq!(r.body["status"], "pending");
    assert_eq!(r.body["created_by"]["agent"], "helper");
    let id = r.body["id"].as_str().unwrap().to_owned();
    for purpose in ["create", "manage", "cost"] {
        let r = env
            .mint(&ada, &device, &id, purpose, STS, Some("helper"))
            .await;
        assert_eq!(
            (r.status, r.code()),
            (403, "connection_pending"),
            "{purpose}"
        );
    }
    // Checking the trust setup works while pending.
    assert_eq!(
        env.mint(&ada, &device, &id, "check", STS, Some("helper"))
            .await
            .status,
        200
    );

    // The agent can't approve; a person can.
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/approve"),
            &ada,
            &device,
            &id,
            "connection.approve",
            Some("helper"),
            None,
        )
        .await;
    assert_eq!((r.status, r.code()), (403, "agent_cannot_approve"));
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/approve"),
            &ada,
            &device,
            &id,
            "connection.approve",
            None,
            None,
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["status"], "connected");
    assert_eq!(r.body["approved_by"]["device"], device.id());
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/approve"),
            &ada,
            &device,
            &id,
            "connection.approve",
            None,
            None,
        )
        .await;
    assert_eq!((r.status, r.code()), (409, "nothing_to_approve"));
    assert_eq!(
        env.mint(&ada, &device, &id, "create", STS, Some("helper"))
            .await
            .status,
        200
    );

    // Budget used up: no create, but manage (stop, delete) still works.
    env.ledger.set(&env.acct("ada"), &id, 50.0);
    let r = env.mint(&ada, &device, &id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (403, "budget_exhausted"));
    assert_eq!(
        env.mint(&ada, &device, &id, "manage", STS, None)
            .await
            .status,
        200
    );
    env.ledger.set(&env.acct("ada"), &id, 49.99);
    assert_eq!(
        env.mint(&ada, &device, &id, "create", STS, None)
            .await
            .status,
        200
    );

    // Revoked: nothing at all.
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/revoke"),
            &ada,
            &device,
            &id,
            "connection.revoke",
            None,
            None,
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body["status"], "revoked");
    for purpose in ["create", "manage", "check", "cost"] {
        let r = env.mint(&ada, &device, &id, purpose, STS, None).await;
        assert_eq!(
            (r.status, r.code()),
            (403, "connection_revoked"),
            "{purpose}"
        );
    }
    // The same cloud can be linked again after a revoke.
    env.connect(&ada, &device, aws_connection()).await;
}

#[tokio::test]
async fn create_needs_a_budget() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let mut body = aws_connection();
    body["limits"] = json!({});
    let id = env.connect(&ada, &device, body).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = env.mint(&ada, &device, &id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (403, "budget_required"));
    assert_eq!(
        env.mint(&ada, &device, &id, "check", STS, None)
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn agents_can_lower_limits_but_raising_waits_for_a_person() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let id = env.connect(&ada, &device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/v1/connections/{id}/limits");
    let limits = |agent, body| {
        env.call(
            "POST",
            &path,
            &ada,
            &device,
            &id,
            "connection.limits",
            agent,
            Some(body),
        )
    };

    let r = limits(Some("helper"), json!({"budget_usd_month": 20.0})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["approval_required"], false);
    assert_eq!(r.body["connection"]["limits"]["budget_usd_month"], 20.0);

    let r = limits(
        Some("helper"),
        json!({"budget_usd_month": 500.0, "max_size": {"vcpu": 2, "memory_gib": 8}}),
    )
    .await;
    assert_eq!(r.body["approval_required"], true);
    assert_eq!(r.body["connection"]["status"], "connected");
    assert_eq!(r.body["connection"]["limits"]["budget_usd_month"], 20.0);
    assert_eq!(
        r.body["connection"]["limits"]["max_size"],
        json!({"vcpu": 2, "memory_gib": 8})
    );
    assert_eq!(
        r.body["connection"]["pending_limits"]["budget_usd_month"],
        500.0
    );

    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/approve"),
            &ada,
            &device,
            &id,
            "connection.approve",
            None,
            None,
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body["limits"]["budget_usd_month"], 500.0);
    assert!(r.body.get("pending_limits").is_none());

    // A person raises at once.
    let r = limits(None, json!({"ttl_hours": 48})).await;
    assert_eq!(r.body["approval_required"], false);
    assert_eq!(r.body["connection"]["limits"]["ttl_hours"], 48);
    // Bad values are refused.
    let r = limits(None, json!({"budget_usd_month": -1.0})).await;
    assert_eq!((r.status, r.code()), (400, "invalid_request"));
}

#[tokio::test]
async fn a_person_denies_an_agents_raise_or_link_and_an_agent_cannot() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let id = env.connect(&ada, &device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let call = |path: String, purpose: &'static str, agent: Option<&'static str>, body| {
        let id = id.clone();
        let ada = ada.clone();
        let env = &env;
        let device = &device;
        async move {
            env.call("POST", &path, &ada, device, &id, purpose, agent, body)
                .await
        }
    };
    let r = call(
        format!("/v1/connections/{id}/limits"),
        "connection.limits",
        Some("helper"),
        Some(json!({"budget_usd_month": 900.0})),
    )
    .await;
    assert_eq!(r.body["approval_required"], true);
    // Who asked is on the record, for the person who approves.
    assert_eq!(r.body["connection"]["pending_agent"], "helper");

    // The agent cannot deny (or approve) for the person.
    let r = call(
        format!("/v1/connections/{id}/deny"),
        "connection.deny",
        Some("helper"),
        None,
    )
    .await;
    assert_eq!((r.status, r.code()), (403, "agent_cannot_approve"));

    // A person denies: the raise is dropped, the limits in force stay.
    let r = call(
        format!("/v1/connections/{id}/deny"),
        "connection.deny",
        None,
        None,
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["status"], "connected");
    assert!(r.body.get("pending_limits").is_none());
    assert!(r.body.get("pending_agent").is_none());
    assert_ne!(r.body["limits"]["budget_usd_month"], 900.0);
    let r = call(
        format!("/v1/connections/{id}/deny"),
        "connection.deny",
        None,
        None,
    )
    .await;
    assert_eq!((r.status, r.code()), (409, "nothing_to_approve"));
    // The wrong proof purpose is refused.
    let r = call(
        format!("/v1/connections/{id}/deny"),
        "connection.approve",
        None,
        None,
    )
    .await;
    assert!(r.status == 401 || r.status == 403, "{}", r.status);

    // An agent's link that a person denies is revoked: no more tokens.
    let r = env
        .call(
            "DELETE",
            &format!("/v1/connections/{id}"),
            &ada,
            &device,
            &id,
            "connection.delete",
            None,
            None,
        )
        .await;
    assert!(r.status < 300, "{} {}", r.status, r.body);
    let r = env
        .call(
            "POST",
            "/v1/connections",
            &ada,
            &device,
            "",
            "connection.create",
            Some("helper"),
            Some(aws_connection()),
        )
        .await;
    assert_eq!(r.body["status"], "pending", "{}", r.body);
    let pending = r.body["id"].as_str().unwrap().to_owned();
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{pending}/deny"),
            &ada,
            &device,
            &pending,
            "connection.deny",
            None,
            None,
        )
        .await;
    assert_eq!(r.body["status"], "revoked", "{}", r.body);
    let r = env.mint(&ada, &device, &pending, "check", STS, None).await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn connections_are_validated_unique_and_deletable() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let first = env.connect(&ada, &device, aws_connection()).await;
    let r = env
        .call(
            "POST",
            "/v1/connections",
            &ada,
            &device,
            "",
            "connection.create",
            None,
            Some(aws_connection()),
        )
        .await;
    assert_eq!((r.status, r.code()), (409, "connection_exists"));
    for bad in [
        json!({"cloud": "aws", "identifiers": {}}),
        json!({"cloud": "aws", "identifiers": {"account_id": "1", "secret_access_key": "x"}}),
        json!({"cloud": "aws", "identifiers": {"account_id": "1"}, "sealed_token": {"ciphertext": "AA==", "generation": 1}}),
        json!({"cloud": "nimbus", "identifiers": {}}),
        json!({"cloud": "aws", "identifiers": {"account_id": "1"}, "access_key": "AKIA"}),
    ] {
        let r = env
            .call(
                "POST",
                "/v1/connections",
                &ada,
                &device,
                "",
                "connection.create",
                None,
                Some(bad.clone()),
            )
            .await;
        assert_eq!((r.status, r.code()), (400, "invalid_request"), "{bad}");
    }
    // Creating needs a device proof for the create action.
    let r = env
        .call(
            "POST",
            "/v1/connections",
            &ada,
            &device,
            "",
            "connection.delete",
            None,
            Some(aws_connection()),
        )
        .await;
    assert_eq!((r.status, r.code()), (401, "invalid_proof"));

    let id = first["id"].as_str().unwrap();
    let list = env.get("/v1/connections", Some(&ada)).await;
    assert_eq!(list.body["connections"].as_array().unwrap().len(), 1);
    let r = env
        .call(
            "DELETE",
            &format!("/v1/connections/{id}"),
            &ada,
            &device,
            id,
            "connection.delete",
            None,
            None,
        )
        .await;
    assert_eq!(r.status, 204);
    let r = env
        .call(
            "DELETE",
            &format!("/v1/connections/{id}"),
            &ada,
            &device,
            id,
            "connection.delete",
            None,
            None,
        )
        .await;
    assert_eq!((r.status, r.code()), (404, "connection_not_found"));
    let kinds: Vec<String> = env
        .registry
        .all_audit()
        .iter()
        .map(|r| r.kind.clone())
        .collect();
    assert!(kinds.contains(&"connection_created".to_owned()));
    assert!(kinds.contains(&"connection_deleted".to_owned()));
}

#[tokio::test]
async fn account_endpoint_and_aws_template() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let acct = env.acct("ada");
    let r = env.get("/v1/account", Some(&ada)).await;
    assert_eq!(r.body["account"], acct);
    assert_eq!(r.body["issuer"], format!("{}/a/{acct}", env.base));
    assert_eq!(r.body["subject"], format!("account:{acct}"));

    let host = env.base.trim_start_matches("http://").to_owned();
    for (path, token) in [
        (format!("/v1/templates/aws.yaml?acct={acct}"), None),
        ("/v1/templates/aws.yaml".to_owned(), Some(ada.as_str())),
    ] {
        let r = env.get(&path, token).await;
        assert_eq!(r.status, 200);
        let text = r.body.as_str().unwrap();
        let plain = text
            .replace("!Sub ", "")
            .replace("!GetAtt ", "")
            .replace("!Ref ", "");
        let doc: serde_yaml::Value = serde_yaml::from_str(&plain).unwrap();
        assert_eq!(
            doc["Parameters"]["CuaAccount"]["Default"].as_str(),
            Some(acct.as_str())
        );
        assert_eq!(
            doc["Parameters"]["IssuerHost"]["Default"].as_str(),
            Some(host.as_str())
        );
        assert_eq!(
            doc["Resources"]["CuaOidcProvider"]["Properties"]["Url"].as_str(),
            Some("https://${IssuerHost}/a/${CuaAccount}")
        );
    }
    assert_eq!(
        env.get("/v1/templates/aws.yaml?acct=nope", None)
            .await
            .status,
        400
    );
    assert_eq!(env.get("/v1/templates/aws.yaml", None).await.status, 400);
}

/// Reports what `key`'s device spent on `id` in `month`.
async fn report(
    env: &Env,
    token: &str,
    key: &DeviceKey,
    id: &str,
    month: &str,
    usd: f64,
) -> common::Response {
    env.call(
        "POST",
        &format!("/v1/connections/{id}/spend"),
        token,
        key,
        id,
        "connection.spend",
        None,
        Some(json!({"month": month, "usd": usd})),
    )
    .await
}

#[tokio::test]
async fn spend_from_every_device_adds_up_against_the_budget() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let laptop = env.device("ada");
    let desktop = env.device("ada");
    let third = env.device("ada");
    // Budget 50 (aws_connection).
    let id = env.connect(&ada, &laptop, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let month = cua_oidc::this_month();

    let r = report(&env, &ada, &laptop, &id, &month, 20.0).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["spent_month_usd"], 20.0);
    // A device's report is its running total: a repeat replaces it.
    let r = report(&env, &ada, &laptop, &id, &month, 30.0).await;
    assert_eq!(r.body["spent_month_usd"], 30.0);
    assert_eq!(
        env.mint(&ada, &third, &id, "create", STS, None)
            .await
            .status,
        200
    );
    let r = report(&env, &ada, &desktop, &id, &month, 20.0).await;
    assert_eq!(r.body["spent_month_usd"], 50.0);
    assert_eq!(r.body["budget_usd_month"], 50.0);

    // The account is at its budget: no device creates; stop and delete
    // (manage), the check and cost reads still work.
    let r = env.mint(&ada, &third, &id, "create", STS, None).await;
    assert_eq!((r.status, r.code()), (403, "budget_exhausted"));
    for purpose in ["manage", "check", "cost"] {
        assert_eq!(
            env.mint(&ada, &third, &id, purpose, STS, None).await.status,
            200,
            "{purpose}"
        );
    }

    // Connections show the account-wide figure.
    let one = env.get(&format!("/v1/connections/{id}"), Some(&ada)).await;
    assert_eq!(one.body["spent_month_usd"], 50.0);
    let all = env.get("/v1/connections", Some(&ada)).await;
    assert_eq!(all.body["connections"][0]["spent_month_usd"], 50.0);

    // Last month's late report doesn't count against this month.
    let last = {
        let (y, m) = month.split_once('-').unwrap();
        let (y, m): (i32, u32) = (y.parse().unwrap(), m.parse().unwrap());
        if m == 1 {
            format!("{:04}-12", y - 1)
        } else {
            format!("{y:04}-{:02}", m - 1)
        }
    };
    let r = report(&env, &ada, &third, &id, &last, 500.0).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["spent_month_usd"], 50.0);
}

#[tokio::test]
async fn spend_reports_are_validated_and_bound_to_the_account_and_device() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let id = env.connect(&ada, &device, aws_connection()).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let month = cua_oidc::this_month();

    for usd in [-1.0, 1e12] {
        let r = report(&env, &ada, &device, &id, &month, usd).await;
        assert_eq!((r.status, r.code()), (400, "invalid_request"), "{usd}");
    }
    for bad in ["2001-01", "soon", ""] {
        let r = report(&env, &ada, &device, &id, bad, 1.0).await;
        assert_eq!((r.status, r.code()), (400, "invalid_request"), "{bad}");
    }
    // NaN is not JSON; an unknown field is refused.
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/spend"),
            &ada,
            &device,
            &id,
            "connection.spend",
            None,
            Some(json!({"month": month, "usd": 1.0, "device": "dev_other"})),
        )
        .await;
    assert_eq!(r.status, 400);
    // A proof for another purpose doesn't count.
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/spend"),
            &ada,
            &device,
            &id,
            "connection.limits",
            None,
            Some(json!({"month": month, "usd": 1.0})),
        )
        .await;
    assert_eq!(r.status, 401, "{}", r.body);

    // Another account's device, or another account, can't report here.
    let bob = env.keycloak.token("bob");
    let bob_device = env.device("bob");
    let r = report(&env, &ada, &bob_device, &id, &month, 99.0).await;
    assert_eq!((r.status, r.code()), (403, "device_not_enrolled"));
    let r = report(&env, &bob, &bob_device, &id, &month, 99.0).await;
    assert_eq!((r.status, r.code()), (404, "connection_not_found"));

    // A revoked device can't report.
    let gone = env.device("ada");
    env.put_device(&gone, "ada", true, true);
    let r = report(&env, &ada, &gone, &id, &month, 99.0).await;
    assert_eq!((r.status, r.code()), (403, "device_revoked"));

    // None of that counted.
    let one = env.get(&format!("/v1/connections/{id}"), Some(&ada)).await;
    assert_eq!(one.body["spent_month_usd"], 0.0);
}

#[tokio::test]
async fn a_sealed_token_is_swapped_only_from_the_version_the_caller_read() {
    let env = Env::start().await;
    let ada = env.keycloak.token("ada");
    let device = env.device("ada");
    let created = env
        .connect(
            &ada,
            &device,
            json!({"cloud": "hetzner", "identifiers": {"project": "p"},
                   "limits": {"budget_usd_month": 50.0}}),
        )
        .await;
    let id = created["id"].as_str().unwrap().to_owned();
    assert!(created.get("sealed_token").is_none());
    let path = format!("/v1/connections/{id}/sealed-token");
    let put = |agent: Option<&'static str>, body: Value| {
        env.call(
            "POST",
            &path,
            &ada,
            &device,
            &id,
            "connection.seal",
            agent,
            Some(body),
        )
    };

    // The first token, from the version the caller read.
    let r = put(
        None,
        json!({"sealed_token": {"ciphertext": "AAEC", "generation": 1}, "expected_version": 1}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        r.body["sealed_token"],
        json!({"ciphertext": "AAEC", "generation": 1})
    );
    assert_eq!(r.body["version"], 2);
    assert_eq!(r.body["status"], "connected");

    // A stale read (another holder swapped it meanwhile) is a conflict.
    let r = put(
        None,
        json!({"sealed_token": {"ciphertext": "AAED", "generation": 2}, "expected_version": 1}),
    )
    .await;
    assert_eq!((r.status, r.code()), (409, "conflict"));
    let r = put(
        None,
        json!({"sealed_token": {"ciphertext": "AAED", "generation": 2}, "expected_generation": 0}),
    )
    .await;
    assert_eq!((r.status, r.code()), (409, "conflict"));
    // Re-sealed under the next generation, from the right generation.
    let r = put(
        None,
        json!({"sealed_token": {"ciphertext": "AAED", "generation": 2}, "expected_generation": 1}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["sealed_token"]["generation"], 2);
    // The loser of the race finds it changed.
    let r = put(
        None,
        json!({"sealed_token": {"ciphertext": "AAEE", "generation": 2}, "expected_generation": 1}),
    )
    .await;
    assert_eq!((r.status, r.code()), (409, "conflict"));

    // A blind write, an empty or oversized blob, generation 0: refused.
    for body in [
        json!({"sealed_token": {"ciphertext": "AAEE", "generation": 2}}),
        json!({"sealed_token": {"ciphertext": "", "generation": 2}, "expected_generation": 2}),
        json!({"sealed_token": {"ciphertext": "AAEE", "generation": 0}, "expected_generation": 2}),
        json!({"sealed_token": {"ciphertext": base64::engine::general_purpose::STANDARD.encode(vec![0u8; 16 * 1024 + 1]), "generation": 2}, "expected_generation": 2}),
        json!({"sealed_token": {"ciphertext": "AAEE", "generation": 2}, "expected_generation": 2, "token": "plain"}),
    ] {
        let r = put(None, body).await;
        assert_eq!((r.status, r.code()), (400, "invalid_request"), "{}", r.body);
    }

    // A token an agent puts in waits for a person.
    let r = put(
        Some("helper"),
        json!({"sealed_token": {"ciphertext": "AAEF", "generation": 2}, "expected_generation": 2}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["status"], "pending");

    // The swap was signed for this connection and purpose only.
    let r = env
        .call(
            "POST",
            &path,
            &ada,
            &device,
            &id,
            "connection.limits",
            None,
            Some(json!({"sealed_token": {"ciphertext": "AAEF", "generation": 2}, "expected_generation": 2})),
        )
        .await;
    assert_eq!((r.status, r.code()), (401, "invalid_proof"));

    let kinds: Vec<String> = env
        .get(&format!("/v1/audit?connection={id}"), Some(&ada))
        .await
        .body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        kinds.contains(&"sealed_token_resealed".to_owned()),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&"sealed_token_replaced".to_owned()),
        "{kinds:?}"
    );

    // An OIDC connection takes no token.
    let aws = env.connect(&ada, &device, aws_connection()).await;
    let aws_id = aws["id"].as_str().unwrap();
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{aws_id}/sealed-token"),
            &ada,
            &device,
            aws_id,
            "connection.seal",
            None,
            Some(json!({"sealed_token": {"ciphertext": "AAEF", "generation": 1}, "expected_version": 1})),
        )
        .await;
    assert_eq!((r.status, r.code()), (400, "connection_not_sealed"));

    // Disconnect drops the ciphertext, and nothing is swapped in after.
    let r = env
        .call(
            "POST",
            &format!("/v1/connections/{id}/revoke"),
            &ada,
            &device,
            &id,
            "connection.revoke",
            None,
            None,
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.get("sealed_token").is_none(), "{}", r.body);
    let r = put(
        None,
        json!({"sealed_token": {"ciphertext": "AAEF", "generation": 3}, "expected_generation": 0}),
    )
    .await;
    assert_eq!((r.status, r.code()), (403, "connection_revoked"));
}
