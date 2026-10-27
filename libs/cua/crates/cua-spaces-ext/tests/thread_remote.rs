// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A teammate reads and posts in the thread of a Space another machine
//! provides, from their own device, through that machine's daemon.
//!
//! Hermetic: the owner is a real cua daemon on a temporary socket and home
//! (its Space a mock spacesd, provided as `relay:space-room0001`); the
//! relay is the fake directory (`cua_host::testing::FakeRelay`); the host's
//! cua-spacesd is a stand-in forwarder that stamps the caller the relay
//! would have verified for the request's account token and forwards to the
//! daemon's socket, as the real one does (its own tests are in
//! cua-spacesd-server's `tests/host_spaces.rs`). The teammates are Spaces
//! runtimes of their own (temporary homes), signed in to the fake relay as
//! bob (an editor of the host) and carol (a viewer).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_host::StaticToken;
use cua_host::provided::ProvidedSpace;
use cua_host::relay::{MachinePatch, RegisterRequest};
use cua_host::testing::FakeRelay;
use cua_proto::env::v1 as pb;
use cua_proto::env::v1::host_thread_service_server::{HostThreadService, HostThreadServiceServer};
use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::thread::policy::GUEST_READ_ONLY;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const HOST: &str = "host-ada00001";
const ROOM: &str = "space-room0001";
const OLD_HOST: &str = "host-old00001";
const OLD_ROOM: &str = "space-old00001";
const NO_DAEMON_HOST: &str = "host-nod00001";
const NO_DAEMON_ROOM: &str = "space-nod00001";

/// The stand-in for the host's cua-spacesd: the relay-verified caller for
/// the request's account token (any `x-cua-host-caller` the client sent is
/// dropped), forwarded to the daemon's socket. Records `(tool, follow_ms)`
/// of each call it forwards.
#[derive(Clone)]
struct Forwarder {
    socket: PathBuf,
    calls: Calls,
}

/// `(tool, follow_ms)` of each forwarded call.
type Calls = Arc<Mutex<Vec<(String, Option<u64>)>>>;

fn verified_caller(token: &str) -> Option<&'static str> {
    Some(match token {
        "ada-token" => r#"{"account":"ada","name":"Ada","role":"owner","via":"relay"}"#,
        "bob-token" => r#"{"account":"bob","name":"Bob","role":"shared","via":"relay"}"#,
        "carol-token" => r#"{"account":"carol","name":"Carol","role":"viewer","via":"relay"}"#,
        _ => return None,
    })
}

#[tonic::async_trait]
impl HostThreadService for Forwarder {
    async fn call_thread_tool(
        &self,
        req: tonic::Request<pb::CallThreadToolRequest>,
    ) -> Result<tonic::Response<pb::CallThreadToolResponse>, tonic::Status> {
        let token = ["x-cua-relay-authorization", "authorization"]
            .iter()
            .find_map(|h| req.metadata().get(*h))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_string();
        // Dave: only the Space is shared with him; the relay grants its
        // thread alone (the Space he asked for, as its editor).
        let granted = req
            .metadata()
            .get(cua_spaces::thread_route::THREAD_SPACE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let who = match (token.as_str(), granted) {
            ("dave-token", Some(space)) => json!({"account": "dave", "name": "Dave",
                "role": "shared", "space": space, "via": "relay"})
            .to_string(),
            _ => verified_caller(&token)
                .ok_or_else(|| tonic::Status::unauthenticated("not a relay account"))?
                .to_string(),
        };
        let r = req.into_inner();
        let args: Value = serde_json::from_str(&r.arguments_json).unwrap_or(Value::Null);
        self.calls
            .lock()
            .unwrap()
            .push((r.name.clone(), args["follow_ms"].as_u64()));
        // A fresh request: only the verified caller goes to the daemon.
        let mut out = tonic::Request::new(r);
        out.metadata_mut()
            .insert("x-cua-host-caller", who.parse().unwrap());
        out.set_timeout(Duration::from_secs(60));
        let daemon = cua_daemon::client::DaemonClient::new(
            cua_daemon::client::DaemonAddress::Socket(self.socket.clone()),
        )
        .unwrap();
        daemon.host_thread().call_thread_tool(out).await
    }
}

/// Serves `routes` over gRPC-Web on loopback (what the fake relay tunnels).
async fn serve_grpc_web(routes: tonic::service::Routes) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .accept_http1(true)
            .layer(tonic_web::GrpcWebLayer::new())
            .add_routes(routes)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    format!("http://{addr}")
}

/// A machine on the fake relay, registered by ada: `host` names the machine
/// that provides it; bob may edit and carol may watch.
async fn register(relay: &FakeRelay, id: &str, host: Option<&str>) {
    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    client
        .register(
            "ada-token",
            &RegisterRequest {
                id: id.into(),
                name: id.into(),
                allow: vec!["bob".into()],
                host: host.map(str::to_string),
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    client
        .patch(
            "ada-token",
            id,
            &MachinePatch {
                viewers: Some(vec!["carol".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

/// Another person's device: a Spaces runtime of its own, signed in to the
/// relay with `token`.
fn device(relay: &FakeRelay, token: &str) -> (Spaces, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder()
            .home(home.path())
            .relay(cua_spaces::RelayAccount::new(
                &relay.url,
                Arc::new(StaticToken(token.into())),
            )),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    (spaces, home)
}

type Out = Result<Value, (String, String)>;

async fn call(spaces: &Spaces, tool: &str, args: Value) -> Out {
    let out = McpServer::new(spaces.clone()).call(tool, args).await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

/// One raw `CallThreadTool` through the relay to `host`, as `spaces`'
/// account.
async fn raw(
    spaces: &Spaces,
    host: &str,
    space: &str,
    tool: &str,
    args: Value,
    forged: Option<&str>,
) -> pb::CallThreadToolResponse {
    let s = spaces.space(&format!("relay:{host}")).await.unwrap();
    let mut req = tonic::Request::new(pb::CallThreadToolRequest {
        space: space.into(),
        name: tool.into(),
        arguments_json: args.to_string(),
    });
    if let Some(f) = forged {
        req.metadata_mut()
            .insert("x-cua-host-caller", f.parse().unwrap());
    }
    s.spacesd()
        .unwrap()
        .host_thread()
        .call_thread_tool(req)
        .await
        .unwrap()
        .into_inner()
}

/// One raw `CallThreadTool` to `host` through the relay as `token`'s
/// account, asking for the thread of `granted` (a thread grant).
async fn raw_granted(
    relay: &FakeRelay,
    token: &str,
    host: &str,
    granted: &str,
    space: &str,
    tool: &str,
) -> pb::CallThreadToolResponse {
    let url = cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .machine_url(host);
    let client = cua_spacesd_client::SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&url)
            .unwrap()
            .gateway(Arc::new(cua_spacesd_client::transport::StaticBearer(
                token.into(),
            )))
            .header(cua_spaces::thread_route::THREAD_SPACE_HEADER, granted)
            .transport(cua_spacesd_client::TransportPreference::GrpcWeb)
            .probe(false),
    )
    .await
    .unwrap();
    client
        .host_thread()
        .call_thread_tool(pb::CallThreadToolRequest {
            space: space.into(),
            name: tool.into(),
            arguments_json: "{}".into(),
        })
        .await
        .unwrap()
        .into_inner()
}

fn entry_of(r: &pb::CallThreadToolResponse) -> Value {
    assert!(!r.is_error, "{}", r.content_json);
    let content: Value = serde_json::from_str(&r.content_json).unwrap();
    serde_json::from_str::<Value>(content[0]["text"].as_str().unwrap()).unwrap()["entry"].clone()
}

fn error_of(r: &pb::CallThreadToolResponse) -> (String, String) {
    assert!(r.is_error, "{}", r.content_json);
    let s: Value = serde_json::from_str(&r.structured_json).unwrap();
    (
        s["error"]["kind"].as_str().unwrap().to_string(),
        s["error"]["message"].as_str().unwrap().to_string(),
    )
}

#[test]
fn a_teammate_reads_posts_and_tasks_an_agent_through_the_owners_daemon() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_stack_size(16 << 20)
                .enable_all()
                .build()
                .unwrap()
                .block_on(scenario())
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn scenario() {
    use cua_daemon::client::{DaemonAddress, DaemonClient};
    use cua_daemon::server::{self, ServerConfig};
    use cua_daemon::{Runtime, RuntimeConfig};

    // Short: macOS caps Unix socket paths at 104 bytes.
    let root = tempfile::Builder::new()
        .prefix("cuatr")
        .tempdir_in("/tmp")
        .unwrap();
    let home = root.path().to_path_buf();
    std::fs::create_dir_all(home.join("user").join(".cua")).unwrap();
    // No supervisor, no drive services, no sign-in read, no OS vault.
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("HOME", home.join("user"));
        std::env::set_var("CUA_HOME", home.join("user").join(".cua"));
        std::env::set_var(cua_daemon::server::ENV_DAEMON_PERSISTENT, "0");
        std::env::set_var(cua_spaces_ext::daemon::ENV_DAEMON_DRIVE, "0");
        std::env::set_var(cua_spaces_ext::daemon::ENV_DAEMON_OWNER_IDENTITY, "0");
        std::env::set_var(
            "CUA_CREDENTIAL_STORE",
            format!("test-keychain:{}", home.join("kc").display()),
        );
    }

    // The owner: a daemon whose Space `room` is provided as relay:ROOM.
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sbx")),
        spaces_home: Some(home.clone()),
        providers: Some(Vec::new()),
        extensions: vec![Arc::new(
            cua_spaces_ext::daemon::CuaSpacesDaemon::with_test_presence(Arc::new(
                cua_keyvault::broker::FakePresence::new(true),
            )),
        )],
        ..Default::default()
    })
    .expect("daemon runtime");
    let env = MockServer::start(MockAuth::default()).await;
    let room = runtime
        .spaces()
        .add(&env.url(), None, Some("room".into()))
        .await
        .unwrap()
        .id;
    cua_host::provided::save_provided(
        &home.join("host"),
        &[ProvidedSpace {
            relay_machine: ROOM.into(),
            local_space: room.clone(),
            name: "room".into(),
            image: "linux".into(),
            created_by: "ada".into(),
            created_by_account: "ada".into(),
            ..Default::default()
        }],
    )
    .unwrap();
    let sock = home.join("cua.sock");
    let daemon = server::start(
        runtime,
        ServerConfig {
            socket_path: Some(sock.clone()),
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: "t0k3n".into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();
    let owner = DaemonClient::new(DaemonAddress::Socket(sock.clone())).unwrap();
    let owner_call = |tool: &'static str, args: Value| {
        let owner = owner.clone();
        async move {
            let r = owner
                .spaces()
                .call_space_tool(cua_proto::daemon::v1::CallSpaceToolRequest {
                    name: tool.into(),
                    arguments_json: args.to_string(),
                })
                .await
                .unwrap()
                .into_inner();
            assert!(!r.is_error, "{}", r.content_json);
            let content: Value = serde_json::from_str(&r.content_json).unwrap();
            serde_json::from_str::<Value>(content[0]["text"].as_str().unwrap()).unwrap()
        }
    };
    owner_call(
        "agent_add",
        json!({"space": room, "name": "ada", "harness": "claude-code"}),
    )
    .await;

    // The relay: ada's host (online, providing Spaces, its HostThreadService
    // the forwarder) and the Space it provides.
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("bob-token", "bob", Some("bob@example.com"));
    relay.add_account("carol-token", "carol", Some("carol@example.com"));
    relay.add_account("dave-token", "dave", Some("dave@example.com"));
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let forwarder = serve_grpc_web(tonic::service::Routes::new(HostThreadServiceServer::new(
        Forwarder {
            socket: sock.clone(),
            calls: calls.clone(),
        },
    )))
    .await;
    let host_env = MockServer::start(MockAuth::default()).await;
    host_env.state.advertise(&["host_spaces", "host_thread"]);
    register(&relay, HOST, None).await;
    register(&relay, ROOM, Some(HOST)).await;
    relay.set_online(HOST, true, "1.0.0");
    relay.tunnel(HOST, &host_env.url());
    relay.tunnel_service(HOST, "/cua.env.v1.HostThreadService/", &forwarder);

    // Dave: the Space is shared with him, the host is not.
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .patch(
            "ada-token",
            ROOM,
            &MachinePatch {
                allow: Some(vec!["bob".into(), "dave".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (bob, bob_home) = device(&relay, "bob-token");
    let (carol, _carol_home) = device(&relay, "carol-token");
    let space = format!("relay:{ROOM}");

    // Bob posts and @mentions the agent: a task in the owner's thread,
    // authored by Bob's account (from the relay, never the arguments).
    let posted = call(
        &bob,
        "thread_post",
        json!({"space": space, "text": "@ada run the e2e suite"}),
    )
    .await
    .unwrap();
    let e = &posted["entry"];
    assert_eq!(
        e["author"],
        json!({"kind": "person", "id": "bob", "name": "Bob"})
    );
    assert_eq!(e["task"]["to"], "ada");
    let seq = e["seq"].as_u64().unwrap();
    // It is the owner's thread: the owner reads it there, and nothing was
    // written on Bob's device.
    let mine = owner_call("thread_read", json!({"space": room})).await;
    let last = mine["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["seq"].as_u64(), Some(seq));
    assert_eq!(last["author"]["id"], "bob");
    assert!(
        !bob_home.path().join("threads").exists(),
        "no thread on the teammate's device"
    );
    let inbox = owner_call("thread_inbox", json!({"space": room, "member": "ada"})).await;
    assert!(
        inbox["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["seq"].as_u64() == Some(seq) && t["author"]["id"] == "bob"),
        "{inbox}"
    );

    // Bob and Carol read it from their devices; each is told their role.
    let read = call(&bob, "thread_read", json!({"space": space}))
        .await
        .unwrap();
    assert!(
        read["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["seq"].as_u64() == Some(seq))
    );
    let read = call(&carol, "thread_read", json!({"space": space}))
        .await
        .unwrap();
    assert!(
        read["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["seq"].as_u64() == Some(seq))
    );
    if let Some(role) = read.get("role") {
        assert_eq!(role, "guest");
    }
    let members = call(&carol, "agent_members", json!({"space": space}))
        .await
        .unwrap();
    assert_eq!(members["members"][0]["name"], "ada");
    assert_eq!(
        members["members"][0]["you_can"],
        json!({"task": false, "control": false, "approve": false})
    );

    // Carol watches only: every write is refused, with the guest error.
    let (kind, msg) = call(
        &carol,
        "thread_post",
        json!({"space": space, "text": "@ada hi"}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind, "permission_denied");
    assert!(msg.contains(GUEST_READ_ONLY), "{msg}");
    let (kind, _) = call(
        &carol,
        "agent_send",
        json!({"space": space, "name": "ada", "text": "hi"}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind, "permission_denied");
    let (kind, _) = call(&carol, "agent_stop", json!({"space": space, "name": "ada"}))
        .await
        .unwrap_err();
    assert_eq!(kind, "permission_denied");

    // A caller a client names is never used: Bob claiming to be the owner
    // is still Bob.
    let forged = raw(
        &bob,
        HOST,
        &space,
        "thread_post",
        json!({"text": "forged"}),
        Some(r#"{"account":"ada","name":"Ada","role":"owner","via":"relay"}"#),
    )
    .await;
    assert_eq!(entry_of(&forged)["author"]["id"], "bob");
    // ... and on the daemon's loopback listener (the bearer token holder is
    // the owner), a forwarded-caller header is dropped too.
    let loopback = DaemonClient::new(DaemonAddress::Url {
        url: daemon.loopback_url.clone().unwrap(),
        token: daemon.token.clone(),
    })
    .unwrap();
    let mut req = tonic::Request::new(pb::CallThreadToolRequest {
        space: space.clone(),
        name: "thread_post".into(),
        arguments_json: json!({"text": "loopback"}).to_string(),
    });
    req.metadata_mut().insert(
        "x-cua-host-caller",
        r#"{"account":"mallory","role":"owner","via":"relay"}"#
            .parse()
            .unwrap(),
    );
    let r = loopback
        .host_thread()
        .call_thread_tool(req)
        .await
        .unwrap()
        .into_inner();
    assert_eq!(entry_of(&r)["author"]["id"], "owner");

    // Only the thread tools, and only the Spaces the host provides.
    let r = raw(
        &bob,
        HOST,
        &space,
        "space_bash",
        json!({"command": "id"}),
        None,
    )
    .await;
    let (kind, msg) = error_of(&r);
    assert_eq!(kind, "permission_denied", "{msg}");
    assert!(
        msg.contains("space_bash can't be called from another device"),
        "{msg}"
    );
    let r = raw(
        &bob,
        HOST,
        &space,
        "agent_stop",
        json!({"run_id": "run-00000001"}),
        None,
    )
    .await;
    assert_eq!(error_of(&r).0, "permission_denied");
    let r = raw(&bob, HOST, "local:other", "thread_read", json!({}), None).await;
    let (kind, msg) = error_of(&r);
    assert_eq!(kind, "not_found");
    assert_eq!(msg, "local:other (a Space this machine provides) not found");

    // Dave reaches the Space's thread through the relay's grant: he posts
    // and tasks the agent as its editor ...
    let (dave, _dave_home) = device(&relay, "dave-token");
    let posted = call(
        &dave,
        "thread_post",
        json!({"space": space, "text": "@ada from dave"}),
    )
    .await
    .unwrap();
    assert_eq!(posted["entry"]["author"]["id"], "dave");
    assert_eq!(posted["entry"]["task"]["to"], "ada");
    // ... and no other Space on that machine.
    let r = raw_granted(
        &relay,
        "dave-token",
        HOST,
        ROOM,
        "relay:space-othr0001",
        "thread_read",
    )
    .await;
    let (kind, msg) = error_of(&r);
    assert_eq!(kind, "permission_denied");
    assert_eq!(
        msg,
        "relay:space-othr0001: a Space shared with you reaches only its own thread here \
         (relay:space-room0001)"
    );
    let r = raw_granted(&relay, "dave-token", HOST, ROOM, &room, "thread_read").await;
    assert_eq!(
        error_of(&r).0,
        "permission_denied",
        "not by its id on the host"
    );
    // Revoking the Space's share ends it.
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .patch(
            "ada-token",
            ROOM,
            &MachinePatch {
                allow: Some(vec!["bob".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (dave, _dave_home) = device(&relay, "dave-token");
    let (kind, msg) = call(&dave, "thread_read", json!({"space": space}))
        .await
        .unwrap_err();
    assert_eq!(kind, "not_found");
    assert_eq!(
        msg,
        "relay:space-room0001 (not one of your Spaces, nor shared with you) not found"
    );

    // Following from another device: a long poll returns the new entry as
    // soon as it is posted.
    let cursor = call(&bob, "thread_read", json!({"space": space}))
        .await
        .unwrap()["cursor"]
        .as_u64()
        .unwrap();
    let follow = {
        let bob = bob.clone();
        let space = space.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let r = call(
                &bob,
                "thread_read",
                json!({"space": space, "after": cursor, "follow_ms": 10000}),
            )
            .await
            .unwrap();
            (r, t.elapsed())
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    owner_call(
        "thread_post",
        json!({"space": room, "text": "the suite is green"}),
    )
    .await;
    let (r, took) = tokio::time::timeout(Duration::from_secs(20), follow)
        .await
        .unwrap()
        .unwrap();
    assert!(took < Duration::from_secs(8), "{took:?}");
    let got = r["entries"].as_array().unwrap();
    assert_eq!(got.len(), 1, "{r}");
    assert_eq!(got[0]["text"], "the suite is green");
    // A long poll longer than one relayed wait is several of them, each at
    // most FOLLOW_CHUNK_MS, and ends when its time is up.
    calls.lock().unwrap().clear();
    let cursor = got[0]["seq"].as_u64().unwrap();
    let t = Instant::now();
    let r = call(
        &bob,
        "thread_read",
        json!({"space": space, "after": cursor, "follow_ms": 16000}),
    )
    .await
    .unwrap();
    assert!(r["entries"].as_array().unwrap().is_empty());
    let took = t.elapsed();
    assert!(
        took >= Duration::from_millis(15500) && took < Duration::from_secs(25),
        "{took:?}"
    );
    let waits: Vec<Option<u64>> = calls.lock().unwrap().iter().map(|c| c.1).collect();
    assert_eq!(waits.len(), 2, "{waits:?}");
    assert_eq!(waits[0], Some(cua_spaces::thread_route::FOLLOW_CHUNK_MS));
    assert!(waits[1].is_some_and(|w| w > 0 && w <= 1000), "{waits:?}");

    // An owner on an older Cua: a host that does not serve the thread, or
    // whose daemon predates it, gets the same "update Cua" answer.
    let old_env = MockServer::start(MockAuth::default()).await;
    old_env.state.advertise(&["host_spaces"]);
    register(&relay, OLD_HOST, None).await;
    register(&relay, OLD_ROOM, Some(OLD_HOST)).await;
    relay.set_online(OLD_HOST, true, "1.0.0");
    relay.tunnel(OLD_HOST, &old_env.url());
    let want = cua_spaces::thread_route::owner_too_old(OLD_HOST);
    let (kind, msg) = call(
        &bob,
        "thread_read",
        json!({"space": format!("relay:{OLD_ROOM}")}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind, "capability_missing");
    assert!(msg.contains(&want), "{msg}");
    // A new cua-spacesd in front of a daemon that has no HostThreadService.
    let new_env = MockServer::start(MockAuth::default()).await;
    new_env.state.advertise(&["host_spaces", "host_thread"]);
    let bare = serve_grpc_web(tonic::service::Routes::new(
        pb::host_spaces_service_server::HostSpacesServiceServer::new(Unimplemented),
    ))
    .await;
    register(&relay, NO_DAEMON_HOST, None).await;
    register(&relay, NO_DAEMON_ROOM, Some(NO_DAEMON_HOST)).await;
    relay.set_online(NO_DAEMON_HOST, true, "1.0.0");
    relay.tunnel(NO_DAEMON_HOST, &new_env.url());
    relay.tunnel_service(NO_DAEMON_HOST, "/cua.env.v1.HostThreadService/", &bare);
    let want = cua_spaces::thread_route::owner_too_old(NO_DAEMON_HOST);
    let (kind, msg) = call(
        &bob,
        "thread_post",
        json!({"space": format!("relay:{NO_DAEMON_ROOM}"), "text": "hi"}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind, "capability_missing");
    assert!(msg.contains(&want), "{msg}");

    daemon.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(10), daemon.wait()).await;
}

/// A server with no `HostThreadService` (as a daemon that predates it).
struct Unimplemented;

#[tonic::async_trait]
impl pb::host_spaces_service_server::HostSpacesService for Unimplemented {
    async fn get_host_spaces(
        &self,
        _: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn create_host_space(
        &self,
        _: tonic::Request<pb::CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn delete_host_space(
        &self,
        _: tonic::Request<pb::DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn cancel_host_space(
        &self,
        _: tonic::Request<pb::CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CancelHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn set_host_space_power(
        &self,
        _: tonic::Request<pb::SetHostSpacePowerRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpacePowerResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn set_host_warm(
        &self,
        _: tonic::Request<pb::SetHostWarmRequest>,
    ) -> Result<tonic::Response<pb::SetHostWarmResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn delete_cloud_space(
        &self,
        _: tonic::Request<pb::DeleteCloudSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteCloudSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn set_host_space_egress(
        &self,
        _: tonic::Request<pb::SetHostSpaceEgressRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpaceEgressResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn diagnose_host(
        &self,
        _: tonic::Request<pb::DiagnoseHostRequest>,
    ) -> Result<tonic::Response<pb::DiagnoseHostResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
}
