// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Talking to agents without typing every @: a message that names several
//! agents (or `@everyone`) tasks each one the poster may task, a reply to an
//! agent's message addresses it, and an agent wakes another agent only
//! through an explicit @, within a hop limit and a per-Space rate limit.
//! Against the in-process spacesd mock and a temporary cua home: nothing
//! runs, nothing touches the host.

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::wake::{PAUSED_NOTE, WakeLimits};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

struct Fx {
    _srv: MockServer,
    _home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
}

async fn setup() -> Fx {
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
    let mcp = McpServer::new(spaces.clone());
    Fx {
        _srv: srv,
        _home: home,
        spaces,
        space: info.id,
        mcp,
    }
}

type Out = Result<Value, (String, String)>;

async fn call_as(fx: &Fx, caller: &Caller, tool: &str, args: Value) -> Out {
    let out = caller
        .clone()
        .scope(Box::pin(fx.mcp.call(tool, args)))
        .await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

fn relay(account: &str, name: &str, role: &str) -> Caller {
    Caller::Relay {
        account: account.into(),
        name: Some(name.into()),
        role: role.into(),
    }
}

fn owner() -> Caller {
    Caller::Owner
}
fn member() -> Caller {
    relay("acct_mia", "Mia", "shared")
}

fn agent(name: &str, fx: &Fx) -> Caller {
    Caller::Agent {
        name: name.into(),
        space: fx.space.clone(),
    }
}

async fn add(fx: &Fx, name: &str) {
    call_as(
        fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": name, "harness": "claude-code",
               "base_url": "http://127.0.0.1:9/v1"}),
    )
    .await
    .unwrap();
}

async fn post(fx: &Fx, who: &Caller, text: &str) -> Value {
    call_as(
        fx,
        who,
        "thread_post",
        json!({"space": fx.space, "text": text}),
    )
    .await
    .unwrap()["entry"]
        .clone()
}

async fn entries(fx: &Fx) -> Vec<Value> {
    call_as(
        fx,
        &owner(),
        "thread_read",
        json!({"space": fx.space, "limit": 500}),
    )
    .await
    .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

/// Who got a task, in thread order, with the message's copy marker.
async fn tasks(fx: &Fx) -> Vec<String> {
    entries(fx)
        .await
        .iter()
        .filter(|e| !e["task"].is_null())
        .map(|e| e["task"]["to"].as_str().unwrap().to_string())
        .collect()
}

async fn paused_notes(fx: &Fx) -> usize {
    entries(fx)
        .await
        .iter()
        .filter(|e| e["text"] == PAUSED_NOTE)
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_mentions_task_each_agent() {
    let fx = setup().await;
    add(&fx, "ada").await;
    add(&fx, "bo").await;
    add(&fx, "cy").await;
    let e = post(&fx, &owner(), "@ada @bo please review").await;
    assert_eq!(e["task"]["to"], "ada");
    assert_eq!(e["mentions"], json!(["ada", "bo"]));
    assert_eq!(tasks(&fx).await, ["ada", "bo"]);
    // The second task sits on a copy that points at the message, so a client draws one message.
    let all = entries(&fx).await;
    let copy = all.iter().find(|x| x["task"]["to"] == "bo").unwrap();
    assert_eq!(copy["fanout_of"], e["seq"]);
    assert_eq!(copy["text"], e["text"]);
    assert!(e.get("fanout_of").is_none());
    // One mention is still one task, and a repeated mention is not two.
    post(&fx, &owner(), "@cy @cy go").await;
    assert_eq!(tasks(&fx).await, ["ada", "bo", "cy"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn everyone_tasks_each_agent_the_poster_may_task() {
    let fx = setup().await;
    add(&fx, "ada").await;
    add(&fx, "bo").await;
    call_as(
        &fx,
        &owner(),
        "agent_set_rights",
        json!({"name": "bo", "task": "owner"}),
    )
    .await
    .unwrap();
    let before = entries(&fx).await.len();
    // A member may task ada only: bo is skipped without a note.
    post(&fx, &member(), "@everyone status?").await;
    assert_eq!(tasks(&fx).await, ["ada"]);
    assert_eq!(entries(&fx).await.len(), before + 1);
    // The owner tasks both.
    post(&fx, &owner(), "@everyone status?").await;
    assert_eq!(tasks(&fx).await, ["ada", "ada", "bo"]);
    // Naming an agent the poster may not task still says why.
    post(&fx, &member(), "@bo status?").await;
    let all = entries(&fx).await;
    assert!(
        all.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("can't give bo tasks")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_to_an_agent_addresses_it() {
    let fx = setup().await;
    add(&fx, "ada").await;
    add(&fx, "bo").await;
    let said = post(&fx, &agent("ada", &fx), "I found two failures").await;
    assert!(said["task"].is_null());
    let r = call_as(
        &fx,
        &owner(),
        "thread_post",
        json!({"space": fx.space, "text": "fix the first", "reply_to": said["seq"]}),
    )
    .await
    .unwrap();
    assert_eq!(r["entry"]["task"]["to"], "ada");
    // A plain message wakes nobody; an explicit mention wins over the reply.
    let plain = post(&fx, &owner(), "thanks all").await;
    assert!(plain["task"].is_null());
    let r = call_as(
        &fx,
        &owner(),
        "thread_post",
        json!({"space": fx.space, "text": "@bo and you?", "reply_to": said["seq"]}),
    )
    .await
    .unwrap();
    assert_eq!(r["entry"]["task"]["to"], "bo");
    assert_eq!(tasks(&fx).await, ["ada", "bo"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_wakes_only_through_an_explicit_mention_and_never_itself() {
    let fx = setup().await;
    add(&fx, "ada").await;
    add(&fx, "bo").await;
    let ada = agent("ada", &fx);
    assert!(post(&fx, &ada, "done").await["task"].is_null());
    // `@everyone` from an agent wakes nobody.
    assert!(post(&fx, &ada, "@everyone look").await["task"].is_null());
    // Self mention is guarded; the other agent is woken.
    let e = post(&fx, &ada, "@ada @bo take a look").await;
    assert_eq!(e["task"]["to"], "bo");
    assert_eq!(e["hops"], 1);
    assert_eq!(tasks(&fx).await, ["bo"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chain_stops_after_three_hops_and_a_person_resets_it() {
    let fx = setup().await;
    add(&fx, "ada").await;
    add(&fx, "bo").await;
    let (ada, bo) = (agent("ada", &fx), agent("bo", &fx));
    let t = post(&fx, &owner(), "@ada start").await;
    assert!(t.get("hops").is_none());
    assert_eq!(post(&fx, &ada, "@bo one").await["hops"], 1);
    assert_eq!(post(&fx, &bo, "@ada two").await["hops"], 2);
    assert_eq!(post(&fx, &ada, "@bo three").await["hops"], 3);
    assert_eq!(paused_notes(&fx).await, 0);
    // The fourth hop is posted but wakes nobody; the thread says so once.
    let over = post(&fx, &bo, "@ada four").await;
    assert!(over["task"].is_null());
    assert_eq!(over["mentions"], json!(["ada"]));
    assert_eq!(paused_notes(&fx).await, 1);
    assert!(post(&fx, &bo, "@ada five").await["task"].is_null());
    assert_eq!(paused_notes(&fx).await, 1, "one note per episode");
    assert_eq!(tasks(&fx).await.len(), 4);
    // A person's message starts the chain over, and a later trip says so again.
    let again = post(&fx, &owner(), "@ada carry on").await;
    assert_eq!(again["task"]["to"], "ada");
    assert_eq!(post(&fx, &ada, "@bo one").await["hops"], 1);
    assert_eq!(post(&fx, &bo, "@ada two").await["hops"], 2);
    assert_eq!(post(&fx, &ada, "@bo three").await["hops"], 3);
    assert!(post(&fx, &bo, "@ada four").await["task"].is_null());
    assert_eq!(paused_notes(&fx).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_allows_a_limited_number_of_agent_wakes_a_minute() {
    let fx = setup().await;
    add(&fx, "ada").await;
    add(&fx, "bo").await;
    let ada = agent("ada", &fx);
    fx.spaces
        .persistent()
        .set_wake_limits(WakeLimits {
            max_hops: 100,
            per_minute: 3,
        })
        .await;
    for i in 0..3 {
        assert_eq!(
            post(&fx, &ada, &format!("@bo {i}")).await["task"]["to"],
            "bo"
        );
    }
    assert!(post(&fx, &ada, "@bo 3").await["task"].is_null());
    assert!(post(&fx, &ada, "@bo 4").await["task"].is_null());
    assert_eq!(paused_notes(&fx).await, 1);
    // A person is not rate limited here.
    assert_eq!(post(&fx, &owner(), "@bo hello").await["task"]["to"], "bo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_limits_are_three_hops_and_twenty_wakes_a_minute() {
    let l = WakeLimits::default();
    assert_eq!((l.max_hops, l.per_minute), (3, 20));
}
