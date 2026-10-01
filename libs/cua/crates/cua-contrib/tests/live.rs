//! Live tests against the real providers. Opt-in: `CUA_E2E_CONTRIB_LIVE=1`
//! plus the provider's key (`E2B_API_KEY`, `DAYTONA_API_KEY`, or
//! `MODAL_TOKEN_ID` + `MODAL_TOKEN_SECRET` and `cua-modal-helper`); every
//! other run skips with the reason. Each test creates one sandbox named
//! `cua-e2e-<provider>-<hex>` from the canonical Linux image, checks that
//! cua-spacesd answers over the provider's URL, and deletes it even when a
//! check fails (the provider's lifetime backstop, 30 minutes, covers a
//! killed run).
//!
//! ```text
//! CUA_E2E_CONTRIB_LIVE=1 E2B_API_KEY=... cargo test -p cua-contrib \
//!   --features all --test live -- --test-threads=1 --nocapture
//! ```

use cua_sandbox_core::{CreateOptions, Provider, ProviderKind, Sandboxes};
use std::{sync::Arc, time::Duration};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04";

fn enabled(provider: &dyn Provider) -> Option<String> {
    if std::env::var("CUA_E2E_CONTRIB_LIVE").as_deref() != Ok("1") {
        return Some("set CUA_E2E_CONTRIB_LIVE=1 for live provider tests".into());
    }
    provider.check_configured().err().map(|e| e.to_string())
}

async fn round_trip(provider: Arc<dyn Provider>) {
    if let Some(why) = enabled(provider.as_ref()) {
        eprintln!("skipped {}: {why}", provider.name());
        return;
    }
    let word = provider.name();
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder()
        .provider(provider.clone())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    let name = format!("cua-e2e-{word}-{:08x}", rand_u32());
    let mut o = CreateOptions::new(ProviderKind::Contrib, IMAGE).name(&name);
    o.contrib = Some(word.into());
    o.fleet.ttl_seconds_after_created = Some(30 * 60);
    o.ready_timeout = Duration::from_secs(30 * 60);
    let created = sbx.create(o).await;
    let result = async {
        let sb = created?;
        let caps = sb.spacesd().await?.capabilities().await?;
        eprintln!(
            "{word}: {} answered cua-spacesd {} ({:?})",
            sb.id(),
            caps.version,
            sb.provider_details()
        );
        Ok::<_, cua_sandbox_core::Error>(())
    }
    .await;
    // Always delete: by name when the create returned, else whatever the
    // provider lists under this name.
    let _ = sbx.delete(&name).await;
    if let Ok(all) = provider.list().await {
        for i in all.into_iter().filter(|i| i.name == name) {
            let _ = provider.delete(&i.id).await;
        }
    }
    result.unwrap();
}

fn rand_u32() -> u32 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    t.subsec_nanos() ^ std::process::id()
}

#[cfg(feature = "e2b")]
#[tokio::test]
async fn live_e2b() {
    round_trip(Arc::new(cua_contrib::e2b::E2b::from_env())).await;
}

#[cfg(feature = "daytona")]
#[tokio::test]
async fn live_daytona() {
    round_trip(Arc::new(cua_contrib::daytona::Daytona::from_env())).await;
}

#[cfg(feature = "modal")]
#[tokio::test]
async fn live_modal() {
    round_trip(Arc::new(cua_contrib::modal::Modal::from_env())).await;
}
