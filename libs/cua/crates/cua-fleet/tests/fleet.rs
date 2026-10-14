#![allow(deprecated)] // exercises the deprecated `apply_pool` wrapper too
//! cua-fleet against the in-memory fake Fleet control plane.

use cua_fleet::{ClaimOptions, Error, PoolSpec, RuntimeKind, testing::FakeFleet};
use cua_spacesd_client::testing::{MockAuth, MockGateway, MockServer};
use std::time::Duration;

const IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34";

#[tokio::test]
async fn apply_pool_creates_namespace_pool_and_template() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let spec = PoolSpec::new("cua-e2e-apply", IMAGE).services([("server", 8000u16)]);
    let handle = fleet.apply_pool(&spec).await.unwrap();
    assert_eq!(handle.name(), "cua-e2e-apply");
    assert!(fake.namespace_exists("cua-e2e-apply"));
    let t = fake
        .object("template", "cua-e2e-apply", "cua-e2e-apply")
        .unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["containerDiskImage"], IMAGE);
    assert_eq!(t["spec"]["vmTemplate"]["services"][0]["name"], "server");
    assert_eq!(t["spec"]["vmTemplate"]["services"][0]["targetPort"], 8000);
    assert!(t["spec"]["vmTemplate"].get("probes").is_none());
    // Every control-plane request carried the Fleet bearer.
    assert!(
        fake.requests()
            .iter()
            .all(|r| r.header("authorization") == Some("Bearer fake-fleet-token"))
    );

    // Re-apply reconciles (PATCH) instead of failing.
    let handle = fleet
        .apply_pool(&spec.clone().runtime(RuntimeKind::Gvisor))
        .await
        .unwrap();
    let t = fake
        .object("template", "cua-e2e-apply", "cua-e2e-apply")
        .unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["runtime"], "gvisor");
    assert!(fake.requests().iter().any(|r| r.method == "PATCH"));

    fleet.delete_pool(handle).await.unwrap();
    assert!(!fake.exists("pool", "cua-e2e-apply", "cua-e2e-apply"));
    assert!(!fake.namespace_exists("cua-e2e-apply"));
}

#[tokio::test]
async fn apply_pool_rolls_back_pool_when_template_fails() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().template_create_status = Some(500);
    let fleet = fake.client();
    let err = fleet
        .apply_pool(&PoolSpec::new("cua-e2e-rollback", IMAGE))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Sdk(_)), "{err:?}");
    assert!(!fake.exists("pool", "cua-e2e-rollback", "cua-e2e-rollback"));
    assert!(!fake.namespace_exists("cua-e2e-rollback"));
}

#[tokio::test]
async fn claim_with_name_reattaches_and_keep_alive_renews() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().pending_reads = 2;
    let fleet = fake.client();
    let handle = fleet
        .apply_pool(&PoolSpec::new("cua-e2e-claims", IMAGE))
        .await
        .unwrap();
    let opts = ClaimOptions {
        name: Some("my-claim".into()),
        ..Default::default()
    };
    let (claim, created) = fleet.claim(&handle.pool, opts.clone()).await.unwrap();
    assert!(created);
    let bound = fleet.wait_claim(&claim).await.unwrap();
    assert_eq!(bound.name, "sbx-my-claim");
    assert_eq!(bound.services, vec!["env".to_string()]);

    // The same name reattaches instead of creating a second claim.
    let (again, created) = fleet.claim(&handle.pool, opts).await.unwrap();
    assert!(!created);
    assert_eq!(again.metadata.name, "my-claim");
    assert_eq!(fleet.list_claims("cua-e2e-claims").await.unwrap().len(), 1);
    let reattached = fleet
        .attach_claim("cua-e2e-claims", "my-claim")
        .await
        .unwrap();
    assert_eq!(reattached, bound);

    let shutdown = fleet
        .keep_alive("cua-e2e-claims", "my-claim", Duration::from_secs(600))
        .await
        .unwrap();
    assert!(shutdown.ends_with('Z'));
    let c = fake.object("claim", "cua-e2e-claims", "my-claim").unwrap();
    assert_eq!(c["spec"]["lifecycle"]["shutdownTime"], shutdown);

    // TTL + explicit spec is rejected, as in cua-sandbox.
    let err = fleet
        .claim(
            &handle.pool,
            ClaimOptions {
                spec: Some(claim.spec.clone()),
                ttl_seconds_after_created: Some(60),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidArgument(_)));

    fleet.release("cua-e2e-claims", "my-claim").await.unwrap();
    fleet.release("cua-e2e-claims", "my-claim").await.unwrap(); // idempotent
    assert!(!fake.exists("claim", "cua-e2e-claims", "my-claim"));
}

#[tokio::test]
async fn acquire_ttl_claim_and_service_request() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let handle = fleet
        .apply_pool(
            &PoolSpec::new("cua-e2e-svc", IMAGE).services([("server", 8000u16), ("env", 3211)]),
        )
        .await
        .unwrap();
    let bound = fleet
        .acquire(
            &handle.pool,
            ClaimOptions {
                ttl_seconds_after_created: Some(120),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let claim = fake.object("claim", "cua-e2e-svc", &bound.claim).unwrap();
    assert_eq!(claim["spec"]["ttlSecondsAfterCreated"], 120);
    assert_eq!(
        fleet.service_url(&bound, "server").unwrap(),
        format!(
            "https://fleet.test/api/svc/cua-e2e-svc/{}-server",
            bound.name
        )
    );
    let r = fleet
        .service_request(
            &bound,
            "server",
            "/status",
            "GET",
            None,
            Some(Duration::from_secs(5)),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let svc_req = fake
        .requests()
        .into_iter()
        .find(|r| r.path.starts_with("/api/svc/"))
        .unwrap();
    assert_eq!(
        svc_req.header("x-cua-fleet-claim"),
        Some(bound.claim.as_str())
    );
    assert_eq!(
        svc_req.header("authorization"),
        Some("Bearer fake-fleet-token")
    );
    assert!(matches!(
        fleet.service_url(&bound, "vnc").unwrap_err(),
        Error::UnknownService { .. }
    ));
}

#[tokio::test]
async fn missing_credentials() {
    let err =
        cua_fleet::FleetClient::connect(cua_fleet::FleetConfig::from_lookup(|_| None)).unwrap_err();
    assert!(matches!(err, Error::MissingCredentials));
}

/// The env channel factory: gRPC-Web through a (mock) Fleet gateway with the
/// Fleet bearer and claim header, the env token in its own header.
#[tokio::test]
async fn env_channel_through_gateway() {
    let fake = FakeFleet::new();
    // Claim names are generated, so bind first against the fake, then start
    // the gateway emulation for the resulting sandbox.
    let fleet = fake.client();
    let handle = fleet
        .apply_pool(&PoolSpec::new("cua-e2e-env", IMAGE))
        .await
        .unwrap();
    let bound = fleet
        .acquire(
            &handle.pool,
            ClaimOptions {
                name: Some("env-claim".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let srv = MockServer::start(MockAuth {
        token: Some("env-secret".into()),
        gateway: Some(MockGateway {
            prefix: format!("/api/svc/cua-e2e-env/{}-env", bound.name),
            bearer: "fake-fleet-token".into(),
            claim: "env-claim".into(),
        }),
        prefix: None,
    })
    .await;
    let via_gateway = fake.client_with_base(&srv.url());
    let env = via_gateway
        .spacesd(&bound, "env", Some("env-secret".into()))
        .await
        .unwrap();
    assert_eq!(env.transport(), cua_spacesd_client::Transport::GrpcWeb);
    assert_eq!(env.run("echo fleet").await.unwrap().stdout_str(), "fleet\n");
}

#[tokio::test]
async fn a_caller_token_provider_supplies_the_bearer() {
    use cua_fleet::sdk::{AccessTokenProvider, AccessTokenProviderError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Rotating(AtomicU32);
    #[async_trait::async_trait]
    impl AccessTokenProvider for Rotating {
        async fn get_access_token(
            &self,
            _force: bool,
        ) -> std::result::Result<String, AccessTokenProviderError> {
            Ok(format!("user-{}", self.0.fetch_add(1, Ordering::SeqCst)))
        }
    }

    let fake = FakeFleet::new();
    let config = cua_fleet::FleetConfig {
        base_url: "http://fleet.invalid".into(),
        ..Default::default()
    };
    let fleet = cua_fleet::FleetClient::connect_with_token_provider(
        config,
        Arc::new(Rotating(AtomicU32::new(0))),
        Some(Arc::new(fake.clone())),
    )
    .unwrap();
    assert_eq!(fleet.access_token(false).await.unwrap(), "user-0");
    fleet
        .apply_pool(&PoolSpec::new("cua-e2e-user", IMAGE))
        .await
        .unwrap();
    assert!(fake.requests().iter().all(|r| {
        r.header("authorization")
            .is_some_and(|h| h.starts_with("Bearer user-"))
    }));
}

#[tokio::test]
async fn cloud_names_are_unique_across_the_accounts_pools() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let a = fleet
        .apply_pool(&PoolSpec::new("cua-e2e-uniq-a", IMAGE))
        .await
        .unwrap();
    let b = fleet
        .apply_pool(&PoolSpec::new("cua-e2e-uniq-b", IMAGE))
        .await
        .unwrap();
    let named = |n: &str| ClaimOptions {
        name: Some(n.into()),
        ..Default::default()
    };
    let (_, created) = fleet.claim(&a.pool, named("box")).await.unwrap();
    assert!(created);
    // The same pool reattaches (idempotent create) ...
    let (_, created) = fleet.claim(&a.pool, named("box")).await.unwrap();
    assert!(!created);
    // ... another pool refuses the name.
    let err = fleet.claim(&b.pool, named("box")).await.unwrap_err();
    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    assert!(err.to_string().contains("cloud:box"), "{err}");
    assert!(
        fleet
            .list_claims("cua-e2e-uniq-b")
            .await
            .unwrap()
            .is_empty()
    );
    // Other names, and unnamed claims, are fine.
    fleet.claim(&b.pool, named("box-2")).await.unwrap();
    fleet.claim(&b.pool, ClaimOptions::default()).await.unwrap();

    let found = fleet.find_claims("box").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].metadata.namespace, "cua-e2e-uniq-a");
    assert!(fleet.find_claims("nope").await.unwrap().is_empty());
}

#[tokio::test]
async fn an_existing_macos_pool_reads_but_does_not_claim() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let handle = fleet
        .apply_pool(&PoolSpec::new("cua-e2e-mac-pool", IMAGE))
        .await
        .unwrap();
    // A pool created elsewhere with the `macos` runtime.
    let template = handle.pool.spec.sandbox_template_ref.name.clone();
    fake.update_object("template", "cua-e2e-mac-pool", &template, |t| {
        t["spec"]["vmTemplate"]["runtime"] = serde_json::json!("macos");
    });
    // Reading still works, and reports the runtime.
    let (_, runtime, _) = fleet.pool_template("cua-e2e-mac-pool").await.unwrap();
    assert_eq!(runtime, cua_fleet::RuntimeKind::Macos);
    assert!(fleet.get_pool("cua-e2e-mac-pool").await.is_ok());
    // Claiming is refused before any claim exists.
    let e = fleet
        .claim(&handle.pool, ClaimOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Unsupported(_)), "{e}");
    assert!(
        fleet
            .list_claims("cua-e2e-mac-pool")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn fake_lists_and_revokes_signed_service_urls() {
    use cyclops_sdk::{HttpClient, HttpRequest};
    let fake = FakeFleet::new();
    let call = |method: &str, path: &str, body: Option<serde_json::Value>| HttpRequest {
        method: method.into(),
        url: format!("http://fleet.test{path}"),
        headers: vec![],
        body: body.map(|b| serde_json::to_vec(&b).unwrap()),
        timeout_secs: None,
        max_response_bytes: None,
    };
    for claim in ["a", "a", "b"] {
        let r = fake
            .execute(call(
                "POST",
                "/api/signed-service-urls/ns",
                Some(serde_json::json!({"claim": claim, "sandbox": "s", "service": "web"})),
            ))
            .await
            .unwrap();
        assert_eq!(r.status, 201);
    }
    let list = |fake: FakeFleet, q: &'static str| async move {
        let r = fake.execute(call("GET", q, None)).await.unwrap();
        (
            r.status,
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap_or_default(),
        )
    };
    let (status, a) = list(fake.clone(), "/api/signed-service-urls/ns?claim=a").await;
    assert_eq!(status, 200);
    assert_eq!(a.as_array().unwrap().len(), 2);
    let id = a[0]["id"].as_str().unwrap().to_string();
    let r = fake
        .execute(call(
            "DELETE",
            &format!("/api/signed-service-urls/ns/{id}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status, 204);
    let (_, a) = list(fake.clone(), "/api/signed-service-urls/ns?claim=a").await;
    assert!(a[0]["revokedAt"].is_string());
    assert_eq!(
        list(fake.clone(), "/api/signed-service-urls/ns").await.0,
        400
    );
}
