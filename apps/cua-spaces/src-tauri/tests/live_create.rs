// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live run of the New Space wizard's create path: the plan the wizard builds
//! for "Ubuntu 24.04 on This Mac, 2 cores, 4 GB" goes through the same
//! `create_space` command core the webview calls, starts a real container
//! under the local runtime, answers as a Space, and is deleted.
//!
//! Opt-in: set `CUA_SPACES_APP_E2E_CREATE=1` (and `DOCKER_HOST` when the
//! engine is not the default socket). The container is named `cua-e2e-*`,
//! capped by the local runtime's memory setting, and deleted at the end even
//! when an assertion fails. Temp `CUA_HOME`; nothing reads the user profile.

use cua_spaces_lib::core::{AppCore, CoreConfig, SpaceCreateConfig};
use std::sync::Arc;
use std::time::Duration;

/// What the wizard sends for `ghcr.io/trycua/linux:24.04` on This Mac
/// (`createFromPlan` in src/state/createSpace.ts: the plain ref, the kind
/// from the image list and the runtime the person chose, `auto` here).
const WIZARD_IMAGE: &str = "ghcr.io/trycua/linux:24.04";

fn wizard_plan(name: String) -> SpaceCreateConfig {
    SpaceCreateConfig {
        on: Some("local".into()),
        kind: Some("container".into()),
        runtime: Some("auto".into()),
        image: Some(WIZARD_IMAGE.into()),
        name: Some(name),
        cpus: Some(2),
        memory_mb: Some(4096),
        disk_gb: None,
        spacesd: Some(true),
        reuse: None,
        gpu: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wizard_plan_creates_a_real_local_space() {
    if std::env::var("CUA_SPACES_APP_E2E_CREATE").ok().as_deref() != Some("1") {
        eprintln!("skipped: set CUA_SPACES_APP_E2E_CREATE=1 to start a real container");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::hermetic(home.path());
    cfg.local_runtime = Some(Arc::new(cua_daemon::local::VmmLocal::default()));
    let core = AppCore::new(cfg);
    let name = format!("cua-e2e-wizard-{}", std::process::id());
    let started = std::time::Instant::now();

    let created = tokio::time::timeout(
        Duration::from_secs(900),
        core.create_space(wizard_plan(name.clone())),
    )
    .await;

    eprintln!("create returned after {:?}", started.elapsed());
    // Whatever happened, try to remove what we started.
    let id = match &created {
        Ok(Ok(row)) => Some(row.id.clone()),
        _ => None,
    };
    let listed = core.list_spaces().await.unwrap_or_default();
    let deleted = match &id {
        Some(id) => Some(core.delete_space(id).await),
        None => None,
    };

    let row = created
        .expect("the create timed out")
        .expect("the Space starts");
    eprintln!("created {row:?}");
    assert!(row.reachable, "the new Space answers: {row:?}");
    assert!(
        !row.spacesd_version.is_empty(),
        "cua-spacesd answered: {row:?}"
    );
    assert_eq!(row.provider.as_str(), "local");
    assert!(
        listed.iter().any(|r| r.id == row.id),
        "the registry lists it"
    );
    deleted.expect("deleted").expect("delete succeeds");
    let after = core.list_spaces().await.unwrap_or_default();
    assert!(after.iter().all(|r| r.id != row.id), "delete removes it");
}

/// Capture helper, opt-in with `CUA_SPACES_APP_E2E_KEEP_HOME=<dir>` (and
/// `CUA_HOME=<same dir>`): runs the same wizard plan against that home and
/// leaves the Space registered and running, so the app launched with that
/// `CUA_HOME` shows it. Remove it afterwards from the app (Delete Space) or
/// with `docker rm -f cua-e2e-wizard-keep`.
///
/// Optional: `CUA_SPACES_APP_E2E_KEEP_IMAGE` replaces the wizard's image (a
/// local candidate build, say); `CUA_SPACES_APP_E2E_KEEP_ON=cloud` sends the
/// wizard's Cua Cloud plan instead (Fleet credentials from the environment,
/// namespace `cua-e2e-app`, so launch the app with
/// `CUA_SPACES_NAMESPACE=cua-e2e-app`); `CUA_SPACES_APP_E2E_KEEP_NAME` must
/// start with `cua-e2e-`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wizard_plan_creates_a_space_kept_for_capture() {
    let Some(dir) = std::env::var("CUA_SPACES_APP_E2E_KEEP_HOME")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("skipped: set CUA_SPACES_APP_E2E_KEEP_HOME to keep a Space for captures");
        return;
    };
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let name = env("CUA_SPACES_APP_E2E_KEEP_NAME").unwrap_or_else(|| "cua-e2e-wizard-keep".into());
    assert!(
        name.starts_with("cua-e2e-"),
        "capture Spaces are named cua-e2e-*"
    );
    let cloud = env("CUA_SPACES_APP_E2E_KEEP_ON").as_deref() == Some("cloud");
    let home = std::path::PathBuf::from(dir);
    let mut cfg = CoreConfig::hermetic(&home);
    cfg.state_dir = None;
    let mut plan = wizard_plan(name);
    if let Some(image) = env("CUA_SPACES_APP_E2E_KEEP_IMAGE") {
        plan.image = Some(image);
    }
    if cloud {
        // What the wizard sends for Cua Cloud: no cores or memory.
        cfg.fleet = cua_fleet::FleetConfig::from_env();
        assert!(cfg.fleet.has_auth(), "Cua Cloud needs Fleet credentials");
        plan.on = Some("cloud".into());
        plan.cpus = None;
        plan.memory_mb = None;
    } else {
        cfg.local_runtime = Some(Arc::new(cua_daemon::local::VmmLocal::default()));
    }
    let core = AppCore::new(cfg);
    let row = tokio::time::timeout(Duration::from_secs(900), core.create_space(plan))
        .await
        .expect("the create timed out")
        .expect("the Space starts");
    eprintln!("kept {} ({})", row.id, row.spacesd_version);
}
