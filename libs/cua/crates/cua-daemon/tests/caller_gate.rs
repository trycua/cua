//! The approval gate at the daemon: the HTTP `/mcp` endpoint and the raw
//! socket reach the same policy as `cua mcp`, and a caller that is not the
//! user's own Cua code cannot do a gated action; the user's own client can,
//! without being asked. Real processes: the untrusted peer is `curl`.
//! Everything lives in a throwaway home; the only processes are this test's.
#![cfg(all(unix, feature = "spaces"))]

use cua_daemon::{
    Runtime, RuntimeConfig,
    caller::{Caller, PeerVerifier, TrustAll},
    client::{DaemonAddress, DaemonClient},
    server::{self, GateOptions, ServerConfig},
};
use cua_proto::daemon::v1 as pb;
use cua_spaces::approvals::{Approver, Cap, MemorySeal, Policy};
use cua_spaces::mcp::gate::Guard;
use prost::Message;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Records what the user was asked and answers no (nobody is at the screen).
#[derive(Default)]
struct Asked(Mutex<Vec<String>>);
impl Approver for Asked {
    fn confirm(&self, reason: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(reason.to_string());
        Err("declined".into())
    }
}

struct Rig {
    handle: server::DaemonHandle,
    sock: std::path::PathBuf,
    url: String,
    asked: Arc<Asked>,
    dir: tempfile::TempDir,
}

async fn rig(verifier: Option<Arc<dyn PeerVerifier>>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // Every capability gated, as the user's sealed policy says.
    let seal = Arc::new(MemorySeal::default());
    let all: Vec<(Cap, bool)> = Cap::ALL.iter().map(|c| (*c, true)).collect();
    Policy::store_unprompted(&home, &all, &*seal).unwrap();
    let asked = Arc::new(Asked::default());
    let guard = Guard::new(home.clone(), asked.clone()).with_seal(seal);
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().join("spaces")),
        ..Default::default()
    })
    .unwrap();
    let sock = dir.path().join("cua.sock");
    let config = ServerConfig {
        socket_path: Some(sock.clone()),
        loopback: Some("127.0.0.1:0".parse().unwrap()),
        token: "agent-tier-token".into(),
        discovery_path: Some(dir.path().join("daemon.json")),
        bridge_ticket_ttl: Duration::from_secs(30),
    };
    let handle = server::start_with(
        runtime,
        config,
        GateOptions {
            peer_verifier: verifier,
            guard: Some(guard),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let url = handle.loopback_url.clone().unwrap();
    Rig {
        handle,
        sock,
        url,
        asked,
        dir,
    }
}

async fn mcp(r: &Rig, token: &str, body: Value) -> Value {
    let resp = reqwest::Client::new()
        .post(format!("{}/mcp", r.url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());
    resp.json().await.unwrap()
}

fn rpc(id: u32, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn kind(v: &Value) -> String {
    v["result"]["structuredContent"]["error"]["kind"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_mcp_goes_through_the_gate_and_the_listed_surface() {
    let r = rig(None).await;
    let tok = "agent-tier-token";
    let init = mcp(
        &r,
        tok,
        rpc(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "evil-agent"}}),
        ),
    )
    .await;
    assert!(init["result"]["instructions"].as_str().unwrap().len() > 10);
    let list = mcp(&r, tok, rpc(2, "tools/list", json!({}))).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"more") && names.contains(&"approvals"),
        "{names:?}"
    );
    // The full contract list (every tool, ungated) is not what is served.
    assert!(
        !names.contains(&"cloud_connect") && names.len() < 30,
        "{names:?}"
    );

    // A gated action by its old name, and through `more`: refused, the user
    // asked (nobody answers), nothing ran.
    for call in [
        rpc(
            3,
            "tools/call",
            json!({"name": "cloud_connect", "arguments": {"provider": "aws"}}),
        ),
        rpc(
            4,
            "tools/call",
            json!({"name": "more", "arguments": {"name": "cloud_connect", "arguments": {"provider": "aws"}}}),
        ),
        rpc(
            5,
            "tools/call",
            json!({"name": "add_space", "arguments": {"url": "http://127.0.0.1:1"}}),
        ),
    ] {
        let v = mcp(&r, tok, call).await;
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(kind(&v), "approval_denied", "{v}");
        assert!(v.to_string().contains("Settings"), "{v}");
    }
    assert_eq!(r.asked.0.lock().unwrap().len(), 3);
    // The Volume's approval tools are never an agent's.
    let v = mcp(
        &r,
        tok,
        rpc(
            6,
            "tools/call",
            json!({"name": "volume_approve", "arguments": {}}),
        ),
    )
    .await;
    assert_eq!(kind(&v), "forbidden", "{v}");
    // A read is free.
    let v = mcp(
        &r,
        tok,
        rpc(
            7,
            "tools/call",
            json!({"name": "list_spaces", "arguments": {}}),
        ),
    )
    .await;
    assert_eq!(v["result"]["isError"], false, "{v}");

    // Who can reach it: no token, a wrong one, and a web page's Origin.
    let c = reqwest::Client::new();
    let url = format!("{}/mcp", r.url);
    let body = rpc(8, "tools/list", json!({}));
    assert_eq!(c.post(&url).json(&body).send().await.unwrap().status(), 401);
    assert_eq!(
        c.post(&url)
            .bearer_auth("nope")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        c.post(&url)
            .bearer_auth(tok)
            .header("origin", "https://evil.example")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    r.handle.shutdown();
}

/// One gRPC-Web call through `curl`, a process that is not Cua: the status
/// code of the reply and its text.
fn curl_grpc(sock: &Path, method: &str, msg: &impl Message) -> (i32, String) {
    let body = msg.encode_to_vec();
    let mut frame = vec![0u8];
    frame.extend((body.len() as u32).to_be_bytes());
    frame.extend(body);
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &frame).unwrap();
    let out = std::process::Command::new("curl")
        .args(["-s", "-i", "--max-time", "20", "--unix-socket"])
        .arg(sock)
        .args([
            "-H",
            "content-type: application/grpc-web+proto",
            "-H",
            "x-grpc-web: 1",
        ])
        .args(["--data-binary", &format!("@{}", f.path().display())])
        .arg(format!("http://localhost/cua.daemon.v1.{method}"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let status = text
        .to_ascii_lowercase()
        .split("grpc-status:")
        .nth(1)
        .and_then(|s| {
            s.trim_start()
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .map(str::to_string)
        })
        .and_then(|s| s.parse().ok())
        .unwrap_or(-1);
    (status, text)
}

const PERMISSION_DENIED: i32 = 7;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_process_that_is_not_cua_cannot_do_a_gated_action_on_the_socket() {
    let r = rig(None).await;
    let sock = r.sock.clone();
    let asked = r.asked.clone();
    let dir = r.dir.path().to_path_buf();
    let (cloud, add) = tokio::task::spawn_blocking(move || {
        let cloud = curl_grpc(
            &sock,
            "SpaceService/CallSpaceTool",
            &pb::CallSpaceToolRequest {
                name: "cloud_connect".into(),
                arguments_json: r#"{"provider":"aws"}"#.into(),
            },
        );
        let add = curl_grpc(
            &sock,
            "SpaceService/AddSpace",
            &pb::AddSpaceRequest {
                url: "http://127.0.0.1:1".into(),
                ..Default::default()
            },
        );
        // And a cloud sandbox, by the sandbox service.
        let sb = curl_grpc(
            &sock,
            "SandboxService/CreateSandbox",
            &pb::CreateSandboxRequest {
                location: "cloud".into(),
                ..Default::default()
            },
        );
        assert_eq!(sb.0, PERMISSION_DENIED, "{}", sb.1);
        (cloud, add)
    })
    .await
    .unwrap();
    // Each is a refused call that names the setting (percent-encoded in the
    // trailer).
    assert_eq!(cloud.0, PERMISSION_DENIED, "{}", cloud.1);
    assert!(cloud.1.contains("Permissions"), "{}", cloud.1);
    assert_eq!(add.0, PERMISSION_DENIED, "{}", add.1);
    assert!(
        asked.0.lock().unwrap().len() >= 3,
        "the user was asked each time"
    );
    // Nothing was registered.
    let c = DaemonClient::new(DaemonAddress::Socket(r.sock.clone())).unwrap();
    let spaces = c
        .spaces()
        .list_spaces(pb::ListSpacesRequest::default())
        .await
        .unwrap();
    assert!(spaces.into_inner().spaces.is_empty());
    drop(dir);

    // An untrusted client is not locked out of what is free.
    let sock = r.sock.clone();
    let (status, _) = tokio::task::spawn_blocking(move || {
        curl_grpc(
            &sock,
            "SpaceService/ListSpaces",
            &pb::ListSpacesRequest::default(),
        )
    })
    .await
    .unwrap();
    assert_eq!(status, 0);
    // It cannot stop the daemon either.
    let sock = r.sock.clone();
    let (status, _) = tokio::task::spawn_blocking(move || {
        curl_grpc(
            &sock,
            "DaemonService/Shutdown",
            &pb::ShutdownRequest::default(),
        )
    })
    .await
    .unwrap();
    assert_eq!(status, PERMISSION_DENIED);
    r.handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_users_own_client_is_not_asked() {
    // This test binary is the daemon's own program: the user's `cua`.
    let r = rig(None).await;
    let c = DaemonClient::new(DaemonAddress::Socket(r.sock.clone())).unwrap();
    let e = c
        .spaces()
        .add_space(pb::AddSpaceRequest {
            url: "http://127.0.0.1:1".into(),
            ..Default::default()
        })
        .await
        .expect_err("nothing listens there");
    // It got as far as trying to connect: the gate let it through.
    assert!(!e.message().contains("approval"), "{e}");
    assert!(
        r.asked.0.lock().unwrap().is_empty(),
        "the user was never asked"
    );
    r.handle.shutdown();
}

/// Says a socket peer is an agent, whatever it is.
#[derive(Debug)]
struct EveryoneIsAnAgent;
impl PeerVerifier for EveryoneIsAnAgent {
    fn verify(&self, _fd: i32) -> Caller {
        Caller::Agent("test".into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_peer_verifier_decides_and_tokens_are_tiered() {
    // An agent by the verifier's word: refused, though it is our own program.
    let r = rig(Some(Arc::new(EveryoneIsAnAgent))).await;
    let c = DaemonClient::new(DaemonAddress::Socket(r.sock.clone())).unwrap();
    let e = c
        .spaces()
        .add_space(pb::AddSpaceRequest {
            url: "http://127.0.0.1:1".into(),
            ..Default::default()
        })
        .await
        .expect_err("refused");
    assert_eq!(e.code(), tonic::Code::PermissionDenied, "{e}");
    // It is handed the discovery-file tier of token.
    let info = c.info().await.unwrap();
    assert_eq!(info.loopback_token, "agent-tier-token");
    r.handle.shutdown();

    // A verified peer is handed the other token, which is not on disk.
    let r = rig(Some(Arc::new(TrustAll))).await;
    let c = DaemonClient::new(DaemonAddress::Socket(r.sock.clone())).unwrap();
    let user_token = c.info().await.unwrap().loopback_token;
    assert_ne!(user_token, "agent-tier-token");
    let on_disk = std::fs::read_to_string(r.dir.path().join("daemon.json")).unwrap();
    assert!(!on_disk.contains(&user_token), "{on_disk}");
    // The machine passthrough: the file's token is an agent (refused), the
    // user's is the user (past the gate; the Space does not exist).
    let key = "ZGlyZWN0OjEyNy4wLjAuMTox"; // base64url("direct:127.0.0.1:1")
    let url = format!("{}/v1/spaces/{key}/env/x", r.url);
    let http = reqwest::Client::new();
    let agent = http
        .get(&url)
        .bearer_auth("agent-tier-token")
        .send()
        .await
        .unwrap();
    assert_eq!(agent.status(), 403);
    let user = http
        .get(&url)
        .bearer_auth(&user_token)
        .send()
        .await
        .unwrap();
    assert_ne!(user.status(), 403, "the user's own client is not gated");
    assert!(
        r.asked.0.lock().unwrap().len() == 1,
        "only the agent's call asked"
    );
    r.handle.shutdown();
}
