// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The typed control-plane client (folded from `cua-spaces-core`) against
//! the Rust MCP server, in-process: what a Swift / TypeScript / Tauri
//! consumer that speaks MCP to `cua daemon mcp` would decode.
//!
//! The scripted `cua.control.session/1` fixture is checked by the unit tests
//! and `cua-spaces-control-fixtures --check`; this test proves the *live*
//! server's answers decode through the same client.

mod common;

use common::{TOKEN, driver, spaces};
use cua_spaces::client::model::SpaceProvider;
use cua_spaces::client::{Connection, InProcessTransport};
use cua_spaces::mcp::McpServer;
use cua_spaces_e2e::contract_tool_names;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_typed_client_decodes_the_rust_server() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let transport = Arc::new(InProcessTransport::new(
        McpServer::new(s.clone()),
        tokio::runtime::Handle::current(),
    ));
    let url = d.url.clone();
    let connection = Arc::new(Connection::new(transport.clone()));

    // The whole client is synchronous; run it off the async workers.
    let c = connection.clone();
    let info = tokio::task::spawn_blocking(move || c.add_space(&url, Some(TOKEN), Some("typed")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.provider, SpaceProvider::Direct);
    assert!(info.is_ready());
    s.space(&info.id)
        .await
        .map(|sp| async move { d.confine(&sp).await })
        .unwrap()
        .await;

    let c = connection.clone();
    let id = info.id.clone();
    tokio::task::spawn_blocking(move || {
        let listed = c.spaces().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        let tools = c.available_tools().unwrap();
        assert_eq!(tools, contract_tool_names());

        let space = c.attach(&id, true).unwrap();
        let out = space.bash("echo typed-client").unwrap();
        assert!(out.contains("typed-client"), "{out}");
        assert!(out.contains("[exit 0]"), "{out}");

        let services = space.services().unwrap();
        assert!(!services.is_empty(), "{services:?}");
        let parts = space
            .call_service_tool("get_screen_size", None, "{}")
            .unwrap();
        assert!(format!("{parts:?}").contains("1280x800"), "{parts:?}");

        let caps = space.harness_capabilities().unwrap();
        assert!(!caps.harnesses.is_empty());

        // A tool error surfaces as an error, never as a value (§3).
        let e = space.stream_endpoint(true).unwrap_err();
        assert_eq!(e.tag(), "ToolFailed", "{e}");
        assert!(e.to_string().contains("desktop_stream"), "{e}");

        c.remove_space(&id).unwrap();
        assert!(c.spaces().unwrap().is_empty());
    })
    .await
    .unwrap();
}
