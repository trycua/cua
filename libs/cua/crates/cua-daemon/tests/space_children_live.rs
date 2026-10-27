//! Opt-in: a Space makes, uses and deletes child Spaces through its own
//! spawn channel, on a real runtime, and cannot reach past its subtree.
//!
//! The parent runs with children on. From inside it, the way an agent does
//! (its own MCP server, `tools/call`), it creates a child, runs a command in
//! it and lists it; it is refused another top-level Space, by id and by
//! name, and the capability it holds does not open its child's channel.
//! Deleting the parent through the owner ends its capability.
//!
//! ```text
//! CUA_SPACESD_STATIC=/path/to/linux/cua-spacesd \
//! cargo test -p cua-daemon --test space_children_live -- --ignored --nocapture
//! ```
//!
//! Needs a local container runtime (Docker on Linux, or the built-in Linux
//! runtime) and image `CUA_CHILDREN_TEST_IMAGE` (default
//! `ghcr.io/trycua/linux:24.04-slim`; it needs curl). `CUA_SPACESD_STATIC`
//! must be a cua-spacesd with the spawn channel (it is injected into the
//! container). Everything lives under a temp home and is deleted on every
//! path.

use std::time::{Duration, Instant};

use cua_daemon::server::{ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_proto::daemon::v1 as pb;
use cua_proto::daemon::v1::space_service_client::SpaceServiceClient;
use futures_util::FutureExt as _;
use serde_json::{Value, json};

type Client = SpaceServiceClient<
    tonic::service::interceptor::InterceptedService<
        tonic::transport::Channel,
        Box<dyn FnMut(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Send>,
    >,
>;

fn image() -> String {
    std::env::var("CUA_CHILDREN_TEST_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into())
}

async fn bash(c: &mut Client, space: &str, command: &str) -> String {
    let r = c
        .call_space_tool(pb::CallSpaceToolRequest {
            name: "space_bash".into(),
            arguments_json: json!({"space": space, "command": command, "timeout": 120}).to_string(),
        })
        .await
        .expect("space_bash")
        .into_inner();
    let content: Value = serde_json::from_str(&r.content_json).unwrap();
    content[0]["text"].as_str().unwrap_or_default().to_owned()
}

/// A `tools/call` on the Space's own MCP server, from inside it.
fn mcp_call(tool: &str, args: Value) -> String {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": tool, "arguments": args}})
    .to_string();
    format!(
        "T=$(cat /run/cua-env/env-token 2>/dev/null || cat /run/cua/env-token 2>/dev/null || \
         printf %s \"$CUA_ENV_TOKEN\"); curl -s -m 600 -H \"authorization: Bearer $T\" \
         -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' \
         http://127.0.0.1:${{CUA_ENV_PORT:-3211}}/mcp -d '{body}'"
    )
}

async fn create(c: &mut Client, name: &str, children: bool) -> String {
    c.create_space(pb::CreateSpaceRequest {
        image: image(),
        location: "local".into(),
        kind: "container".into(),
        name: name.into(),
        headless: true,
        spacesd: Some(true),
        children: Some(children),
        ..Default::default()
    })
    .await
    .expect("create")
    .into_inner()
    .space
    .expect("space")
    .id
}

#[tokio::test]
#[ignore = "needs a local container runtime and a cua-spacesd with the spawn channel"]
async fn a_space_makes_uses_and_cannot_escape_its_children() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: set before the runtime or any thread reads it.
    unsafe { std::env::set_var("CUA_HOME", home.path()) };
    let runtime = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(home.path().join("sandboxes")),
        spaces_home: Some(home.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();
    let handle = start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some(([127, 0, 0, 1], 0).into()),
            token: "owner".into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();
    let channel = tonic::transport::Endpoint::from_shared(handle.loopback_url.clone().unwrap())
        .unwrap()
        .connect()
        .await
        .unwrap();
    let auth: Box<
        dyn FnMut(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Send,
    > = Box::new(|mut r: tonic::Request<()>| {
        r.metadata_mut()
            .insert("authorization", "Bearer owner".parse().unwrap());
        Ok(r)
    });
    let mut c: Client = SpaceServiceClient::with_interceptor(channel, auth);

    let mut made = Vec::new();
    let result = std::panic::AssertUnwindSafe(async {
        let parent = create(&mut c, "kid-parent", true).await;
        made.push(parent.clone());
        let outsider = create(&mut c, "kid-outsider", false).await;
        made.push(outsider.clone());

        // The host starts the parent's channel.
        let deadline = Instant::now() + Duration::from_secs(90);
        let address = loop {
            let a = bash(&mut c, &parent, "printenv CUA_SPAWN || true").await;
            if a.trim().starts_with("http://127.0.0.1:") {
                break a.trim().to_owned();
            }
            assert!(Instant::now() < deadline, "no spawn channel: {a}");
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        eprintln!("parent channel at {address}");

        // Create a child from inside, as an agent does.
        let out = bash(&mut c, &parent, &mcp_call("create_space", json!({"headless": true, "kind": "container", "image": image()}))).await;
        eprintln!("create_space: {out}");
        let list = c
            .list_spaces(pb::ListSpacesRequest::default())
            .await
            .unwrap()
            .into_inner();
        let child = list
            .spaces
            .iter()
            .find(|s| s.labels.get("cua.parent") == Some(&parent))
            .expect("the child is registered under its parent")
            .id
            .clone();
        made.push(child.clone());

        // Use it, and see only it.
        let out = bash(&mut c, &parent, &mcp_call("space_bash", json!({"space": child, "command": "echo hello-from-child"}))).await;
        assert!(out.contains("hello-from-child"), "{out}");
        let out = bash(&mut c, &parent, &mcp_call("list_spaces", json!({}))).await;
        assert!(out.contains(&child) && !out.contains(&outsider), "{out}");

        // Another top-level Space is not there, by id or by name.
        for target in [outsider.as_str(), "kid-outsider", parent.as_str()] {
            let out = bash(&mut c, &parent, &mcp_call("delete_space", json!({"space": target}))).await;
            assert!(out.contains("isError\":true") && out.contains("no Space"), "{target}: {out}");
        }
        assert!(c
            .resolve_space(pb::ResolveSpaceRequest { space: outsider.clone() })
            .await
            .is_ok());

        // The parent's capability does not open the child's channel.
        let deadline = Instant::now() + Duration::from_secs(90);
        let child_address = loop {
            let a = bash(&mut c, &child, "printenv CUA_SPAWN || true").await;
            if a.trim().starts_with("http://127.0.0.1:") {
                break a.trim().to_owned();
            }
            assert!(Instant::now() < deadline, "no channel in the child: {a}");
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        let parent_cap = bash(&mut c, &parent, "cat \"$CUA_SPAWN_TOKEN_FILE\"").await;
        let probe = |cap: &str| {
            format!(
                "curl -s -o /dev/null -w '%{{http_code}}' -H 'authorization: Bearer {}' {child_address}/v1/spaces/x/env/y",
                cap.trim()
            )
        };
        let code = bash(&mut c, &child, &probe(&parent_cap)).await;
        assert_eq!(code.trim(), "401", "the parent's capability on the child's channel");
        let own = bash(&mut c, &child, "cat \"$CUA_SPAWN_TOKEN_FILE\"").await;
        let code = bash(&mut c, &child, &probe(&own)).await;
        assert_ne!(code.trim(), "401", "the child's own capability");

        // Deleted from inside: gone.
        let out = bash(&mut c, &parent, &mcp_call("delete_space", json!({"space": child}))).await;
        assert!(!out.contains("isError\":true"), "{out}");
        assert!(c
            .resolve_space(pb::ResolveSpaceRequest { space: child.clone() })
            .await
            .is_err());
    })
    .catch_unwind()
    .await;
    for id in made.iter().rev() {
        let _ = c
            .delete_space(pb::DeleteSpaceRequest { space: id.clone() })
            .await;
    }
    handle.shutdown();
    handle.wait().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
