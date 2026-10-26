// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Agents connected from a computer: member identity, join codes, member
//! keys and the limits around them. Against the in-process spacesd mock and
//! a temporary cua home; no keychain, no network, nothing on the host.

// The relay HTTP clients' futures nest deep enough to pass rustc's default (128).
#![recursion_limit = "256"]

use std::sync::Arc;
use std::time::Duration;

use cua_host::device::{DeviceKey, member_message};
use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::join::{CODE_REFUSED, KEY_REVOKED, OTHER_ACCOUNT, SIGN_IN_FIRST};
use cua_spaces_ext::thread::{Author, AuthorKind};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

struct Fx {
    _srv: MockServer,
    home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
}

fn ada() -> Author {
    Author {
        kind: AuthorKind::Person,
        id: "acct_ada".into(),
        name: "Ada".into(),
        identity: None,
    }
}

/// A runtime whose owner is `owner` (signed in) or the default
/// (not signed in).
async fn setup_as(owner: Option<Author>) -> Fx {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let mut ext = cua_spaces_ext::DriveExtension::new(cua_volume::Drive::open_local(home.path()));
    if let Some(o) = owner {
        ext = ext.with_owner(o);
    }
    let spaces = Spaces::builder()
        .home(home.path())
        .extension(Arc::new(ext))
        .build();
    let info = spaces
        .add(
            &format!("http://{}", srv.addr),
            None,
            Some("ubuntu-build".into()),
        )
        .await
        .unwrap();
    let mcp = McpServer::new(spaces.clone());
    Fx {
        _srv: srv,
        home,
        spaces,
        space: info.id,
        mcp,
    }
}

async fn setup() -> Fx {
    setup_as(Some(ada())).await
}

type Out = Result<Value, (String, String)>;

async fn call_as(fx: &Fx, caller: &Caller, tool: &str, args: Value) -> Out {
    let out = caller
        .clone()
        .scope(Box::pin(fx.mcp.call(tool, args)))
        .await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

async fn owner_call(fx: &Fx, tool: &str, args: Value) -> Out {
    call_as(fx, &Caller::Owner, tool, args).await
}

/// A connected agent on this computer: what `cua join` keeps.
#[derive(Clone)]
struct Member {
    member_id: String,
    key: String,
    device: Option<Arc<DeviceKey>>,
}

impl Member {
    fn credential(&self) -> Value {
        let mut c = json!({"member_id": self.member_id, "key": self.key});
        if let Some(d) = &self.device {
            let ts = cua_volume::now_ms();
            c["ts_ms"] = json!(ts);
            c["signature"] = json!(d.sign(&member_message(&self.member_id, ts)).unwrap());
        }
        c
    }
}

/// A member's call: the credential rides in `_member`, whoever carries it.
async fn member_call(fx: &Fx, m: &Member, tool: &str, mut args: Value) -> Out {
    args["_member"] = m.credential();
    owner_call(fx, tool, args).await
}

async fn create_code(fx: &Fx, extra: Value) -> Value {
    let mut args = json!({"space": fx.space});
    for (k, v) in extra.as_object().unwrap() {
        args[k] = v.clone();
    }
    owner_call(fx, "thread_join_code_create", args)
        .await
        .unwrap()
}

fn device() -> Value {
    json!({"id": "dev_0123456789abcdef01234567", "name": "Ada's MacBook Pro"})
}

async fn redeem(fx: &Fx, code: &str) -> Out {
    owner_call(
        fx,
        "thread_join_redeem",
        json!({"code": code, "device": device()}),
    )
    .await
}

async fn join(fx: &Fx) -> (Member, Value) {
    let c = create_code(fx, json!({})).await;
    let r = redeem(fx, c["code"].as_str().unwrap()).await.unwrap();
    (
        Member {
            member_id: r["member_id"].as_str().unwrap().into(),
            key: r["key"].as_str().unwrap().into(),
            device: None,
        },
        r,
    )
}

async fn entries(fx: &Fx) -> Vec<Value> {
    owner_call(fx, "thread_read", json!({"space": fx.space, "limit": 500}))
        .await
        .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

async fn codes(fx: &Fx) -> Vec<Value> {
    owner_call(fx, "thread_join_code_list", json!({"space": fx.space}))
        .await
        .unwrap()["codes"]
        .as_array()
        .unwrap()
        .clone()
}

async fn members(fx: &Fx) -> Vec<Value> {
    owner_call(fx, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap()["members"]
        .as_array()
        .unwrap()
        .clone()
}

fn codes_file(fx: &Fx) -> std::path::PathBuf {
    fx.home.path().join("persistent").join("join-codes.json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_code_is_one_line_works_once_and_is_stored_hashed() {
    let fx = setup().await;
    let c = create_code(&fx, json!({})).await;
    let code = c["code"].as_str().unwrap();
    assert_eq!(c["url"], format!("https://cua.ai/join/{code}"));
    assert_eq!(
        c["line"],
        format!("Join my Cua Space \"ubuntu-build\": https://cua.ai/join/{code}")
    );
    let ttl = c["expires_at_ms"].as_u64().unwrap() - cua_volume::now_ms();
    assert!((3_500_000..=3_600_000).contains(&ttl), "{ttl}");
    // Only the hash is kept.
    let file = std::fs::read_to_string(codes_file(&fx)).unwrap();
    assert!(!file.contains(code) && !file.contains(&code.replace('-', "")));
    assert_eq!(codes(&fx).await[0]["state"], "open");

    // The link form redeems too, once.
    let r = redeem(&fx, &format!("https://cua.ai/join/{}", code.to_lowercase()))
        .await
        .unwrap();
    assert_eq!(r["state"], "active");
    assert_eq!(r["space_name"], "ubuntu-build");
    assert_eq!(r["space_owner"]["name"], "Ada");
    assert_eq!(r["device"]["name"], "Ada's MacBook Pro");
    assert!(r["key"].as_str().unwrap().starts_with("cuamk_"));
    let again = redeem(&fx, code).await.unwrap_err();
    assert_eq!(again, ("join_code_refused".into(), CODE_REFUSED.into()));
    let listed = codes(&fx).await;
    assert_eq!(listed[0]["state"], "redeemed");
    assert_eq!(listed[0]["member_handle"], r["handle"]);
    // The key is never in the record, the list or the thread.
    let agents = std::fs::read_to_string(fx.home.path().join("persistent/agents.json")).unwrap();
    assert!(!agents.contains(r["key"].as_str().unwrap()));
    let read = serde_json::to_string(&entries(&fx).await).unwrap();
    assert!(!read.contains(r["key"].as_str().unwrap()));
    // The join note names the owner and the computer.
    let notes = entries(&fx).await;
    let joined = notes
        .iter()
        .find(|e| e["event"]["kind"] == "member_joined")
        .unwrap();
    assert_eq!(joined["text"], "Ada's agent joined from Ada's MacBook Pro.");
    assert_eq!(joined["event"]["member"]["owner"]["id"], "acct_ada");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ttl_bounds_cancel_and_expiry() {
    let fx = setup().await;
    for bad in [899, 86_401] {
        let e = owner_call(
            &fx,
            "thread_join_code_create",
            json!({"space": fx.space, "ttl_s": bad}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, "invalid_argument");
    }
    let bad = owner_call(
        &fx,
        "thread_join_code_create",
        json!({"space": fx.space, "can_task": "everyone"}),
    )
    .await
    .unwrap_err();
    assert_eq!(bad.0, "invalid_argument");
    // Cancelled: refused like any other dead code.
    let c = create_code(&fx, json!({"ttl_s": 900})).await;
    let out = owner_call(
        &fx,
        "thread_join_code_cancel",
        json!({"space": fx.space, "id": c["id"]}),
    )
    .await
    .unwrap();
    assert_eq!(out["state"], "cancelled");
    assert_eq!(
        redeem(&fx, c["code"].as_str().unwrap())
            .await
            .unwrap_err()
            .1,
        CODE_REFUSED
    );
    // Expired.
    let c = create_code(&fx, json!({"ttl_s": 86_400})).await;
    let mut all: Vec<Value> =
        serde_json::from_slice(&std::fs::read(codes_file(&fx)).unwrap()).unwrap();
    for x in &mut all {
        if x["id"] == c["id"] {
            x["expires_ms"] = json!(cua_volume::now_ms() - 1);
        }
    }
    std::fs::write(codes_file(&fx), serde_json::to_vec(&all).unwrap()).unwrap();
    let states: Vec<String> = codes(&fx)
        .await
        .iter()
        .map(|c| c["state"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(states, ["expired", "cancelled"]);
    assert_eq!(
        redeem(&fx, c["code"].as_str().unwrap())
            .await
            .unwrap_err()
            .1,
        CODE_REFUSED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redeeming_needs_a_sign_in_and_the_same_account() {
    let fx = setup_as(None).await;
    let c = create_code(&fx, json!({})).await;
    let e = redeem(&fx, c["code"].as_str().unwrap()).await.unwrap_err();
    assert_eq!(e, ("not_signed_in".into(), SIGN_IN_FIRST.into()));
    // Still open: the refusal used nothing.
    assert_eq!(codes(&fx).await[0]["state"], "open");

    // Another account (a teammate, through the relay) can't redeem the
    // owner's code yet.
    let fx = setup().await;
    let c = create_code(&fx, json!({})).await;
    let grace = Caller::Relay {
        account: "acct_grace".into(),
        name: Some("Grace".into()),
        role: "shared".into(),
    };
    let e = call_as(
        &fx,
        &grace,
        "thread_join_redeem",
        json!({"code": c["code"], "device": device()}),
    )
    .await
    .unwrap_err();
    assert_eq!(e, ("permission_denied".into(), OTHER_ACCOUNT.into()));
    // A guest can't even try.
    let guest = Caller::Relay {
        account: "acct_gus".into(),
        name: None,
        role: "viewer".into(),
    };
    let e = call_as(
        &fx,
        &guest,
        "thread_join_redeem",
        json!({"code": c["code"], "device": device()}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
    assert_eq!(codes(&fx).await[0]["state"], "open");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_guesses_are_limited_per_account() {
    let fx = setup().await;
    let c = create_code(&fx, json!({})).await;
    for _ in 0..5 {
        let e = redeem(&fx, "AAAA-AAAA").await.unwrap_err();
        assert_eq!(e.0, "join_code_refused");
    }
    // Locked out, even with the right code; the code is untouched.
    let e = redeem(&fx, c["code"].as_str().unwrap()).await.unwrap_err();
    assert_eq!(e.0, "rate_limited");
    assert!(e.1.starts_with("Too many wrong codes. Wait "), "{}", e.1);
    assert_eq!(codes(&fx).await[0]["state"], "open");
    // Another account has its own budget (and here gets the other-account
    // line, not a lockout).
    let other = Caller::Relay {
        account: "acct_bob".into(),
        name: None,
        role: "owner".into(),
    };
    let e = call_as(
        &fx,
        &other,
        "thread_join_redeem",
        json!({"code": "BBBB-BBBB", "device": device()}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "join_code_refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn too_many_wrong_codes_on_this_computer_cancel_every_open_code() {
    let fx = setup().await;
    let c = create_code(&fx, json!({})).await;
    for i in 0..30 {
        let who = Caller::Relay {
            account: format!("acct_{i}"),
            name: None,
            role: "owner".into(),
        };
        let _ = call_as(
            &fx,
            &who,
            "thread_join_redeem",
            json!({"code": "ZZZZ-ZZZZ", "device": device()}),
        )
        .await;
    }
    assert_eq!(codes(&fx).await[0]["state"], "cancelled");
    assert_eq!(
        redeem(&fx, c["code"].as_str().unwrap())
            .await
            .unwrap_err()
            .1,
        CODE_REFUSED
    );
    assert!(entries(&fx).await.iter().any(|e| {
        e["text"]
            .as_str()
            .unwrap_or("")
            .contains("too many wrong codes")
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_posts_as_itself_with_owner_and_device_from_its_key() {
    let fx = setup().await;
    let (m, r) = join(&fx).await;
    let joined = member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Codex", "harness": "codex", "version": "0.61.0", "icon": "harness"}),
    )
    .await
    .unwrap();
    assert_eq!(joined["handle"], "codex");
    assert_eq!(
        joined["line"],
        "Joined ubuntu-build as @codex: Ada's agent, posting from Ada's MacBook Pro."
    );
    let id = &joined["member"]["identity"];
    assert_eq!(id["origin"], "external");
    assert_eq!(id["display_name"], "Codex");
    assert_eq!(
        id["harness"],
        json!({"id": "codex", "version": "0.61.0", "reported": true})
    );
    assert_eq!(id["owner"], json!({"id": "acct_ada", "name": "Ada"}));
    assert_eq!(id["device"]["name"], "Ada's MacBook Pro");
    assert_eq!(id["joined"]["method"], "prompt");
    assert_eq!(id["state"], "active");
    assert_ne!(r["handle"], "codex", "the handle came from the name");

    // The author is stamped from the key; arguments can't say otherwise.
    let post = member_call(
        &fx,
        &m,
        "thread_post",
        json!({"space": fx.space, "text": "Hi, I'm Codex.", "author": {"id": "acct_ada", "kind": "person"}}),
    )
    .await
    .unwrap();
    let a = &post["entry"]["author"];
    assert_eq!(a["kind"], "agent");
    assert_eq!(a["id"], "agent:codex");
    assert_eq!(a["identity"]["owner"]["id"], "acct_ada");
    assert_eq!(a["identity"]["device"]["id"], device()["id"]);
    // A member reaches only its Space's thread tools.
    for (tool, args) in [
        ("space_bash", json!({"space": fx.space, "command": "id"})),
        (
            "agent_add",
            json!({"space": fx.space, "name": "x", "agent": "claude-code"}),
        ),
        ("thread_join_code_create", json!({"space": fx.space})),
        ("agent_remove", json!({"name": "codex"})),
        ("volume_ls", json!({})),
    ] {
        let e = member_call(&fx, &m, tool, args).await.unwrap_err();
        assert_eq!(e.0, "permission_denied", "{tool}: {}", e.1);
    }
    // Tasks for it wait in its inbox (no run is started).
    owner_call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "@codex run the tests"}),
    )
    .await
    .unwrap();
    let inbox = member_call(&fx, &m, "thread_inbox", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(inbox["tasks"][0]["to"], "codex");
    let p = fx.spaces.persistent();
    // The dispatcher leaves a connected agent's task for its inbox.
    p.tick().await;
    let inbox = member_call(&fx, &m, "thread_inbox", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(inbox["tasks"][0]["state"], "queued");
    // It replies and closes the task itself.
    let seq = inbox["tasks"][0]["seq"].clone();
    let out = member_call(
        &fx,
        &m,
        "thread_reply",
        json!({"space": fx.space, "task": seq, "text": "All green."}),
    )
    .await
    .unwrap();
    assert_eq!(out["reply"]["author"]["id"], "agent:codex");
    assert_eq!(out["update"]["update"]["state"], "done");
    // It is never a persistent agent, and can't be run in the Space.
    let list = owner_call(&fx, "persistent_agent_list", json!({}))
        .await
        .unwrap();
    assert_eq!(list["agents"], json!([]));
    let e = p.send("codex", "hi").await.unwrap_err();
    assert!(
        e.to_string()
            .contains("is connected from Ada's MacBook Pro"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_members_key_stops_at_once() {
    let fx = setup().await;
    let (m, _) = join(&fx).await;
    member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Hermes"}),
    )
    .await
    .unwrap();
    member_call(
        &fx,
        &m,
        "thread_post",
        json!({"space": fx.space, "text": "here"}),
    )
    .await
    .unwrap();
    // A wrong key never worked.
    let mut bad = m.clone();
    bad.key = cua_spaces_ext::persistent::identity::new_key();
    let e = member_call(&fx, &bad, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap_err();
    assert_eq!(e, ("permission_denied".into(), KEY_REVOKED.into()));
    owner_call(&fx, "agent_remove", json!({"name": "hermes"}))
        .await
        .unwrap();
    for tool in [
        "thread_read",
        "thread_post",
        "thread_inbox",
        "agent_members",
    ] {
        let e = member_call(&fx, &m, tool, json!({"space": fx.space, "text": "x"}))
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ("permission_denied".into(), KEY_REVOKED.into()),
            "{tool}"
        );
    }
    // What it posted stays, with its mark and owner; the note says who.
    let all = entries(&fx).await;
    let post = all.iter().find(|e| e["text"] == "here").unwrap();
    assert_eq!(post["author"]["identity"]["owner"]["name"], "Ada");
    let note = all
        .iter()
        .find(|e| e["event"]["kind"] == "member_removed")
        .unwrap();
    assert_eq!(note["event"]["by"]["id"], "acct_ada");
    assert_eq!(
        note["text"],
        "Ada removed Hermes (Ada's agent). Its key no longer works."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_bound_to_a_device_key_needs_its_signature() {
    let fx = setup().await;
    let (dk, _) = DeviceKey::generate().unwrap();
    let dk = Arc::new(dk);
    let c = create_code(&fx, json!({})).await;
    // A public key that isn't the named device's is refused.
    let (other, _) = DeviceKey::generate().unwrap();
    let e = owner_call(
        &fx,
        "thread_join_redeem",
        json!({"code": c["code"], "device": {"id": dk.id(), "name": "Mac", "public_key": other.public_key()}}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    let r = owner_call(
        &fx,
        "thread_join_redeem",
        json!({"code": c["code"], "device": {"id": dk.id(), "name": "Mac", "public_key": dk.public_key()}}),
    )
    .await
    .unwrap();
    let signed = Member {
        member_id: r["member_id"].as_str().unwrap().into(),
        key: r["key"].as_str().unwrap().into(),
        device: Some(dk.clone()),
    };
    member_call(&fx, &signed, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    // The key alone (copied off the computer) doesn't work.
    let unsigned = Member {
        device: None,
        ..signed.clone()
    };
    let e = member_call(&fx, &unsigned, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap_err();
    assert_eq!(e.0, "permission_denied");
    // Nor signed by another key, nor an old signature.
    let forged = Member {
        device: Some(Arc::new(other)),
        ..signed.clone()
    };
    assert!(
        member_call(&fx, &forged, "thread_read", json!({"space": fx.space}))
            .await
            .is_err()
    );
    let ts = cua_volume::now_ms() - 10 * 60 * 1000;
    let stale = json!({"space": fx.space, "_member": {
        "member_id": signed.member_id, "key": signed.key, "ts_ms": ts,
        "signature": dk.sign(&member_message(&signed.member_id, ts)).unwrap()}});
    assert!(owner_call(&fx, "thread_read", stale).await.is_err());
    // Removing the device removes its members.
    let out = owner_call(&fx, "thread_device_revoke", json!({"device": dk.id()}))
        .await
        .unwrap();
    assert_eq!(out["removed"].as_array().unwrap().len(), 1);
    assert_eq!(
        member_call(&fx, &signed, "thread_read", json!({"space": fx.space}))
            .await
            .unwrap_err()
            .1,
        KEY_REVOKED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn names_are_checked_and_collisions_marked() {
    let fx = setup().await;
    // A started agent named codex holds that handle.
    owner_call(
        &fx,
        "agent_add",
        json!({"space": fx.space, "name": "codex", "agent": "openai-codex"}),
    )
    .await
    .unwrap();
    owner_call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "hello"}),
    )
    .await
    .unwrap();
    let (m, _) = join(&fx).await;
    for (name, why) in [
        ("", "a name is required"),
        ("Cua", "\"Cua\" is the app's own name"),
        ("ada", "is a person in this Space"),
        ("codex", "is another member's handle"),
        (&"x".repeat(33), "at most 32"),
        ("bad\u{7}name", "control characters"),
    ] {
        let e = member_call(
            &fx,
            &m,
            "thread_join",
            json!({"space": fx.space, "name": name}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, "invalid_argument", "{name:?}");
        assert!(e.1.contains(why), "{name:?}: {}", e.1);
    }
    let refused = entries(&fx)
        .await
        .into_iter()
        .filter(|e| e["event"]["kind"] == "name_refused")
        .count();
    assert_eq!(refused, 6);
    // The same display name as another member is allowed, and marked.
    let joined = member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Codex CLI"}),
    )
    .await
    .unwrap();
    assert_eq!(joined["handle"], "codex-cli");
    let (m2, _) = join(&fx).await;
    let joined2 = member_call(
        &fx,
        &m2,
        "thread_join",
        json!({"space": fx.space, "name": "codex cli", "icon": "https://example.com/i.png"}),
    )
    .await
    .unwrap();
    assert_eq!(joined2["handle"], "codex-cli-2");
    assert_eq!(joined2["member"]["identity"]["collides"], true);
    let rows = members(&fx).await;
    let by = |h: &str| rows.iter().find(|r| r["name"] == h).unwrap()["identity"].clone();
    assert_eq!(by("codex-cli")["collides"], true);
    assert_eq!(by("codex-cli-2")["collides"], true);
    assert_eq!(by("codex")["collides"], false);
    assert_eq!(by("codex")["origin"], "space");
    // An unreported harness says so.
    assert_eq!(
        by("codex-cli")["harness"],
        json!({"id": "other", "version": null, "reported": false})
    );
    // Its handle is fixed after the first join; later joins change only how
    // it looks.
    let again = member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Builder"}),
    )
    .await
    .unwrap();
    assert_eq!(again["handle"], "codex-cli");
    assert_eq!(again["member"]["identity"]["display_name"], "Builder");
    // A bad icon is refused.
    let e = member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Builder", "icon": "http://plain"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_from_before_identities_read_as_started_and_upgrade() {
    let fx = setup().await;
    let path = fx.home.path().join("persistent/agents.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = json!([{
        "name": "scout", "harness": "hermes", "space": fx.space,
        "created_ms": 1761000000000u64, "member_since_ms": 1761000000500u64,
        "paused": true, "space_state": "running", "cursor": 0, "saved_turn": 0,
        "notify": true, "saved_ms": 0, "sessions": 1,
        "added_by": {"kind": "person", "id": "acct_grace", "name": "Grace"},
    }]);
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let rows = members(&fx).await;
    assert_eq!(
        rows[0]["identity"],
        json!({
            "origin": "space", "handle": "scout", "display_name": "scout", "collides": false,
            "icon": "harness", "harness": {"id": "hermes", "version": null, "reported": false},
            "owner": {"id": "acct_grace", "name": "Grace"}, "device": null,
            "joined": {"method": "started", "at_ms": 1761000000500u64, "by": {"id": "acct_grace", "name": "Grace"}},
            "state": "active", "last_seen_ms": null,
        })
    );
    // The next save writes it in.
    owner_call(
        &fx,
        "agent_set_rights",
        json!({"name": "scout", "task": "owner"}),
    )
    .await
    .unwrap();
    let saved: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved[0]["identity"]["origin"], "space");
    assert_eq!(saved[0]["identity"]["owner"]["id"], "acct_grace");
    assert_eq!(members(&fx).await[0]["identity"], rows[0]["identity"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_before_join_waits_for_the_owner() {
    let fx = setup().await;
    let c = create_code(&fx, json!({"ask_before_join": true, "can_task": "members"})).await;
    let r = redeem(&fx, c["code"].as_str().unwrap()).await.unwrap();
    assert_eq!(r["state"], "pending");
    let m = Member {
        member_id: r["member_id"].as_str().unwrap().into(),
        key: r["key"].as_str().unwrap().into(),
        device: None,
    };
    assert_eq!(codes(&fx).await[0]["state"], "pending_approval");
    let e = member_call(&fx, &m, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap_err();
    assert!(e.1.contains("waiting for the Space's owner"), "{}", e.1);
    let req = entries(&fx)
        .await
        .into_iter()
        .find(|e| e["kind"] == "join_request")
        .unwrap();
    assert_eq!(req["join_request"]["can_task"], "members");
    assert_eq!(req["join_request"]["member"]["state"], "pending");
    assert_eq!(req["you_can_answer"], true);
    assert_eq!(members(&fx).await[0]["status"]["state"], "pending");
    // A member of the Space can't let it in.
    let mia = Caller::Relay {
        account: "acct_mia".into(),
        name: Some("Mia".into()),
        role: "shared".into(),
    };
    let e = call_as(
        &fx,
        &mia,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": {"approval": req["seq"], "approve": true}}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
    let seen = call_as(&fx, &mia, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let mine = seen["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "join_request")
        .unwrap()
        .clone();
    assert_eq!(mine["you_can_answer"], false);
    owner_call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": {"approval": req["seq"], "approve": true}}),
    )
    .await
    .unwrap();
    member_call(&fx, &m, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(codes(&fx).await[0]["state"], "redeemed");
    let rows = members(&fx).await;
    assert_eq!(rows[0]["identity"]["state"], "active");
    assert_eq!(rows[0]["rights"]["task"], "members");

    // Denied: the member is dropped and the code says so.
    let c = create_code(&fx, json!({"ask_before_join": true})).await;
    let r = redeem(&fx, c["code"].as_str().unwrap()).await.unwrap();
    let req = entries(&fx)
        .await
        .into_iter()
        .rfind(|e| e["kind"] == "join_request")
        .unwrap();
    owner_call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": {"approval": req["seq"], "approve": false}}),
    )
    .await
    .unwrap();
    assert!(members(&fx).await.iter().all(|m| m["name"] != r["handle"]));
    assert_eq!(codes(&fx).await[0]["state"], "denied");

    // Cancelling a pending code closes its request.
    let c = create_code(&fx, json!({"ask_before_join": true})).await;
    let r = redeem(&fx, c["code"].as_str().unwrap()).await.unwrap();
    owner_call(
        &fx,
        "thread_join_code_cancel",
        json!({"space": fx.space, "id": c["id"]}),
    )
    .await
    .unwrap();
    assert!(members(&fx).await.iter().all(|m| m["name"] != r["handle"]));
    let all = entries(&fx).await;
    let last_req = all.iter().rfind(|e| e["kind"] == "join_request").unwrap();
    assert_eq!(last_req["you_can_answer"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_space_setting_and_who_may_change_it() {
    let fx = setup().await;
    let s = owner_call(&fx, "thread_settings", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(s["ask_before_join"], true);
    let mia = Caller::Relay {
        account: "acct_mia".into(),
        name: None,
        role: "shared".into(),
    };
    let e = call_as(
        &fx,
        &mia,
        "thread_settings",
        json!({"space": fx.space, "ask_before_join": false}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
    let s = owner_call(
        &fx,
        "thread_settings",
        json!({"space": fx.space, "ask_before_join": false}),
    )
    .await
    .unwrap();
    assert_eq!(s["ask_before_join"], false);
    // The owner's own codes never ask unless the code says so.
    let (_, r) = join(&fx).await;
    assert_eq!(r["state"], "active");
    // A member's codes: theirs to list and cancel; others' hidden.
    let mine = call_as(
        &fx,
        &mia,
        "thread_join_code_create",
        json!({"space": fx.space}),
    )
    .await
    .unwrap();
    let listed = call_as(
        &fx,
        &mia,
        "thread_join_code_list",
        json!({"space": fx.space}),
    )
    .await
    .unwrap();
    assert_eq!(listed["codes"].as_array().unwrap().len(), 1);
    assert_eq!(listed["codes"][0]["id"], mine["id"]);
    assert_eq!(codes(&fx).await.len(), 2);
    let owners = codes(&fx)
        .await
        .into_iter()
        .find(|c| c["id"] != mine["id"])
        .unwrap();
    let e = call_as(
        &fx,
        &mia,
        "thread_join_code_cancel",
        json!({"space": fx.space, "id": owners["id"]}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn posts_are_limited_with_one_note() {
    let fx = setup().await;
    let (m, _) = join(&fx).await;
    member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Chatty"}),
    )
    .await
    .unwrap();
    for i in 0..30 {
        member_call(
            &fx,
            &m,
            "thread_post",
            json!({"space": fx.space, "text": format!("{i}")}),
        )
        .await
        .unwrap();
    }
    for _ in 0..5 {
        let e = member_call(
            &fx,
            &m,
            "thread_post",
            json!({"space": fx.space, "text": "more"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.0, "rate_limited");
        assert!(e.1.contains("30 posts a minute"), "{}", e.1);
    }
    let notes = entries(&fx)
        .await
        .into_iter()
        .filter(|e| e["event"]["kind"] == "rate_limited")
        .count();
    assert_eq!(notes, 1);
    // People and started agents aren't limited this way.
    for _ in 0..40 {
        owner_call(&fx, "thread_post", json!({"space": fx.space, "text": "ok"}))
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_inbox_waits_for_a_task() {
    let fx = setup().await;
    let (m, _) = join(&fx).await;
    member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Hermes"}),
    )
    .await
    .unwrap();
    let t0 = std::time::Instant::now();
    let empty = member_call(
        &fx,
        &m,
        "thread_inbox",
        json!({"space": fx.space, "wait_ms": 300}),
    )
    .await
    .unwrap();
    assert_eq!(empty["tasks"], json!([]));
    assert!(t0.elapsed() >= Duration::from_millis(280));
    let fx = Arc::new(fx);
    let poster = {
        let fx = fx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            owner_call(
                &fx,
                "thread_post",
                json!({"space": fx.space, "text": "@hermes hi"}),
            )
            .await
            .unwrap();
        })
    };
    let t0 = std::time::Instant::now();
    let got = member_call(
        &fx,
        &m,
        "thread_inbox",
        json!({"space": fx.space, "wait_ms": 20000}),
    )
    .await
    .unwrap();
    poster.await.unwrap();
    assert_eq!(got["tasks"][0]["to"], "hermes");
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approvals_of_a_connected_agent_go_to_its_owner() {
    let fx = setup().await;
    let (m, _) = join(&fx).await;
    member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Builder"}),
    )
    .await
    .unwrap();
    let ask = member_call(
        &fx,
        &m,
        "thread_post",
        json!({"space": fx.space, "text": "may I deploy?", "approval": {"action": "deploy"}}),
    )
    .await
    .unwrap();
    // An admin may not answer (the default for a connected agent is its
    // owner), the owner may.
    let admin = Caller::Relay {
        account: "acct_ana".into(),
        name: Some("Ana".into()),
        role: "admin".into(),
    };
    let e = call_as(
        &fx,
        &admin,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": {"approval": ask["entry"]["seq"], "approve": true}}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
    owner_call(
        &fx,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": {"approval": ask["entry"]["seq"], "approve": true}}),
    )
    .await
    .unwrap();
    // The agent itself can't answer its own approval.
    let ask2 = member_call(
        &fx,
        &m,
        "thread_post",
        json!({"space": fx.space, "text": "again?", "approval": {"action": "deploy"}}),
    )
    .await
    .unwrap();
    let e = member_call(
        &fx,
        &m,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": {"approval": ask2["entry"]["seq"], "approve": true}}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
}

// ---------------------------------------------------------------------------
// Harness matrix rows: the app's op (the tool its bridge op calls), the
// member's MCP server (what `cua mcp --member` serves) and the CLI (its exit
// code for the same answer) report the same state.

fn member_mcp(fx: &Fx, m: &Member) -> McpServer {
    let m = m.clone();
    cua_spaces::mcp::member::member_server(
        Arc::new(fx.mcp.clone()),
        &fx.space,
        "test",
        Arc::new(move || Ok(m.credential())),
    )
}

async fn mcp_out(server: &McpServer, tool: &str, args: Value) -> Out {
    cua_spaces::extension::outcome_json(server.call(tool, args).await)
        .map_err(|e| (e.tag().to_string(), e.to_string()))
}

fn cli_exit(out: &Out) -> i32 {
    match out {
        Ok(_) => 0,
        Err((kind, msg)) => cua_spaces::mcp::member::exit_code(kind, msg),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matrix_joined_with_cua_join_then_removed() {
    use cua_spaces::mcp::member::exit;
    let fx = setup().await;
    let (m, _) = join(&fx).await;
    let mcp = member_mcp(&fx, &m);
    // The member's MCP server serves the thread tools only, pinned to its Space.
    let listed = mcp.tools_list();
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        cua_spaces::mcp::member::MEMBER_MCP_TOOLS
            .iter()
            .copied()
            .collect()
    );
    let joined = mcp_out(
        &mcp,
        "thread_join",
        json!({"space": "elsewhere", "name": "Codex", "harness": "codex"}),
    )
    .await;
    assert_eq!(cli_exit(&joined), 0);
    // Joined: the app sees it, MCP works, the CLI exits 0.
    let app = members(&fx).await;
    assert!(
        app.iter()
            .any(|r| r["name"] == "codex" && r["identity"]["state"] == "active")
    );
    let read = mcp_out(&mcp, "thread_read", json!({})).await;
    assert_eq!(cli_exit(&read), 0);
    assert_eq!(read.unwrap()["role"], "agent");
    // Removed from the app (thread.agentRemove -> agent_remove).
    owner_call(&fx, "agent_remove", json!({"name": "codex"}))
        .await
        .unwrap();
    let app = members(&fx).await;
    assert!(app.iter().all(|r| r["name"] != "codex"));
    for tool in [
        "thread_read",
        "thread_post",
        "thread_inbox",
        "agent_members",
    ] {
        let out = mcp_out(&mcp, tool, json!({"text": "still here?"})).await;
        assert_eq!(out.as_ref().unwrap_err().1, KEY_REVOKED, "{tool}");
        assert_eq!(cli_exit(&out), exit::NOT_A_MEMBER, "{tool}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matrix_code_expired_or_reused() {
    use cua_spaces::mcp::member::exit;
    let fx = setup().await;
    // Reused: the app shows it redeemed by the first agent; the second
    // redemption is refused (CLI exit 4); the first agent's MCP sees one
    // connected member.
    let c = create_code(&fx, json!({})).await;
    let first = redeem(&fx, c["code"].as_str().unwrap()).await.unwrap();
    let again = redeem(&fx, c["code"].as_str().unwrap()).await;
    assert_eq!(cli_exit(&again), exit::CODE_REFUSED);
    assert_eq!(again.unwrap_err().1, CODE_REFUSED);
    let app = codes(&fx).await;
    assert_eq!(app[0]["state"], "redeemed");
    assert_eq!(app[0]["member_handle"], first["handle"]);
    let m = Member {
        member_id: first["member_id"].as_str().unwrap().into(),
        key: first["key"].as_str().unwrap().into(),
        device: None,
    };
    let seen = mcp_out(&member_mcp(&fx, &m), "agent_members", json!({}))
        .await
        .unwrap();
    let external: Vec<&Value> = seen["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["identity"]["origin"] == "external")
        .collect();
    assert_eq!(external.len(), 1);
    assert_eq!(external[0]["name"], first["handle"]);
    // Agents don't see how members are set up.
    assert!(external[0].get("policy").is_none());

    // Expired: the app shows expired, the CLI exits 4, and no member exists
    // for MCP to serve.
    let c = create_code(&fx, json!({})).await;
    let mut all: Vec<Value> =
        serde_json::from_slice(&std::fs::read(codes_file(&fx)).unwrap()).unwrap();
    for x in &mut all {
        if x["id"] == c["id"] {
            x["expires_ms"] = json!(cua_volume::now_ms() - 1);
        }
    }
    std::fs::write(codes_file(&fx), serde_json::to_vec(&all).unwrap()).unwrap();
    assert_eq!(codes(&fx).await[0]["state"], "expired");
    let late = redeem(&fx, c["code"].as_str().unwrap()).await;
    assert_eq!(cli_exit(&late), exit::CODE_REFUSED);
    let seen = mcp_out(&member_mcp(&fx, &m), "agent_members", json!({}))
        .await
        .unwrap();
    assert_eq!(
        seen["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["identity"]["origin"] == "external")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signing_out_removes_the_accounts_connected_agents() {
    let fx = setup().await;
    let (m, _) = join(&fx).await;
    member_call(
        &fx,
        &m,
        "thread_join",
        json!({"space": fx.space, "name": "Hermes"}),
    )
    .await
    .unwrap();
    let gone = fx
        .spaces
        .persistent()
        .drop_members_of(&ada())
        .await
        .unwrap();
    assert_eq!(gone, ["hermes"]);
    assert_eq!(
        member_call(&fx, &m, "thread_read", json!({"space": fx.space}))
            .await
            .unwrap_err()
            .1,
        KEY_REVOKED
    );
    let note = entries(&fx)
        .await
        .into_iter()
        .rfind(|e| e["event"]["kind"] == "member_removed")
        .unwrap();
    assert!(
        note["text"]
            .as_str()
            .unwrap()
            .starts_with("Ada signed out of Cua")
    );
}
