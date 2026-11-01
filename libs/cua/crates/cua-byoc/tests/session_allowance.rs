// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces in your own cloud count against your plan's live sessions (a
//! Space on a pool host is reached through the relay). A Free-plan account
//! with no session left asks for a Space in its cloud: it is refused with
//! the plan's numbers and the upgrade path, and no cloud machine is bought.
//! With a session left, the same request plans a machine. Against a
//! scripted cloud and a fake relay; nothing leaves the process.

use std::sync::Arc;

use cua_byoc::model::{Connection, Tier};
use cua_byoc::state::Store;
use cua_byoc::testing::FakeProvider;
use cua_byoc::{CloudApi, RelayAccess};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::Sandboxes;
use cua_sandbox_core::placement::On;
use cua_spaces::{SpaceCreate, Spaces};

struct World {
    _dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    fake: Arc<FakeProvider>,
    spaces: Spaces,
}

/// Settings come from a test home, never the user's; this machine takes
/// no Spaces in the pool (the request must go to the cloud).
fn test_env() {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: set once, before any test of this binary reads them.
        unsafe {
            std::env::set_var("CUA_HOME", home.path());
            std::env::set_var("CUA_ELASTIC_THIS_MACHINE", "off");
        }
        home
    });
}

async fn world() -> World {
    test_env();
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("fake", Tier::Vm);
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![fake.clone() as Arc<dyn CloudApi>],
        RelayAccess::new(
            relay.url.clone(),
            Arc::new(cua_host::relay::StaticToken("acct-token".into())),
        ),
    )
    .build();
    Store::new(dir.path())
        .connect(Connection {
            provider: "fake".into(),
            region: "fake-1".into(),
            ..Default::default()
        })
        .unwrap();
    let spaces = Spaces::builder()
        .home(dir.path())
        .sandboxes(sandboxes)
        .relay(cua_spaces::relay::RelayAccount::new(
            relay.url.clone(),
            Arc::new(cua_spaces::relay::StaticToken("acct-token".into())),
        ))
        .build();
    World {
        _dir: dir,
        relay,
        fake,
        spaces,
    }
}

fn free_plan(in_use: u32) -> serde_json::Value {
    serde_json::json!({
        "metered": true,
        "period": "2026-10",
        "plan": "free",
        "live_sessions": in_use,
        "live_sessions_limit": 2,
        "hours_used": 1.0,
        "hours_limit": 20.0,
        "upgrade_url": "https://cua.ai/pricing",
    })
}

fn cloud_space() -> SpaceCreate {
    SpaceCreate {
        image: Some("ghcr.io/trycua/linux:24.04-slim".into()),
        kind: cua_sandbox_core::placement::Kind::Container,
        cpus: Some(1),
        memory_mb: Some(512),
        on: Some(On::Auto),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_free_plan_with_no_session_left_buys_no_cloud_machine() {
    let w = world().await;
    w.relay.set_usage(Some(free_plan(2)));
    let room = w.spaces.cloud_session_room().await.expect("a metered plan");
    assert_eq!((room.left, room.used, room.limit), (0, 2, 2));

    let e = w.spaces.create(cloud_space()).await.unwrap_err();
    assert_eq!(e.tag(), "limit_exceeded", "{e}");
    let m = e.to_string();
    assert!(
        m.contains("Your plan allows 2 live sessions (2 in use)"),
        "{m}"
    );
    assert!(m.contains("needs 1"), "{m}");
    assert!(m.contains("no cloud machine was started"), "{m}");
    assert!(
        m.contains("Upgrade to Pro or Teams (https://cua.ai/pricing)"),
        "{m}"
    );
    // Nothing was created in the cloud account.
    assert!(
        !w.fake.calls().iter().any(|c| c.starts_with("provision")),
        "{:?}",
        w.fake.calls()
    );
}

#[tokio::test]
async fn a_free_plan_with_a_session_left_may_place_in_the_cloud() {
    let w = world().await;
    w.relay.set_usage(Some(free_plan(1)));
    let room = w.spaces.cloud_session_room().await.expect("a metered plan");
    assert_eq!(room.left, 1);
    // An unmetered account (or a relay without usage) is not capped here.
    w.relay
        .set_usage(Some(serde_json::json!({"metered": false})));
    assert!(w.spaces.cloud_session_room().await.is_none());
    w.relay.set_usage(None);
    assert!(w.spaces.cloud_session_room().await.is_none());
}
