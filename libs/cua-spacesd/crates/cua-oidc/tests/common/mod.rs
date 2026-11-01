// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A test issuer: fixed RSA keys, a fake Keycloak, in-memory devices,
//! registry and ledger, served on a loopback port.

#![allow(dead_code)]

use std::sync::Arc;

use base64::Engine as _;
use cua_oidc::account::AccountIds;
use cua_oidc::api::Issuer;
use cua_oidc::devices::{DeviceInfo, MemoryDeviceDirectory};
use cua_oidc::keycloak::{KeycloakConfig, KeycloakValidator};
use cua_oidc::proof::testing::DeviceKey;
use cua_oidc::registry::{MemoryLedger, MemoryRegistry};
use cua_oidc::signer::{FileSigner, KeySet, Signer as _};
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

pub const KEYCLOAK: &str = "https://auth.test/realms/cua";
pub const SECRET: &[u8] = b"test-account-secret-0123456789abcdef";

pub fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// A Keycloak stand-in: ES256 access tokens and their JWKS.
pub struct FakeKeycloak {
    pub issuer: String,
    kid: String,
    encoding: jsonwebtoken::EncodingKey,
    public: Vec<u8>,
}

impl FakeKeycloak {
    pub fn new(issuer: &str) -> Self {
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        Self {
            issuer: issuer.to_owned(),
            kid: format!("kc-{}", hex::encode(&pair.public_key().as_ref()[1..9])),
            encoding: jsonwebtoken::EncodingKey::from_ec_der(pkcs8.as_ref()),
            public: pair.public_key().as_ref().to_vec(),
        }
    }

    pub fn jwks(&self) -> jsonwebtoken::jwk::JwkSet {
        serde_json::from_value(json!({"keys": [{
            "kty": "EC", "crv": "P-256", "kid": self.kid, "alg": "ES256", "use": "sig",
            "x": b64().encode(&self.public[1..33]), "y": b64().encode(&self.public[33..65]),
        }]}))
        .unwrap()
    }

    pub fn token_for(&self, sub: &str, aud: &str, ttl: i64) -> String {
        let now = cua_oidc::now_secs() as i64;
        let claims =
            json!({"iss": self.issuer, "sub": sub, "aud": aud, "iat": now, "exp": now + ttl});
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        jsonwebtoken::encode(&header, &claims, &self.encoding).unwrap()
    }

    pub fn token(&self, sub: &str) -> String {
        self.token_for(sub, "cua-oidc", 300)
    }
}

pub fn signer(name: &str) -> FileSigner {
    let bytes = match name {
        "a" => &include_bytes!("../fixtures/test-rsa-a.pk8.der")[..],
        "b" => &include_bytes!("../fixtures/test-rsa-b.pk8.der")[..],
        _ => &include_bytes!("../fixtures/test-rsa-c.pk8.der")[..],
    };
    FileSigner::from_pkcs8(bytes).unwrap()
}

pub struct Env {
    pub base: String,
    pub keycloak: FakeKeycloak,
    pub devices: Arc<MemoryDeviceDirectory>,
    pub registry: Arc<MemoryRegistry>,
    pub ledger: Arc<MemoryLedger>,
    pub accounts: AccountIds,
    pub http: reqwest::Client,
}

pub struct Response {
    pub status: u16,
    pub body: Value,
}

impl Response {
    pub fn code(&self) -> &str {
        self.body["error"].as_str().unwrap_or("")
    }
}

impl Env {
    pub async fn start() -> Self {
        let keycloak = FakeKeycloak::new(KEYCLOAK);
        let devices = Arc::new(MemoryDeviceDirectory::new());
        let registry = Arc::new(MemoryRegistry::new());
        let ledger = Arc::new(MemoryLedger::new());
        let accounts = AccountIds::new(SECRET).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let keys = KeySet::new(Arc::new(signer("a")))
            .with_next(signer("b").public_key().clone())
            .with_previous(signer("c").public_key().clone());
        let issuer = Issuer {
            base_url: base.clone(),
            keys,
            keycloak: Arc::new(KeycloakValidator::with_jwks(
                KeycloakConfig::new(KEYCLOAK),
                keycloak.jwks(),
            )),
            accounts: accounts.clone(),
            devices: devices.clone(),
            registry: registry.clone(),
            ledger: ledger.clone(),
        };
        let router = issuer.router();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base,
            keycloak,
            devices,
            registry,
            ledger,
            accounts,
            http: reqwest::Client::new(),
        }
    }

    /// An enrolled device of Keycloak account `account`.
    pub fn device(&self, account: &str) -> DeviceKey {
        let key = DeviceKey::generate();
        self.put_device(&key, account, true, false);
        key
    }

    pub fn put_device(&self, key: &DeviceKey, account: &str, enrolled: bool, revoked: bool) {
        self.devices.put(DeviceInfo {
            id: key.id(),
            account: account.to_owned(),
            public_jwk: Some(key.jwk()),
            enrolled,
            revoked,
        });
    }

    pub fn acct(&self, account: &str) -> String {
        self.accounts.derive(account)
    }

    pub async fn get(&self, path: &str, token: Option<&str>) -> Response {
        let mut req = self.http.get(format!("{}{path}", self.base));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        Response {
            status,
            body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
        }
    }

    /// A request with a device proof.
    #[allow(clippy::too_many_arguments)]
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        token: &str,
        key: &DeviceKey,
        connection: &str,
        purpose: &str,
        agent: Option<&str>,
        body: Option<Value>,
    ) -> Response {
        let proof = key.proof(
            method,
            &format!("{}{path}", self.base),
            token,
            connection,
            purpose,
            agent,
            cua_oidc::now_secs(),
            &fresh_jti(),
        );
        self.send(method, path, token, Some(&proof), body).await
    }

    pub async fn send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        proof: Option<&str>,
        body: Option<Value>,
    ) -> Response {
        let mut req = self
            .http
            .request(method.parse().unwrap(), format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(p) = proof {
            req = req.header("DPoP", p);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        Response {
            status,
            body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
        }
    }

    /// Creates a connection as a person on `key`'s device.
    pub async fn connect(&self, token: &str, key: &DeviceKey, body: Value) -> Value {
        let r = self
            .call(
                "POST",
                "/v1/connections",
                token,
                key,
                "",
                "connection.create",
                None,
                Some(body),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.body);
        r.body
    }

    /// Asks for a token.
    #[allow(clippy::too_many_arguments)]
    pub async fn mint(
        &self,
        token: &str,
        key: &DeviceKey,
        connection: &str,
        purpose: &str,
        audience: &str,
        agent: Option<&str>,
    ) -> Response {
        let mut body = json!({"connection": connection, "purpose": purpose, "audience": audience});
        if let Some(a) = agent {
            body["agent"] = a.into();
        }
        self.call(
            "POST",
            "/v1/token",
            token,
            key,
            connection,
            purpose,
            agent,
            Some(body),
        )
        .await
    }
}

pub fn fresh_jti() -> String {
    format!("jti-{}", hex::encode(rand::random::<[u8; 12]>()))
}

pub fn aws_connection() -> Value {
    json!({
        "cloud": "aws",
        "identifiers": {"account_id": "123456789012", "role_arn": "arn:aws:iam::123456789012:role/cua-spaces"},
        "region": "us-west-2",
        "limits": {"budget_usd_month": 50.0, "max_size": {"vcpu": 4, "memory_gib": 16}, "ttl_hours": 4},
    })
}
