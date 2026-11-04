// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's daemon wire is the experiment `mobile`: while it is off the
//! phone's calls and its extras on the thread tools are refused in the
//! experiment's own words, the thread tools behave as they always did, and no
//! event is reported for an alert.

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const OFF: &str = "Phone app is an experiment that is off. Turn it on with `cua experiments enable mobile` (or in the app: Settings, Advanced, Labs).";

async fn setup() -> (MockServer, tempfile::TempDir, McpServer, String) {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let info = spaces
        .add(&format!("http://{}", srv.addr), None, Some("room".into()))
        .await
        .unwrap();
    (srv, home, McpServer::new(spaces), info.id)
}

async fn call(mcp: &McpServer, tool: &str, args: Value) -> Result<Value, String> {
    let owner = Caller::Owner;
    let out = owner.scope(Box::pin(mcp.call(tool, args))).await;
    cua_spaces::extension::outcome_json(out).map_err(|e| e.to_string())
}

#[tokio::test]
async fn every_phone_call_is_refused_while_the_experiment_is_off() {
    let _off = cua_spaces::experiments::hold(cua_spaces::experiments::Gate::Mobile, false);
    let (_srv, _home, mcp, space) = setup().await;
    for tool in cua_spaces_ext::persistent::phone::TOOLS {
        let err = call(
            &mcp,
            tool,
            json!({"space": space, "client_id": "0f8e6c1a-1b2c-4d3e-8f90-123456789abc"}),
        )
        .await
        .expect_err(tool);
        assert!(err.contains(OFF), "{tool}: {err}");
    }
}

#[tokio::test]
async fn the_phones_extras_on_the_thread_tools_are_refused_and_the_thread_is_unchanged() {
    let _off = cua_spaces::experiments::hold(cua_spaces::experiments::Gate::Mobile, false);
    let (_srv, _home, mcp, space) = setup().await;
    // A plain post and a plain read work as they always did.
    call(
        &mcp,
        "thread_post",
        json!({"space": space, "text": "hello"}),
    )
    .await
    .expect("a plain post");
    let read = call(&mcp, "thread_read", json!({"space": space}))
        .await
        .expect("a plain read");
    assert!(read.to_string().contains("hello"));
    // A retry key and a standing answer's scope are the phone's.
    for extra in [
        json!({"space": space, "text": "again", "client_id": "0f8e6c1a-1b2c-4d3e-8f90-123456789abc"}),
        json!({"space": space, "text": "ok", "answer": {"approval": 1, "approve": true, "scope": "space"}}),
        json!({"space": space, "text": "ok", "answer": {"approval": 1, "approve": true, "client_id": "0f8e6c1a-1b2c-4d3e-8f90-123456789abc"}}),
    ] {
        let err = call(&mcp, "thread_post", extra.clone())
            .await
            .expect_err("refused");
        assert!(err.contains(OFF), "{extra}: {err}");
    }
    // Nothing the refused calls carried was written.
    let read = call(&mcp, "thread_read", json!({"space": space}))
        .await
        .unwrap();
    assert!(!read.to_string().contains("again"));
}

#[test]
fn no_event_is_reported_for_an_alert_while_the_experiment_is_off() {
    let gate = cua_spaces::experiments::hold(cua_spaces::experiments::Gate::Mobile, false);
    struct Count(std::sync::atomic::AtomicUsize);
    impl cua_spaces_ext::push_client::Transport for Count {
        fn send(
            &self,
            _e: cua_push::PushEvent,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {})
        }
    }
    let count = std::sync::Arc::new(Count(Default::default()));
    cua_spaces_ext::push_client::install(count.clone());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        cua_spaces_ext::push_client::report(cua_push::PushEvent {
            id: "approval:room:1".into(),
            kind: cua_push::EventKind::ApprovalNeeded,
            space: Some("relay:m".into()),
            entry: Some(1),
            agent: Some("Claude Code".into()),
            scope_choices: vec![],
            at_ms: 1,
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });
    assert_eq!(count.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    // On: it is sent.
    gate.set(true);
    rt.block_on(async {
        cua_spaces_ext::push_client::report(cua_push::PushEvent {
            id: "approval:room:2".into(),
            kind: cua_push::EventKind::ApprovalNeeded,
            space: Some("relay:m".into()),
            entry: Some(2),
            agent: None,
            scope_choices: vec![],
            at_ms: 2,
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });
    assert_eq!(count.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}
