//! The agent surface of the MCP server: the small tool list, routing onto
//! the underlying tools, the approval gate and the user-only tools. The
//! backend is a recorder: nothing here reaches a Space, the network or the
//! user's home (the approval policy lives in a temp dir).

use async_trait::async_trait;
use cua_spaces::approvals::{Approver, Cap, MemorySeal, Policy};
use cua_spaces::mcp::gate::Guard;
use cua_spaces::mcp::{McpServer, ToolBackend, ToolExtension, ToolOutcome};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, Value)>>);

#[async_trait]
impl ToolBackend for Recorder {
    async fn call(&self, tool: &str, arguments: Value) -> ToolOutcome {
        self.0.lock().unwrap().push((tool.to_string(), arguments));
        if tool == "list_spaces" {
            return ToolOutcome::json(&json!([
                {"id": "local:dev", "name": "dev"},
                {"id": "relay:mini", "name": "mini"},
            ]));
        }
        ToolOutcome::text("ok")
    }
}

struct Ext;

#[async_trait]
impl ToolExtension for Ext {
    fn tools(&self) -> Vec<Value> {
        [
            "computer_click",
            "computer_screenshot",
            "sandbox_create",
            "skills_list",
        ]
        .iter()
        .map(|n| json!({"name": n, "description": "x", "inputSchema": {"type": "object"}}))
        .collect()
    }
    async fn call(&self, tool: &str, arguments: Value) -> Option<ToolOutcome> {
        Some(ToolOutcome::text(format!("ext {tool} {arguments}")))
    }
}

struct Approvals {
    asked: Mutex<Vec<String>>,
    yes: bool,
}

impl Approver for Approvals {
    fn confirm(&self, reason: &str) -> Result<(), String> {
        self.asked.lock().unwrap().push(reason.to_string());
        if self.yes {
            Ok(())
        } else {
            Err("declined".into())
        }
    }
}

struct Rig {
    server: McpServer,
    backend: Arc<Recorder>,
    approver: Arc<Approvals>,
    home: tempfile::TempDir,
    seal: Arc<MemorySeal>,
}

fn rig(yes: bool) -> Rig {
    let backend = Arc::new(Recorder::default());
    let approver = Arc::new(Approvals {
        asked: Mutex::default(),
        yes,
    });
    let home = tempfile::tempdir().unwrap();
    let seal = Arc::new(MemorySeal::default());
    let guard = Guard::new(home.path().to_path_buf(), approver.clone()).with_seal(seal.clone());
    // These tests are about enforcement, so every capability starts gated
    // (the fresh-install defaults are tested in `approvals`).
    let all: Vec<(Cap, bool)> = Cap::ALL.iter().map(|c| (*c, true)).collect();
    Policy::store_unprompted(home.path(), &all, &*seal).unwrap();
    let server = McpServer::remote(backend.clone())
        .with_extension(Arc::new(Ext))
        .with_agent_surface(guard);
    Rig {
        server,
        backend,
        approver,
        home,
        seal,
    }
}

async fn rpc(server: &McpServer, method: &str, params: Value) -> Value {
    server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .await
        .unwrap()
}

async fn call(r: &Rig, tool: &str, args: Value) -> Value {
    rpc(
        &r.server,
        "tools/call",
        json!({"name": tool, "arguments": args}),
    )
    .await
}

fn is_error(v: &Value) -> bool {
    v["result"]["isError"].as_bool().unwrap_or(false)
}

fn kind(v: &Value) -> String {
    let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    let structured = &v["result"]["structuredContent"]["error"]["kind"];
    structured
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| text.to_string())
}

fn calls(r: &Rig) -> Vec<String> {
    r.backend
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(t, _)| t.clone())
        .collect()
}

fn set_policy(r: &Rig, cap: Cap, require: bool) {
    Policy::store_unprompted(r.home.path(), &[(cap, require)], &*r.seal).unwrap();
}

#[tokio::test]
async fn the_list_is_small_and_names_no_legacy_tool() {
    let r = rig(true);
    let list = rpc(&r.server, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.len() <= 24, "{names:?}");
    for want in [
        "list_spaces",
        "create_space",
        "space",
        "space_bash",
        "space_files",
        "computer",
        "volume",
        "teleport",
        "more",
        "approvals",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    for legacy in [
        "volume_approve",
        "volume_grant",
        "sandbox_create",
        "computer_click",
        "cloud_connect",
        "volume_storage_set",
    ] {
        assert!(!names.contains(&legacy), "{legacy} is not listed");
    }
    let bytes = serde_json::to_string(&list).unwrap().len();
    assert!(bytes < 14_000, "{bytes} bytes");
}

#[tokio::test]
async fn facade_calls_run_the_underlying_tool() {
    let r = rig(true);
    call(&r, "space", json!({"action": "stop", "space": "local:dev"})).await;
    call(
        &r,
        "space_files",
        json!({"action": "write", "space": "local:dev", "path": "a", "content": "b"}),
    )
    .await;
    let v = call(
        &r,
        "computer",
        json!({"action": "click", "space": "local:dev", "x": 1, "y": 2}),
    )
    .await;
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ext computer_click")
    );
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"sandbox\":\"local:dev\"")
    );
    assert_eq!(calls(&r), ["stop_space", "space_write"]);
    // A Space named by its display name reaches the tools that take a
    // sandbox ref by id.
    let v = call(
        &r,
        "computer",
        json!({"action": "screenshot", "space": "dev"}),
    )
    .await;
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"sandbox\":\"local:dev\""), "{text}");
    // The old name still works.
    call(
        &r,
        "space_bash",
        json!({"space": "local:dev", "command": "ls"}),
    )
    .await;
    assert_eq!(calls(&r).last().unwrap(), "space_bash");
    let bad = call(&r, "space", json!({"action": "explode"})).await;
    assert!(is_error(&bad));
}

#[tokio::test]
async fn gated_actions_are_refused_without_the_user_and_never_reach_the_backend() {
    let r = rig(false);
    for (tool, args, cap) in [
        (
            "add_space",
            json!({"url": "http://evil:1", "token": "t"}),
            Cap::Machines,
        ),
        ("cloud_connect", json!({"provider": "aws"}), Cap::Cloud),
        ("create_space", json!({"on": "cloud"}), Cap::Cloud),
        (
            "routine_add",
            json!({"agent": "a", "title": "t", "prompt": "p"}),
            Cap::Routines,
        ),
        ("volume_storage_set", json!({"backend": "s3"}), Cap::Storage),
        ("hotspot_start", json!({"space": "local:dev"}), Cap::Network),
        (
            "agent_start",
            json!({"space": "local:dev", "agent": "claude", "prompt": "x", "env_from_host": ["ANTHROPIC_API_KEY"]}),
            Cap::ApiKeys,
        ),
    ] {
        let v = call(&r, tool, args).await;
        assert!(is_error(&v), "{tool}");
        assert_eq!(kind(&v), "approval_denied", "{tool}: {v}");
        assert!(
            v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(cap.title()),
            "{tool} names the setting"
        );
    }
    assert!(calls(&r).is_empty(), "nothing ran: {:?}", calls(&r));
    assert_eq!(r.approver.asked.lock().unwrap().len(), 7);
}

#[tokio::test]
async fn an_approved_action_runs_and_a_turned_off_gate_does_not_ask() {
    let r = rig(true);
    let v = call(&r, "add_space", json!({"url": "http://x:1"})).await;
    assert!(!is_error(&v));
    assert_eq!(calls(&r), ["add_space"]);
    assert_eq!(r.approver.asked.lock().unwrap().len(), 1);

    set_policy(&r, Cap::Machines, false);
    let v = call(&r, "add_space", json!({"url": "http://y:1"})).await;
    assert!(!is_error(&v));
    assert_eq!(
        r.approver.asked.lock().unwrap().len(),
        1,
        "the setting applies at once"
    );

    // Local work needs nothing.
    call(&r, "create_space", json!({"on": "local"})).await;
    call(
        &r,
        "space_bash",
        json!({"space": "local:dev", "command": "ls"}),
    )
    .await;
    assert_eq!(r.approver.asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn commands_on_the_users_own_machines_are_gated_by_id_or_by_name() {
    let r = rig(false);
    for space in ["relay:mini", "mini", "direct:1.2.3.4:5"] {
        let v = call(&r, "space_bash", json!({"space": space, "command": "id"})).await;
        assert_eq!(kind(&v), "approval_denied", "{space}");
    }
    let v = call(
        &r,
        "computer",
        json!({"action": "screenshot", "space": "relay:mini"}),
    )
    .await;
    assert_eq!(kind(&v), "approval_denied");
    assert!(!calls(&r).contains(&"space_bash".to_string()));
    // A sandbox of its own is free.
    let v = call(&r, "space_bash", json!({"space": "dev", "command": "id"})).await;
    assert!(!is_error(&v));
    let v = call(
        &r,
        "space_bash",
        json!({"space": "local:dev", "command": "id"}),
    )
    .await;
    assert!(!is_error(&v));
}

#[tokio::test]
async fn agents_cannot_approve_their_own_requests_or_edit_access() {
    let r = rig(true);
    for tool in [
        "volume_approve",
        "volume_deny",
        "volume_grant",
        "volume_revoke",
        "volume_grants",
        "volume_requests",
    ] {
        let v = call(&r, tool, json!({"request_id": "r", "grant_id": "g", "principal": "p", "prefix": "x", "mode": "rw"})).await;
        assert_eq!(kind(&v), "forbidden", "{tool}");
        let via_more = call(&r, "more", json!({"name": tool, "arguments": {}})).await;
        assert!(is_error(&via_more), "{tool} through more");
    }
    assert!(calls(&r).is_empty());
    assert!(
        r.approver.asked.lock().unwrap().is_empty(),
        "the user is not even asked"
    );
}

#[tokio::test]
async fn volume_calls_are_made_as_the_calling_agent() {
    let r = rig(true);
    rpc(
        &r.server,
        "initialize",
        json!({"clientInfo": {"name": "Claude Code"}}),
    )
    .await;
    call(&r, "volume", json!({"action": "ls", "path": "agents/someone-else", "as_agent": "someone-else", "in_space": "x"})).await;
    let (tool, args) = r.backend.0.lock().unwrap().last().unwrap().clone();
    assert_eq!(tool, "volume_ls");
    assert_eq!(args["as_agent"], "host-claude-code", "{args}");
    assert!(args.get("in_space").is_none());
    // Also through the old name: never the whole volume as the user.
    call(&r, "volume_read", json!({"path": "agents/x/secret"})).await;
    assert_eq!(
        r.backend.0.lock().unwrap().last().unwrap().1["as_agent"],
        "host-claude-code"
    );
}

#[tokio::test]
async fn sensitive_host_files_and_settings_are_protected() {
    let r = rig(false);
    let home = std::env::var("HOME").unwrap_or_default();
    let v = call(&r, "space_files", json!({"action": "upload", "space": "local:dev", "path": format!("{home}/.ssh/id_ed25519")})).await;
    assert_eq!(kind(&v), "approval_denied");
    let v = call(
        &r,
        "space_files",
        json!({"action": "send", "space": "local:dev", "path": "~/.aws"}),
    )
    .await;
    assert_eq!(kind(&v), "approval_denied");
    // A download cannot land in the Cua home (where the policy lives).
    let cua = cua_home::cua_home();
    let v = call(&r, "space_files", json!({"action": "download", "space": "local:dev", "path": "x", "dest": cua.to_string_lossy()})).await;
    assert_eq!(kind(&v), "forbidden");
    assert!(calls(&r).is_empty(), "{:?}", calls(&r));
}

#[tokio::test]
async fn more_lists_describes_and_calls_rare_tools_through_the_same_gate() {
    let r = rig(false);
    let v = call(&r, "more", json!({})).await;
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("cloud_connect"), "{text}");
    assert!(!text.contains("volume_approve"));
    assert!(text.len() < 4_000, "{} bytes", text.len());
    let v = call(&r, "more", json!({"name": "cloud_connect"})).await;
    assert!(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("inputSchema")
    );
    let v = call(
        &r,
        "more",
        json!({"name": "cloud_sweep", "arguments": {"all": true}}),
    )
    .await;
    assert_eq!(kind(&v), "approval_denied");
    let v = call(&r, "more", json!({"name": "space_bash", "arguments": {}})).await;
    assert!(is_error(&v), "only the rare tools go through more");
    assert!(calls(&r).is_empty());
}

#[tokio::test]
async fn approvals_shows_the_settings_and_has_no_way_to_change_them() {
    let r = rig(true);
    let v = call(&r, "approvals", json!({})).await;
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Use the cloud"), "{text}");
    let list = rpc(&r.server, "tools/list", json!({})).await;
    let schema = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "approvals")
        .unwrap()["inputSchema"]
        .clone();
    assert!(schema["properties"].as_object().unwrap().is_empty());
    // Asking to change it through any other tool is not a thing.
    let before = std::fs::read_to_string(cua_spaces::approvals::path_in(r.home.path())).unwrap();
    let v = call(&r, "approvals", json!({"cloud": false})).await;
    let after = std::fs::read_to_string(cua_spaces::approvals::path_in(r.home.path())).unwrap();
    assert_eq!(before, after, "{v}");
}

#[tokio::test]
async fn permissions_filter_still_applies_underneath() {
    let backend = Arc::new(Recorder::default());
    let guard = Guard::new(
        tempfile::tempdir().unwrap().keep(),
        Arc::new(Approvals {
            asked: Mutex::default(),
            yes: true,
        }),
    );
    let server = McpServer::remote(backend.clone())
        .with_filter(Arc::new(|t: &str| t == "list_spaces"))
        .with_agent_surface(guard);
    let list = rpc(&server, "tools/list", json!({})).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"list_spaces"));
    assert!(
        !names.contains(&"space_bash") && !names.contains(&"space") && !names.contains(&"volume"),
        "{names:?}"
    );
    let v = rpc(
        &server,
        "tools/call",
        json!({"name": "space", "arguments": {"action": "delete", "space": "x"}}),
    )
    .await;
    assert!(v.get("error").is_some() || v["result"]["isError"] == true);
    assert!(backend.0.lock().unwrap().is_empty());
}
