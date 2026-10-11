//! The spacesd token handoff of a cloud (Fleet managed pool) sandbox.
//!
//! A Spaces image mints a random token at boot unless Fleet mounts the
//! claim's Secret at `/run/cua` (await-token-file mode); the published
//! `ghcr.io/trycua/linux:24.04` (pre-rename cua-guestd, `ai.cua.guestd`
//! label only) behaves the same. So the SDK must opt the template into
//! claim Secrets and deliver the token it will present with the claim;
//! otherwise every call fails with `missing or invalid bearer token`.
//!
//! Fakes only: FakeFleet, an in-memory registry and a mock spacesd behind
//! an emulated Fleet gateway that accepts only the claim Secret's token.
//! Its own binary: it installs a process-wide registry source.

use base64::Engine as _;
use cua_fleet::autopool::{auto_pool_name, tenant_from_token};
use cua_fleet::testing::FakeFleet;
use cua_fleet::{BoundSandbox, ClaimSecretsWait, FleetClient, TokenProbe, TokenState};
use cua_image::testing::FakeRegistry;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};
use cua_spacesd_client::testing::{MockAuth, MockGateway, MockServer};
use serde_json::json;
use std::{sync::Arc, time::Duration};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04";
/// The bearer of `FakeFleet::client_with_base`.
const FLEET_BEARER: &str = "fake-fleet-token";

/// The in-guest side: the driver has the token exactly when the claim's
/// Secret holds it (what cua-spacesd's await-token-file mode reads).
struct GuestReadsClaimSecret(FakeFleet);

#[async_trait::async_trait]
impl TokenProbe for GuestReadsClaimSecret {
    async fn probe(&self, _: &FleetClient, sb: &BoundSandbox, token: &str) -> TokenState {
        let held = self
            .0
            .object("secret", &sb.namespace, &format!("cua-claim-{}", sb.claim))
            .and_then(|s| s["data"]["env-token"].as_str().map(str::to_string))
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .map(|b| String::from_utf8(b).unwrap());
        match held {
            Some(t) if t == token => TokenState::Delivered,
            // No mount: the image minted a token of its own.
            _ => TokenState::Awaiting("missing or invalid bearer token".into()),
        }
    }
}

fn install_registry(labels: serde_json::Value) {
    let mut r = FakeRegistry::default();
    r.index(IMAGE, &["amd64", "arm64"], false, Some(labels));
    cua_image::resolve::set_source(Some(Arc::new(r)));
}

fn manager(fake: &FakeFleet, base: &str, dir: &std::path::Path) -> Sandboxes {
    // After FakeFleet::new (which installs the fixtures' inspector): resolve
    // through the in-memory registry.
    cua_fleet::set_image_inspector(None);
    let fleet = fake
        .client_with_base(base)
        .with_claim_secrets_wait(ClaimSecretsWait {
            budget: Duration::from_millis(200),
            every: Duration::from_millis(20),
            probe: Arc::new(GuestReadsClaimSecret(fake.clone())),
        });
    Sandboxes::builder()
        .fleet(fleet)
        .state_dir(dir.join("sandboxes"))
        .build()
}

fn options(name: Option<&str>, token: Option<&str>) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE);
    if let Some(n) = name {
        o = o.name(n);
    }
    if let Some(t) = token {
        o.env.insert("CUA_ENV_TOKEN".into(), t.into());
    }
    o
}

/// The managed pool `o` claims from, once its template opts into claim
/// Secrets.
async fn claim_secrets_pool(o: &CreateOptions) -> String {
    let (mut key, _) = o.fleet_pool_key_resolved().await.unwrap();
    key.claim_secrets = true;
    auto_pool_name(&tenant_from_token(FLEET_BEARER), &key.spec_hash())
}

#[tokio::test]
async fn cloud_sandboxes_deliver_their_spacesd_token_with_the_claim() {
    // The published image carries only the pre-rename labels; the
    // upcoming one `ai.cua.spacesd`. Both run the same token scripts.
    for (case, labels) in [
        (
            "published",
            json!({"ai.cua.guestd": "true", "ai.cua.env-driver": "true"}),
        ),
        ("upcoming", json!({"ai.cua.spacesd": "true"})),
    ] {
        install_registry(labels);
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeFleet::new();
        let name = format!("cua-e2e-tok-{case}");
        let token = format!("caller-token-{case}-0123456789");
        let o = options(Some(&name), Some(&token));
        cua_fleet::set_image_inspector(None);
        let pool = claim_secrets_pool(&o).await;
        // The guest's spacesd behind the gateway: only the claim token
        // opens it.
        let guest = MockServer::start(MockAuth {
            token: Some(token.clone()),
            gateway: Some(MockGateway {
                prefix: format!("/api/svc/{pool}/sbx-{name}-env"),
                bearer: FLEET_BEARER.into(),
                claim: name.clone(),
            }),
            prefix: None,
        })
        .await;
        let sbx = manager(&fake, &guest.url(), dir.path());
        let sb = sbx
            .create(o)
            .await
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(
            sb.image_info().and_then(|i| i.spacesd),
            Some(true),
            "{case}"
        );
        let bound = sb.fleet_sandbox().unwrap().clone();
        assert_eq!(bound.namespace, pool, "{case}");

        // The template opted in; the claim names its Secret.
        let t = fake.object("template", &pool, &pool).unwrap();
        assert_eq!(t["spec"]["vmTemplate"]["claimSecrets"], true, "{case}");
        let claim = fake.object("claim", &pool, &bound.claim).unwrap();
        assert_eq!(
            claim["spec"]["secretRef"]["name"],
            format!("cua-claim-{}", bound.claim),
            "{case}"
        );
        // The token is only in the Secret: never in the template, pool or
        // claim (a pool's template is shared by every claim).
        for r in fake.requests() {
            let body = r.body.map(|b| b.to_string()).unwrap_or_default();
            if !r.path.ends_with("/secrets") {
                assert!(!body.contains(&token), "{case}: {} {}", r.method, r.path);
            }
        }

        // An authenticated call through the gateway succeeds.
        assert_eq!(sb.env_token().as_deref(), Some(token.as_str()), "{case}");
        let env = sb.spacesd().await.unwrap_or_else(|e| panic!("{case}: {e}"));
        env.health().await.unwrap_or_else(|e| panic!("{case}: {e}"));

        // A reattach (`cua sb exec NAME`, another process) presents it too.
        let again = sbx.connect(&name).await.unwrap();
        assert_eq!(again.env_token().as_deref(), Some(token.as_str()), "{case}");
        again.spacesd().await.unwrap().health().await.unwrap();
        drop(again);

        // Deleting releases the claim and its Secret.
        sb.delete().await.unwrap();
        assert!(!fake.exists("claim", &pool, &bound.claim), "{case}");
        assert!(
            !fake.exists("secret", &pool, &format!("cua-claim-{}", bound.claim)),
            "{case}"
        );
    }

    // One test: the registry source is process-wide.
    generated_bad_and_plain().await;
    cua_image::resolve::set_source(None);
}

async fn generated_bad_and_plain() {
    install_registry(json!({"ai.cua.guestd": "true"}));
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let sbx = manager(&fake, "https://fleet.test", dir.path());

    // A token cua-spacesd would refuse is refused before any request.
    let before = fake.requests().len();
    assert!(sbx.create(options(None, Some("short"))).await.is_err());
    assert_eq!(fake.requests().len(), before);

    // An image without cua-spacesd: no claim Secrets, no token.
    install_registry(json!({"ai.cua.spacesd": "false"}));
    let plain = sbx.create(options(None, None)).await.unwrap();
    assert_eq!(plain.env_token(), None);
    let b = plain.fleet_sandbox().unwrap().clone();
    let t = fake.object("template", &b.namespace, &b.namespace).unwrap();
    assert_ne!(t["spec"]["vmTemplate"]["claimSecrets"], true);
    let claim = fake.object("claim", &b.namespace, &b.claim).unwrap();
    assert!(claim["spec"].get("secretRef").is_none());
    plain.delete().await.unwrap();
}
