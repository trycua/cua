#![allow(deprecated)] // exercises the deprecated `apply_pool` wrapper too
//! Per-claim env tokens (Fleet claim Secrets, trycua/cloud#7885) against the
//! fake control plane: the token travels only in a write-only
//! `cua-claim-<claim>` Secret referenced by the claim, and `acquire` waits
//! (bounded) for the driver to have it on both runtimes, releasing the
//! claim with `ClaimSecretsNotDelivered` when it never arrives.

use cua_fleet::claim_secrets::{self, generate_claim_token};
use cua_fleet::sdk::{HttpClient, HttpRequest};
use cua_fleet::{
    BoundSandbox, ClaimOptions, ClaimSecretsWait, FleetClient, PoolOptions, RuntimeKind,
    SandboxSpec, TokenProbe, TokenState, testing::FakeFleet,
};
use std::{sync::Arc, time::Duration};

const IMAGE: &str = "ghcr.io/trycua/cua-desktop-linux:cua-e2e-test";

#[tokio::test]
async fn fake_claim_secrets_are_write_only_and_prefixed() {
    let fake = FakeFleet::new();
    // Built through serde so the test does not depend on every field of
    // HttpRequest (it gains optional ones over time).
    let post = |body: serde_json::Value| -> HttpRequest {
        serde_json::from_value(serde_json::json!({
            "method": "POST",
            "url": "https://fleet.test/api/k8s/api/v1/namespaces/ns/secrets",
            "headers": [],
            "body": serde_json::to_vec(&body).unwrap(),
        }))
        .unwrap()
    };
    let ok = fake
        .execute(post(serde_json::json!({
            "metadata": {"name": "cua-claim-c1"}, "type": "Opaque",
            "stringData": {"env-token": "t"}})))
        .await
        .unwrap();
    assert_eq!(ok.status, 201);
    let other = fake
        .execute(post(
            serde_json::json!({"metadata": {"name": "ecr-credentials"}, "type": "Opaque"}),
        ))
        .await
        .unwrap();
    assert_eq!(other.status, 403);
    let read = fake
        .execute(HttpRequest {
            method: "GET".into(),
            ..post(serde_json::json!({}))
        })
        .await
        .unwrap();
    assert_eq!(read.status, 403, "secrets are write-only");
    assert!(fake.exists("secret", "ns", "cua-claim-c1"));
}

#[tokio::test]
async fn claim_token_travels_only_in_the_claim_secret() {
    const { assert!(claim_secrets::SUPPORTED) };
    let fake = FakeFleet::new();
    let fleet = fake
        .client()
        .with_claim_secrets_wait(instant(TokenState::Delivered));
    let mut spec = SandboxSpec::new(IMAGE);
    spec.claim_secrets = true;
    spec.services = [("env".to_string(), 3211)].into();
    let handle = fleet
        .apply(
            "cua-e2e-secrets",
            &spec,
            &PoolOptions {
                runtime: Some(RuntimeKind::Gvisor),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let t = fake
        .object("template", "cua-e2e-secrets", "cua-e2e-secrets")
        .unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["claimSecrets"], true);

    // Malformed tokens are refused before any request.
    let before = fake.requests().len();
    assert!(
        fleet
            .claim(
                &handle.pool,
                ClaimOptions {
                    claim_token: Some("bad token".into()),
                    ..Default::default()
                },
            )
            .await
            .is_err()
    );
    assert_eq!(fake.requests().len(), before);

    let token = generate_claim_token();
    let bound = fleet
        .acquire(
            &handle.pool,
            ClaimOptions {
                name: Some("c1".into()),
                claim_token: Some(token.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(bound.claim, "c1");
    let claim_json = fake.object("claim", "cua-e2e-secrets", "c1").unwrap();
    assert_eq!(claim_json["spec"]["secretRef"]["name"], "cua-claim-c1");
    let secret = fake
        .object("secret", "cua-e2e-secrets", "cua-claim-c1")
        .unwrap();
    assert_eq!(secret["type"], "Opaque");
    assert_eq!(secret["metadata"]["labels"]["osgym.cua.ai/claim"], "c1");
    // The token is only in the Secret POST, never in the claim.
    for r in fake.requests() {
        let body = r.body.map(|b| b.to_string()).unwrap_or_default();
        if r.path.ends_with("/secrets") {
            assert!(body.contains(claim_secrets::ENV_TOKEN_KEY));
        } else {
            assert!(!body.contains(&token), "{} {}", r.method, r.path);
        }
    }
    // Releasing deletes the claim and its Secret.
    fleet.release("cua-e2e-secrets", "c1").await.unwrap();
    assert!(!fake.exists("secret", "cua-e2e-secrets", "cua-claim-c1"));
    assert!(!fake.exists("claim", "cua-e2e-secrets", "c1"));
}

/// A probe that answers `state` at once, counting calls.
struct Scripted(Vec<TokenState>, std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl TokenProbe for Scripted {
    async fn probe(&self, _: &FleetClient, _: &BoundSandbox, _: &str) -> TokenState {
        let i = self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0[i.min(self.0.len() - 1)].clone()
    }
}

fn scripted(states: Vec<TokenState>, budget_ms: u64) -> (ClaimSecretsWait, Arc<Scripted>) {
    let probe = Arc::new(Scripted(states, Default::default()));
    (
        ClaimSecretsWait {
            budget: Duration::from_millis(budget_ms),
            every: Duration::from_millis(10),
            probe: probe.clone(),
        },
        probe,
    )
}

fn instant(state: TokenState) -> ClaimSecretsWait {
    scripted(vec![state], 1000).0
}

async fn secrets_pool(fleet: &FleetClient, name: &str, runtime: RuntimeKind) -> cua_fleet::Pool {
    let mut spec = SandboxSpec::new(IMAGE);
    spec.claim_secrets = true;
    spec.services = [("env".to_string(), 3211)].into();
    fleet
        .apply(
            name,
            &spec,
            &PoolOptions {
                runtime: Some(runtime),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .pool
}

#[tokio::test]
async fn awaiting_token_is_retried_until_delivered() {
    let fake = FakeFleet::new();
    let (wait, probe) = scripted(
        vec![
            TokenState::Unknown("not up".into()),
            TokenState::Awaiting("awaiting token".into()),
            TokenState::Delivered,
        ],
        5_000,
    );
    let fleet = fake.client().with_claim_secrets_wait(wait);
    let pool = secrets_pool(&fleet, "cua-e2e-sec-retry", RuntimeKind::Gvisor).await;
    fleet
        .acquire(
            &pool,
            ClaimOptions {
                name: Some("c2".into()),
                claim_token: Some(generate_claim_token()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(probe.1.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(fake.exists("claim", "cua-e2e-sec-retry", "c2"));
}

#[tokio::test]
async fn undelivered_secrets_release_the_claim_and_raise_on_both_runtimes() {
    for (i, runtime) in [RuntimeKind::Gvisor, RuntimeKind::Kubevirt]
        .into_iter()
        .enumerate()
    {
        let fake = FakeFleet::new();
        let (wait, probe) = scripted(vec![TokenState::Awaiting("awaiting token".into())], 200);
        let fleet = fake.client().with_claim_secrets_wait(wait);
        let pool_name = format!("cua-e2e-sec-never-{i}");
        let pool = secrets_pool(&fleet, &pool_name, runtime.clone()).await;
        let started = std::time::Instant::now();
        let err = fleet
            .acquire(
                &pool,
                ClaimOptions {
                    name: Some("c3".into()),
                    claim_token: Some(generate_claim_token()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "never hangs");
        match &err {
            cua_fleet::Error::ClaimSecretsNotDelivered {
                claim, runtime: rt, ..
            } => {
                assert_eq!(claim, "c3");
                assert_eq!(rt, cua_fleet::runtime_name(&runtime));
            }
            other => panic!("{other:?}"),
        }
        assert!(err.to_string().contains("released"), "{err}");
        let probes = probe.1.load(std::sync::atomic::Ordering::SeqCst);
        assert!((2..=25).contains(&probes), "bounded polling: {probes}");
        assert!(!fake.exists("claim", &pool_name, "c3"), "claim released");
        assert!(
            !fake.exists("secret", &pool_name, "cua-claim-c3"),
            "secret wiped"
        );
    }
}

#[tokio::test]
async fn unverifiable_sandboxes_are_accepted_after_the_budget() {
    let fake = FakeFleet::new();
    let (wait, _) = scripted(vec![TokenState::Unknown("no driver".into())], 100);
    let fleet = fake.client().with_claim_secrets_wait(wait);
    let pool = secrets_pool(&fleet, "cua-e2e-sec-unknown", RuntimeKind::Gvisor).await;
    fleet
        .acquire(
            &pool,
            ClaimOptions {
                claim_token: Some(generate_claim_token()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

/// Managed pools (`cua sb create IMAGE --on cloud`): a key with
/// `claim_secrets` gets its own pool whose template opts in, the claim
/// carries the token in its Secret, and a token that never reaches the
/// driver releases the claim (with its Secret) instead of handing out a
/// sandbox no client can authenticate to.
#[tokio::test]
async fn managed_pools_deliver_claim_tokens() {
    use cua_fleet::{AcquireOpts, AutoPoolConfig, PoolManager, PoolSpecKey};
    let home = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let cfg = |fleet: FleetClient| {
        PoolManager::new(
            fleet,
            AutoPoolConfig {
                home: home.path().to_path_buf(),
                idle_gc: None,
                heartbeat_every: Some(Duration::from_millis(20)),
                clock: fake.clock(),
                ..AutoPoolConfig::default()
            },
        )
    };
    let key = PoolSpecKey::new(IMAGE)
        .runtime(RuntimeKind::Gvisor)
        .services([("env", 3211u16)]);
    let with_secrets = PoolSpecKey {
        claim_secrets: true,
        ..key.clone()
    };
    // Its own pool: templates without the opt-in keep theirs.
    assert_ne!(key.spec_hash(), with_secrets.spec_hash());
    assert!(!key.canonical().contains("claim_secrets"));

    // A token without the template opt-in is refused before any request.
    let mgr = cfg(fake
        .client()
        .with_claim_secrets_wait(instant(TokenState::Delivered)));
    let before = fake.requests().len();
    let token = generate_claim_token();
    let refused = mgr
        .acquire(
            key.clone(),
            AcquireOpts {
                claim_token: Some(token.clone()),
                ..Default::default()
            },
        )
        .await;
    assert!(refused.is_err());
    assert_eq!(fake.requests().len(), before);

    let c = mgr
        .acquire(
            with_secrets.clone(),
            AcquireOpts {
                name: Some("cua-e2e-managed".into()),
                claim_token: Some(token.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ns = c.pool.clone();
    let t = fake.object("template", &ns, &ns).unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["claimSecrets"], true);
    let claim = fake.object("claim", &ns, "cua-e2e-managed").unwrap();
    assert_eq!(
        claim["spec"]["secretRef"]["name"],
        "cua-claim-cua-e2e-managed"
    );
    assert!(fake.exists("secret", &ns, "cua-claim-cua-e2e-managed"));
    for r in fake.requests() {
        let body = r.body.map(|b| b.to_string()).unwrap_or_default();
        if !r.path.ends_with("/secrets") {
            assert!(!body.contains(&token), "{} {}", r.method, r.path);
        }
    }
    c.release().await.unwrap();
    assert!(!fake.exists("claim", &ns, "cua-e2e-managed"));
    assert!(!fake.exists("secret", &ns, "cua-claim-cua-e2e-managed"));

    // Never delivered: the claim and its Secret are released, and the
    // error says so.
    let (wait, probe) = scripted(vec![TokenState::Awaiting("awaiting token".into())], 60);
    let mgr = cfg(fake.client().with_claim_secrets_wait(wait));
    let err = mgr
        .acquire(
            with_secrets,
            AcquireOpts {
                name: Some("cua-e2e-undelivered".into()),
                claim_token: Some(generate_claim_token()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, cua_fleet::Error::ClaimSecretsNotDelivered { .. }),
        "{err}"
    );
    assert!(probe.1.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    assert!(!fake.exists("claim", &ns, "cua-e2e-undelivered"));
    assert!(!fake.exists("secret", &ns, "cua-claim-cua-e2e-undelivered"));
}
