//! Live: a cloud sandbox of the published Spaces image answers cua-spacesd
//! with the token the SDK delivered through the claim's Secret (what
//! `cua sb create linux --on cloud` does). Skipped unless `CUA_E2E_FLEET=1`
//! (and Fleet credentials).
//!
//! Creates a managed-pool claim named `cua-e2e-tok-<rand>`, runs a command
//! and takes a screenshot through cua-spacesd, then deletes it and checks
//! the claim is gone (also when an assertion fails). The managed pool stays
//! warm for reuse until its backstop TTL, like any `--on cloud` create.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 cargo test -p cua-sandbox-core --test fleet_claim_token_live -- --nocapture
//! ```
//!
//! `CUA_E2E_FLEET_TOKEN_IMAGE` overrides the image (default the published
//! `ghcr.io/trycua/linux:24.04`, whose daemon predates the cua-spacesd
//! rename); `CUA_E2E_FLEET_SCREENSHOT=<path>` saves the screenshot.

use cua_fleet::FleetClient;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};
use cua_spacesd_client::{Command, ScreenshotOptions};
use std::time::Duration;

#[tokio::test]
async fn live_cloud_sandbox_authenticates_with_the_claim_token() {
    if std::env::var("CUA_E2E_FLEET").as_deref() != Ok("1") {
        eprintln!("skipping: set CUA_E2E_FLEET=1 (and Fleet credentials)");
        return;
    }
    let image = std::env::var("CUA_E2E_FLEET_TOKEN_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04".into());
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder()
        .fleet(fleet.clone())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let name = format!("cua-e2e-tok-{:08x}", rand::random::<u32>());
    let mut o = CreateOptions::new(ProviderKind::Fleet, &image).name(&name);
    o.ready_timeout = Duration::from_secs(600);
    let started = std::time::Instant::now();
    let sb = tokio::time::timeout(Duration::from_secs(720), sbx.create(o))
        .await
        .expect("create timed out")
        .expect("create");
    let bound = sb.fleet_sandbox().unwrap().clone();
    eprintln!(
        "claimed {} in {} ({:?}); image {:?}",
        bound.claim,
        bound.namespace,
        started.elapsed(),
        sb.image_info().map(|i| &i.pinned_ref)
    );

    let result = async {
        assert!(
            sb.env_token().is_some_and(|t| t.len() == 64),
            "a per-claim token"
        );
        let env = sb.spacesd().await.map_err(|e| e.to_string())?;
        let out = env
            .run(Command::shell(
                "uname -s; ps -eo args | grep -E 'cua-(guestd|spacesd)' | grep -v grep | head -1",
            ))
            .await
            .map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        eprintln!("run: {stdout}");
        assert!(stdout.starts_with("Linux"), "{stdout}");
        let shot = env
            .screenshot(ScreenshotOptions::default())
            .await
            .map_err(|e| e.to_string())?;
        eprintln!(
            "screenshot {}x{} ({} bytes)",
            shot.width,
            shot.height,
            shot.image.len()
        );
        assert!(shot.width > 0 && shot.height > 0 && !shot.image.is_empty());
        if let Ok(path) = std::env::var("CUA_E2E_FLEET_SCREENSHOT") {
            std::fs::write(&path, &shot.image).unwrap();
            eprintln!("wrote {path}");
        }
        Ok::<_, String>(())
    }
    .await;

    let deleted = sb.delete().await;
    let left = fleet
        .sdk()
        .list_claims(bound.namespace.clone())
        .await
        .map(|c| c.iter().any(|c| c.metadata.name == bound.claim));
    result.unwrap();
    deleted.unwrap();
    assert_eq!(left.ok(), Some(false), "the claim was released");
    eprintln!("released claim {}", bound.claim);
}
