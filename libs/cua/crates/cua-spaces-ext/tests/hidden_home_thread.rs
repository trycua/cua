// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The home that warms in the background for Home's prompt box is hidden
//! (label `cua.hidden`) until a person writes into its thread: the first
//! message from a person makes it visible, and nobody else does. Against the
//! in-process spacesd mock and a temporary cua home.

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

struct Fx {
    _srv: MockServer,
    _home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
}

async fn setup(hidden: bool) -> Fx {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let info = spaces
        .add(
            &format!("http://{}", srv.addr),
            None,
            Some("home-claude-code".into()),
        )
        .await
        .unwrap();
    if hidden {
        spaces
            .registry()
            .update_meta(&info.id, |m| {
                m.labels.insert("cua.hidden".into(), "home".into());
                m.labels.insert("cua.relay".into(), "off".into());
            })
            .unwrap();
    }
    let mcp = McpServer::new(spaces.clone());
    Fx {
        _srv: srv,
        _home: home,
        spaces,
        space: info.id,
        mcp,
    }
}

async fn call(fx: &Fx, tool: &str, args: Value) -> Value {
    let out = Caller::Owner.scope(Box::pin(fx.mcp.call(tool, args))).await;
    cua_spaces::extension::outcome_json(out).unwrap_or_else(|e| panic!("{tool}: {e}"))
}

fn listed(fx: &Fx) -> cua_spaces::SpaceInfo {
    fx.spaces.list().unwrap().into_iter().next().unwrap()
}

#[tokio::test]
async fn the_first_message_from_a_person_makes_a_hidden_home_visible() {
    let fx = setup(true).await;
    assert!(listed(&fx).hidden);
    // The tools list it, marked hidden, while it warms.
    let rows = call(&fx, "list_spaces", json!({})).await;
    assert_eq!(rows[0]["hidden"], true, "{rows}");
    // Reading its thread is not writing into it.
    call(&fx, "thread_read", json!({"space": fx.space})).await;
    assert!(listed(&fx).hidden);

    call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "fix the build"}),
    )
    .await;
    let info = listed(&fx);
    assert!(!info.hidden, "{info:?}");
    assert!(!info.labels.contains_key("cua.hidden"));
    // (This Space was added by address, so it has no relay row to turn on;
    // a local one is listed on the account by `reveal`: see relay_publish.)
    let rows = call(&fx, "list_spaces", json!({})).await;
    assert!(rows[0].get("hidden").is_none(), "{rows}");
}

#[tokio::test]
async fn a_home_the_app_did_not_prepare_is_never_hidden() {
    let fx = setup(false).await;
    assert!(!listed(&fx).hidden);
    call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "hello"}),
    )
    .await;
    assert!(!listed(&fx).hidden);
}
