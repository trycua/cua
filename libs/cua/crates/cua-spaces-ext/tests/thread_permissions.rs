// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who can task, control and approve each agent member; guests read only;
//! authors come from the authenticated caller on every path; a task someone
//! else gives says whose keys it runs on. Against the in-process spacesd
//! mock and a temporary cua home (and, for the transport, a real daemon on
//! a temporary socket): nothing runs in a guest, nothing touches the host.

#[cfg(unix)]
use std::time::Duration;

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{Persistent, bridge};
use cua_spaces_ext::thread::policy::GUEST_READ_ONLY;
use cua_spacesd_client::Command;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";
const RUN: &str = "run-00000001";

struct Fx {
    srv: MockServer,
    home: tempfile::TempDir,
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
        srv,
        home,
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
fn admin() -> Caller {
    relay("acct_ad", "Ana", "admin")
}
fn member() -> Caller {
    relay("acct_mia", "Mia", "shared")
}
fn guest() -> Caller {
    relay("acct_gus", "Gus", "viewer")
}

fn edit_record(fx: &Fx, name: &str, f: impl FnOnce(&mut Value)) {
    let path = fx.home.path().join("persistent").join("agents.json");
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let rec = all.iter_mut().find(|r| r["name"] == name).unwrap();
    f(rec);
    std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
}

fn run_state(fx: &Fx, state: Value) {
    let meta = json!({"run_id": RUN, "harness": "claude-code", "prompt": "hi",
                      "cwd": "/home/cua/work", "created_at": 0.0});
    fx.srv.state.put_file(
        &format!("{RUNS}/{RUN}/meta.json"),
        meta.to_string().into_bytes(),
    );
    fx.srv.state.put_file(
        &format!("{RUNS}/{RUN}/state.json"),
        state.to_string().into_bytes(),
    );
}

fn events(fx: &Fx, evs: &[(&str, u32)]) {
    let path = format!("{RUNS}/{RUN}/events.jsonl");
    let mut log = String::new();
    for (i, (kind, turn)) in evs.iter().enumerate() {
        let mut e = json!({"seq": i + 1, "ts": 0, "turn": turn, "type": kind});
        if *kind == "turn_ended" {
            e["stopReason"] = json!("end_turn");
        }
        log.push_str(&e.to_string());
        log.push('\n');
    }
    fx.srv.state.put_file(&path, log.into_bytes());
}

/// Prompts the runner was handed, in order.
fn inbox(fx: &Fx) -> Vec<String> {
    let prefix = format!("{RUNS}/{RUN}/inbox/");
    let mut files: Vec<(String, Vec<u8>)> = fx
        .srv
        .state
        .files()
        .into_iter()
        .filter(|(p, _)| p.starts_with(&prefix))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|(_, b)| {
            serde_json::from_slice::<Value>(&b).unwrap()["text"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// `ada` (forwards the owner's Anthropic key) with an idle run at turn 1,
/// and `bo` (its own endpoint, no keys of the owner).
async fn members(fx: &Fx) -> (Persistent, cua_spacesd_client::ProcessHandle) {
    call_as(
        fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code",
               "env_from_host": ["ANTHROPIC_API_KEY"]}),
    )
    .await
    .unwrap();
    call_as(
        fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": "bo", "harness": "claude-code",
               "base_url": "http://127.0.0.1:9/v1"}),
    )
    .await
    .unwrap();
    edit_record(fx, "ada", |r| r["run_id"] = json!(RUN));
    run_state(fx, json!({"status": "idle", "turn": 1}));
    events(fx, &[("turn_started", 1), ("turn_ended", 1)]);
    let s = fx.spaces.space(&fx.space).await.unwrap();
    let proc = s
        .spacesd()
        .unwrap()
        .spawn(
            Command::new("sleep")
                .arg("600000")
                .tag(format!("cua-agent/{RUN}")),
        )
        .await
        .unwrap();
    edit_record(fx, "ada", |r| {
        r["saved_turn"] = json!(1);
        r["cursor"] = json!(2);
    });
    (fx.spaces.persistent(), proc)
}

fn denied(r: Out) -> String {
    let (kind, msg) = r.unwrap_err();
    assert_eq!(kind, "permission_denied", "{msg}");
    msg
}

async fn post(fx: &Fx, who: &Caller, text: &str) -> Out {
    call_as(
        fx,
        who,
        "thread_post",
        json!({"space": fx.space, "text": text}),
    )
    .await
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

/// The approval `ada` posts through its bridge.
async fn ada_asks(fx: &Fx, p: &Persistent) -> u64 {
    let rec = p.get("ada").unwrap();
    let server = bridge::server(&fx.spaces, p.clone(), &rec, RUN);
    let reply = server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "thread_post", "arguments": {"text": "may I buy it?",
                "approval": {"action": "buy", "details": "one license"}}}}))
        .await
        .unwrap();
    let posted: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(posted["entry"]["kind"], "approval");
    posted["entry"]["seq"].as_u64().unwrap()
}

async fn answer(fx: &Fx, who: &Caller, seq: u64) -> Out {
    call_as(
        fx,
        who,
        "thread_post",
        json!({"space": fx.space, "text": "ok",
               "answer": {"approval": seq, "approve": true}}),
    )
    .await
}

// ------------------------------------------------------------ the matrix

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_task_and_approve_by_role() {
    let fx = setup().await;
    let (p, _proc) = members(&fx).await;

    // read: everyone, guests included, with what each can do.
    for (who, can, role) in [
        (owner(), [true, true, true], "owner"),
        (admin(), [true, true, true], "admin"),
        (member(), [true, false, false], "member"),
        (guest(), [false, false, false], "guest"),
    ] {
        let t = call_as(&fx, &who, "thread_read", json!({"space": fx.space}))
            .await
            .unwrap();
        assert_eq!(t["role"], role);
        call_as(&fx, &who, "thread_inbox", json!({"space": fx.space}))
            .await
            .unwrap();
        let m = call_as(&fx, &who, "agent_members", json!({"space": fx.space}))
            .await
            .unwrap();
        let ada = m["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "ada")
            .unwrap()
            .clone();
        assert_eq!(
            ada["you_can"],
            json!({"task": can[0], "control": can[1], "approve": can[2]}),
            "{who:?}"
        );
        assert_eq!(
            ada["rights"],
            json!({"task": "members", "control": "admins", "approve": "admins"})
        );
    }

    // task: an @mention by the owner, an admin or a member is a task;
    // only a task someone else gave says whose keys it runs on.
    for who in [owner(), admin(), member()] {
        let e = post(&fx, &who, "@ada check the build").await.unwrap();
        assert_eq!(e["entry"]["task"]["to"], "ada", "{who:?}");
        let keys = &e["entry"]["runs_on_keys_of"];
        if who == owner() {
            assert!(keys.is_null());
        } else {
            assert_eq!(*keys, json!({"id": "owner", "name": "Owner"}));
        }
    }
    // An agent with its own endpoint does not run on the owner's keys.
    let e = post(&fx, &member(), "@bo look").await.unwrap();
    assert_eq!(e["entry"]["task"]["to"], "bo");
    assert!(e["entry"].get("runs_on_keys_of").is_none());
    // A guest posts nothing: the exact refusal, and nothing written.
    let before = entries(&fx).await.len();
    assert_eq!(
        denied(post(&fx, &guest(), "@ada hi").await),
        GUEST_READ_ONLY
    );
    assert_eq!(entries(&fx).await.len(), before);
    // agent_send is a task too.
    denied(
        call_as(
            &fx,
            &guest(),
            "agent_send",
            json!({"name": "ada", "text": "hi"}),
        )
        .await,
    );

    // approve: owners and admins answer ada's approvals; members and
    // guests are refused with the exact reason.
    let a1 = ada_asks(&fx, &p).await;
    assert_eq!(
        denied(answer(&fx, &member(), a1).await),
        "you can't answer ada's approval requests: only the owner and admins can"
    );
    assert_eq!(denied(answer(&fx, &guest(), a1).await), GUEST_READ_ONLY);
    let ok = answer(&fx, &admin(), a1).await.unwrap();
    assert_eq!(ok["entry"]["answer"]["approval"], a1);
    assert_eq!(ok["entry"]["author"]["id"], "acct_ad");
    let a2 = ada_asks(&fx, &p).await;
    answer(&fx, &owner(), a2).await.unwrap();
    // An agent never answers approvals, not even its own.
    let a3 = ada_asks(&fx, &p).await;
    let rec = p.get("ada").unwrap();
    let server = bridge::server(&fx.spaces, p.clone(), &rec, RUN);
    let reply = server
        .handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "thread_post", "arguments": {"text": "yes",
                "answer": {"approval": a3, "approve": true}}}}))
        .await
        .unwrap();
    // (The bridge's thread_post takes no `answer`: it is only a message.)
    let posted: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(posted["entry"]["kind"], "message");
    assert!(posted["entry"].get("answer").is_none());
    let t = p.thread_store(&fx.space).fold().unwrap();
    assert_eq!(t.approvals.get(&a3), Some(&false), "still open");
    // Through the daemon's own tools an agent caller is refused too.
    let agent = Caller::Agent {
        name: "ada".into(),
        space: fx.space.clone(),
    };
    assert_eq!(
        denied(answer(&fx, &agent, a3).await),
        "you can't answer ada's approval requests: only the owner and admins can"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_is_taking_control_and_holds_the_agent() {
    let fx = setup().await;
    let (p, _proc) = members(&fx).await;
    let interrupt = |who: Caller| {
        let fx = &fx;
        async move {
            call_as(
                fx,
                &who,
                "agent_interrupt",
                json!({"space": fx.space, "name": "ada"}),
            )
            .await
        }
    };
    // A member who did not ask for the current task, and a guest: refused.
    assert_eq!(
        denied(interrupt(member()).await),
        "you can't stop or take control of ada: only the owner and admins can, and whoever asked for its current task"
    );
    assert_eq!(denied(interrupt(guest()).await), GUEST_READ_ONLY);
    for tool in ["agent_pause", "agent_resume"] {
        assert!(
            denied(call_as(&fx, &member(), tool, json!({"name": "ada"})).await)
                .starts_with("you can't stop or take control of ada")
        );
    }
    // Reading the run is open to a guest.
    call_as(
        &fx,
        &guest(),
        "agent_status",
        json!({"space": fx.space, "name": "ada"}),
    )
    .await
    .unwrap();
    assert!(p.get("ada").unwrap().held_by.is_none());

    // The member's own task: they may stop it.
    let e = post(&fx, &member(), "@ada long job").await.unwrap();
    let task = e["entry"]["seq"].as_u64().unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(p.get("ada").unwrap().task.unwrap().seq, task);
    interrupt(member()).await.unwrap();
    let rec = p.get("ada").unwrap();
    assert_eq!(rec.held_by.unwrap().id, "acct_mia");
    // Its task went back to the queue, and the thread says who.
    assert!(rec.task.is_none());
    let all = entries(&fx).await;
    let note = all
        .iter()
        .rev()
        .find(|e| e["author"]["id"] == "system:control")
        .unwrap();
    assert_eq!(
        note["text"],
        "Mia took control of ada. ada waits until control is given back."
    );
    assert_eq!(note["task_seq"], task);
    let m = call_as(&fx, &owner(), "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    let ada = &m["members"][0];
    assert_eq!(ada["name"], "ada");
    assert_eq!(
        ada["status"],
        json!({"state": "paused", "activity": "Mia has control"})
    );
    assert_eq!(ada["held_by"]["name"], "Mia");

    // Held: nothing is delivered, even to the owner.
    run_state(&fx, json!({"status": "idle", "turn": 1}));
    let before = inbox(&fx).len();
    post(&fx, &owner(), "@ada another").await.unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(inbox(&fx).len(), before);

    // An admin gives control back (agent_resume): delivery starts again.
    call_as(&fx, &admin(), "agent_resume", json!({"name": "ada"}))
        .await
        .unwrap();
    assert!(p.get("ada").unwrap().held_by.is_none());
    let all = entries(&fx).await;
    assert!(
        all.iter()
            .any(|e| e["text"] == "Ana gave control back to ada.")
    );
    Box::pin(p.tick()).await;
    assert_eq!(inbox(&fx).len(), before + 1);

    // An admin may take control of any task.
    run_state(&fx, json!({"status": "running", "turn": 2}));
    interrupt(admin()).await.unwrap();
    assert_eq!(p.get("ada").unwrap().held_by.unwrap().id, "acct_ad");
}

/// The agent's own owner changes its rights; another member can't, and the
/// refusal says who can.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agents_owner_sets_its_rights_and_another_member_cannot() {
    let fx = setup().await;
    let (_p, _proc) = members(&fx).await;
    edit_record(&fx, "bo", |r| {
        let mia = json!({"id": "acct_mia", "name": "Mia"});
        r["added_by"] = json!({"kind": "person", "id": "acct_mia", "name": "Mia"});
        r["identity"]["owner"] = mia.clone();
        r["identity"]["joined"]["by"] = mia;
    });
    // Mia owns bo: she can change who may stop it, and the thread says so.
    let out = call_as(
        &fx,
        &member(),
        "agent_set_rights",
        json!({"name": "bo", "control": "members"}),
    )
    .await
    .unwrap();
    assert_eq!(out["rights"]["control"], "members");
    assert_eq!(out["you_can"]["control"], true);
    let all = entries(&fx).await;
    assert_eq!(
        all.last().unwrap()["text"],
        "Mia changed who can use bo. control: the owner and members"
    );
    // She does not own ada, and another member owns neither.
    let msg = denied(
        call_as(
            &fx,
            &member(),
            "agent_set_rights",
            json!({"name": "ada", "control": "members"}),
        )
        .await,
    );
    assert_eq!(
        msg,
        "only the owner, admins and ada's owner change who can task, stop and approve ada"
    );
    let other = relay("acct_zed", "Zed", "shared");
    denied(
        call_as(
            &fx,
            &other,
            "agent_set_rights",
            json!({"name": "bo", "task": "owner"}),
        )
        .await,
    );
    // Guests are refused at the door.
    denied(
        call_as(
            &fx,
            &guest(),
            "agent_set_rights",
            json!({"name": "bo", "task": "owner"}),
        )
        .await,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rights_are_set_by_owners_and_admins_and_a_refused_mention_gives_no_task() {
    let fx = setup().await;
    let (_p, _proc) = members(&fx).await;
    // Members and guests can't change rights.
    for who in [member(), guest()] {
        denied(
            call_as(
                &fx,
                &who,
                "agent_set_rights",
                json!({"name": "ada", "task": "owner"}),
            )
            .await,
        );
    }
    // Bad values are refused before anything changes.
    let e = call_as(
        &fx,
        &owner(),
        "agent_set_rights",
        json!({"name": "ada", "task": "everyone"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    // An admin makes ada the owner's to task.
    let out = call_as(
        &fx,
        &admin(),
        "agent_set_rights",
        json!({"name": "ada", "task": "owner"}),
    )
    .await
    .unwrap();
    assert_eq!(out["rights"]["task"], "owner");
    assert_eq!(out["you_can"]["task"], false);
    let all = entries(&fx).await;
    assert_eq!(
        all.last().unwrap()["text"],
        "Ana changed who can use ada. task: the owner"
    );

    // A member's mention is posted, but makes no task, and a note says why.
    let e = post(&fx, &member(), "@ada please").await.unwrap();
    assert!(e["entry"]["task"].is_null());
    assert_eq!(e["entry"]["mentions"], json!(["ada"]));
    let all = entries(&fx).await;
    let note = all.last().unwrap();
    assert_eq!(note["author"]["id"], "system:rights");
    assert_eq!(
        note["text"],
        "Mia can't give ada tasks, so no task was made. Only the owner can."
    );
    // agent_send and agent_message by name are refused outright.
    assert_eq!(
        denied(
            call_as(
                &fx,
                &member(),
                "agent_send",
                json!({"name": "ada", "text": "hi"}),
            )
            .await
        ),
        "you can't give ada tasks: only the owner can"
    );
    denied(
        call_as(
            &fx,
            &member(),
            "agent_message",
            json!({"space": fx.space, "name": "ada", "text": "hi"}),
        )
        .await,
    );

    // Named accounts: Mia may, another member may not.
    call_as(
        &fx,
        &owner(),
        "agent_set_rights",
        json!({"name": "ada", "task": ["acct_mia"], "approve": "members"}),
    )
    .await
    .unwrap();
    let e = post(&fx, &member(), "@ada now").await.unwrap();
    assert_eq!(e["entry"]["task"]["to"], "ada");
    let nia = relay("acct_nia", "Nia", "shared");
    let e = post(&fx, &nia, "@ada me too").await.unwrap();
    assert!(e["entry"]["task"].is_null());

    // Rights on agent_add, checked before the member is created.
    let e = call_as(
        &fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": "cy", "harness": "claude-code",
               "rights": {"control": "nobody"}}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    assert!(fx.spaces.persistent().get("cy").is_err());
    let cy = call_as(
        &fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": "cy", "harness": "claude-code",
               "rights": {"approve": "members"}}),
    )
    .await
    .unwrap();
    assert_eq!(
        cy["rights"],
        json!({"task": "members", "control": "admins", "approve": "members"})
    );
    // Members manage nothing else either.
    for (tool, args) in [
        (
            "agent_add",
            json!({"space": fx.space, "name": "dee", "harness": "claude-code"}),
        ),
        ("agent_remove", json!({"name": "cy"})),
        ("agent_schedule_list", json!({})),
        ("notifications_list", json!({})),
        (
            "thread_import",
            json!({"space": fx.space, "source": "x", "entries": []}),
        ),
    ] {
        denied(call_as(&fx, &member(), tool, args).await);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_on_another_device_is_the_owner() {
    let fx = setup().await;
    let (_p, _proc) = members(&fx).await;
    let me = relay("acct_owner", "Dillon", "owner");
    let e = post(&fx, &me, "@ada go").await.unwrap();
    assert_eq!(e["entry"]["task"]["to"], "ada");
    assert_eq!(
        e["entry"]["author"],
        json!({"kind": "person", "id": "acct_owner", "name": "Dillon"})
    );
    assert!(e["entry"].get("runs_on_keys_of").is_none());
    call_as(
        &fx,
        &me,
        "agent_set_rights",
        json!({"name": "ada", "control": "owner"}),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_of_the_owners_team_is_an_admin() {
    let fx = setup().await;
    let (p, _proc) = members(&fx).await;
    let a1 = ada_asks(&fx, &p).await;
    let lee = relay("acct_lee", "Lee", "shared");
    denied(answer(&fx, &lee, a1).await);
    // The daemon reads the owner's teams: owners and admins only.
    let snapshot: cua_host::spaces_api::TeamsSnapshot = serde_json::from_value(json!({
        "email": "o@example.com", "plan": "teams",
        "teams": [{"org": {"id": "org1", "name": "Team"}, "invites": [],
                   "members": [{"sub": "acct_lee", "email": "lee@x", "role": "admin"},
                               {"sub": "acct_owner", "email": "o@x", "role": "owner"},
                               {"sub": "acct_mia", "email": "mia@x", "role": "member"}]}]
    }))
    .unwrap();
    let admins = cua_spaces_ext::daemon::admins_of(&snapshot);
    assert_eq!(admins.len(), 2);
    assert!(!admins.contains("acct_mia"));
    p.set_team_admins(admins);
    answer(&fx, &lee, a1).await.unwrap();
    let m = call_as(&fx, &lee, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(
        m["members"][0]["you_can"],
        json!({"task": true, "control": true, "approve": true})
    );
    // A team admin who reaches the Space only to watch still only reads.
    let watching = relay("acct_lee", "Lee", "viewer");
    assert_eq!(denied(post(&fx, &watching, "hi").await), GUEST_READ_ONLY);
    // Members stay members.
    let a2 = ada_asks(&fx, &p).await;
    denied(answer(&fx, &member(), a2).await);
}

// ------------------------------------------------------------ compatibility

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_records_and_clients_get_the_defaults() {
    let fx = setup().await;
    let (p, _proc) = members(&fx).await;
    // A record written before rights (no policy.rights) and an old client
    // that sends none: the defaults, and the old fields as they were.
    edit_record(&fx, "ada", |r| {
        r["policy"].as_object_mut().unwrap().remove("rights");
        r.as_object_mut().unwrap().remove("held_by");
    });
    assert_eq!(p.get("ada").unwrap().policy.rights, None);
    let m = call_as(&fx, &owner(), "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(
        m["members"][0]["rights"],
        json!({"task": "members", "control": "admins", "approve": "admins"})
    );
    let out = call_as(
        &fx,
        &owner(),
        "persistent_agent_create",
        json!({"space": fx.space, "name": "old", "agent": "claude-code"}),
    )
    .await
    .unwrap();
    for k in [
        "name",
        "harness",
        "space",
        "model",
        "base_url",
        "env_from_host",
        "paused",
        "space_state",
        "created_ms",
    ] {
        assert!(out.get(k).is_some(), "{k}");
    }
    // Saving the record does not write rights nobody set.
    let raw = std::fs::read_to_string(fx.home.path().join("persistent/agents.json")).unwrap();
    let all: Vec<Value> = serde_json::from_str(&raw).unwrap();
    assert!(all.iter().all(|r| r["policy"].get("rights").is_none()));
    // The owner's own post, as before: a task, no keys line.
    let e = post(&fx, &owner(), "@old hi").await.unwrap();
    assert_eq!(e["entry"]["task"]["to"], "old");
    assert!(e["entry"].get("runs_on_keys_of").is_none());
}

// ------------------------------------------------------------ identity

#[test]
fn the_owner_posts_as_the_signed_in_account() {
    let id = cua_auth::Identity {
        username: Some("dillon".into()),
        email: Some("d@example.com".into()),
        name: Some("Dillon".into()),
        subject: Some("acct_7f".into()),
    };
    let a = cua_spaces_ext::daemon::owner_author(&id).unwrap();
    assert_eq!((a.id.as_str(), a.name.as_str()), ("acct_7f", "Dillon"));
    let only_email = cua_auth::Identity {
        email: Some("ada@example.com".into()),
        subject: Some("acct_1".into()),
        ..Default::default()
    };
    assert_eq!(
        cua_spaces_ext::daemon::owner_author(&only_email)
            .unwrap()
            .name,
        "ada"
    );
    // No account id: no identity (the thread keeps `owner`).
    assert!(cua_spaces_ext::daemon::owner_author(&cua_auth::Identity::default()).is_none());
}

/// Through a real daemon: the socket's peer (the host's cua-spacesd)
/// forwards the relay-verified caller; a caller header a client sets on the
/// loopback listener, and a share-caller header anywhere, are ignored.
#[cfg(unix)]
#[test]
fn a_caller_header_from_a_client_is_ignored() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(16 << 20)
                .enable_all()
                .build()
                .unwrap()
                .block_on(through_the_daemon())
        })
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(unix)]
async fn through_the_daemon() {
    use cua_daemon::client::{DaemonAddress, DaemonClient};
    use cua_daemon::server::{self, ServerConfig};
    use cua_daemon::{Runtime, RuntimeConfig};
    use cua_proto::daemon::v1 as pb;
    use std::sync::Arc;

    // Short: macOS caps Unix socket paths at 104 bytes.
    let root = tempfile::Builder::new()
        .prefix("cuatp")
        .tempdir_in("/tmp")
        .unwrap();
    let home = root.path().to_path_buf();
    // No supervisor, no drive services, no sign-in read, no OS vault.
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var(cua_daemon::server::ENV_DAEMON_PERSISTENT, "0");
        std::env::set_var(cua_spaces_ext::daemon::ENV_DAEMON_DRIVE, "0");
        std::env::set_var(cua_spaces_ext::daemon::ENV_DAEMON_OWNER_IDENTITY, "0");
        std::env::set_var(
            "CUA_CREDENTIAL_STORE",
            format!("test-keychain:{}", home.join("kc").display()),
        );
    }
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
    let space = runtime
        .spaces()
        .add(&env.url(), None, Some("room".into()))
        .await
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
    let carol = r#"{"account":"acct_carol","name":"Carol","role":"shared","via":"relay"}"#;
    let mallory = r#"{"account":"acct_mallory","name":"Mallory","role":"owner","via":"relay"}"#;
    let post = |client: DaemonClient, headers: Vec<(&'static str, &'static str)>, text: &str| {
        let args = json!({"space": space.id, "text": text}).to_string();
        async move {
            let mut req = tonic::Request::new(pb::CallSpaceToolRequest {
                name: "thread_post".into(),
                arguments_json: args,
            });
            for (k, v) in headers {
                req.metadata_mut().insert(k, v.parse().unwrap());
            }
            let r = client
                .spaces()
                .call_space_tool(req)
                .await
                .unwrap()
                .into_inner();
            assert!(!r.is_error, "{}", r.content_json);
            let content: Value = serde_json::from_str(&r.content_json).unwrap();
            let entry: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
            entry["entry"]["author"].clone()
        }
    };
    let socket = || DaemonClient::new(DaemonAddress::Socket(sock.clone())).unwrap();
    let loopback = || {
        DaemonClient::new(DaemonAddress::Url {
            url: daemon.loopback_url.clone().unwrap(),
            token: daemon.token.clone(),
        })
        .unwrap()
    };
    // The socket: the forwarded, relay-verified caller.
    let a = post(socket(), vec![("x-cua-host-caller", carol)], "from carol").await;
    assert_eq!(
        a,
        json!({"kind": "person", "id": "acct_carol", "name": "Carol"})
    );
    // A share-caller header is never an identity here.
    let a = post(socket(), vec![("x-cua-share-caller", mallory)], "share").await;
    assert_eq!(a["id"], "owner");
    // The loopback listener: whatever the client claims, it is the owner.
    let a = post(
        loopback(),
        vec![
            ("x-cua-host-caller", mallory),
            ("x-cua-share-caller", mallory),
        ],
        "forged",
    )
    .await;
    assert_eq!(a, json!({"kind": "person", "id": "owner", "name": "Owner"}));
    daemon.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(10), daemon.wait()).await;
}
