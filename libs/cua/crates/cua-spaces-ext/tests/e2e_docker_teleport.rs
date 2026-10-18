// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live e2e of session teleport against a real linux container (the real
//! spacesd). Run through `libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh`
//! (it exports `CUA_SPACES_E2E_URL` and `CUA_SPACES_E2E_TOKEN`); without
//! them the test is skipped. Host safety: the Firefox profile is generated
//! under a temp home and the teleport sender runs on a `FakeHost`.

use cua_spaces::{Space, Spaces};
use cua_spaces_ext::teleport::SpaceTeleport as _;
use std::sync::Arc;
use std::time::Duration;

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let token = std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default();
    Some((url, token))
}

macro_rules! require_target {
    () => {
        match target() {
            Some(t) => t,
            None => {
                eprintln!("skipped: set CUA_SPACES_E2E_URL (run libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh)");
                return;
            }
        }
    };
}

async fn connect(reg: &std::path::Path) -> (Spaces, Space) {
    let (url, token) = target().unwrap();
    let spaces = Spaces::builder()
        .home(reg)
        .download_dir(reg.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("e2e".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    (spaces, space)
}

async fn sh(space: &Space, cmd: &str) -> String {
    let out = space.bash(cmd, Duration::from_secs(120)).await.unwrap();
    assert!(out.success(), "{cmd}: {}", out.render());
    out.stdout
}

#[tokio::test]
async fn e2e_teleport_a_synthetic_firefox_profile() {
    use cua_spaces_ext::teleport::providers::{ExportRegistry, FakeHost, FirefoxProvider};
    use cua_spaces_ext::teleport::{AppSessions, ImportOptions, TeleportScope};
    require_target!();
    let reg = tempfile::tempdir().unwrap();
    let (_spaces, space) = connect(reg.path()).await;
    assert!(space.supports("teleport.firefox"));

    let host_home = tempfile::tempdir().unwrap();
    let profile = host_home.path().join("synthetic-profile");
    std::fs::create_dir_all(&profile).unwrap();
    let marker = format!("cua-e2e-{:08x}", rand_u32());
    std::fs::write(
        profile.join("prefs.js"),
        format!("user_pref(\"cua.e2e.marker\", \"{marker}\");\n"),
    )
    .unwrap();
    std::fs::write(profile.join("places.sqlite"), b"synthetic history").unwrap();
    std::fs::write(profile.join("cookies.sqlite"), b"synthetic cookies").unwrap();
    let host = Arc::new(FakeHost::new().with_home(host_home.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        FirefoxProvider::new()
            .with_host(host)
            .with_profile_dir(&profile),
    ));
    let sessions = Arc::new(AppSessions::from_registry(registry));

    let manifest = sessions.manifest("firefox", TeleportScope::Full).unwrap();
    let approval = manifest
        .approving_default(&space.id().to_string(), true)
        .unwrap();
    let receipt = space
        .teleport(sessions, &approval, ImportOptions::default())
        .await
        .unwrap();
    assert!(!receipt.imported.is_empty(), "{receipt:?}");
    let found = sh(
        &space,
        &format!("grep -rl '{marker}' ~/.mozilla ~/snap 2>/dev/null | head -1"),
    )
    .await;
    assert!(
        found.contains("prefs.js"),
        "profile landed in the guest: {found:?}"
    );
}

fn rand_u32() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}
