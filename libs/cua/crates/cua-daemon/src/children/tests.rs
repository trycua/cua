//! What a Space's spawn channel answers, attacked from inside the Space: a
//! sibling, an ancestor, another tree, a guessed id, a name that collides
//! with another Space, a replayed or copied capability, and owner calls.
//! In-process: the channel's app on an in-memory stream, no VM.

use std::sync::Arc;

use cua_proto::daemon::v1::{
    self as pb, daemon_service_client::DaemonServiceClient,
    space_service_client::SpaceServiceClient,
};
use cua_spaces::registry::Credential;
use tonic::{Code, Request};

use super::api::LABEL_PARENT;
use crate::server::Shared;
use crate::{Runtime, RuntimeConfig};

struct World {
    _home: tempfile::TempDir,
    shared: Arc<Shared>,
}

/// root ── a ── a1 ("dup")
///      └─ b
/// other ── o1
/// owner-dup ("dup", a Space of the owner's own)
fn world() -> World {
    let home = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(home.path().join("sandboxes")),
        spaces_home: Some(home.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();
    let shared = Shared::for_tests(runtime);
    let spaces = shared.runtime.spaces();
    let add = |id: &str, name: &str, parent: Option<(&str, &str, u32)>| {
        spaces
            .registry()
            .upsert(
                pb::Space {
                    id: id.into(),
                    name: name.into(),
                    ..Default::default()
                },
                Credential::default(),
            )
            .unwrap();
        if let Some((parent, root, depth)) = parent {
            spaces
                .registry()
                .update_meta(id, |m| {
                    m.parent = Some(parent.into());
                    m.root = Some(root.into());
                    m.depth = depth;
                    m.origin = Some("child".into());
                    m.labels.insert(LABEL_PARENT.into(), parent.into());
                })
                .unwrap();
        }
    };
    add("local:root", "root", None);
    add("local:a", "a", Some(("local:root", "local:root", 1)));
    add("local:a1", "dup", Some(("local:a", "local:root", 2)));
    add("local:b", "b", Some(("local:root", "local:root", 1)));
    add("local:other", "other", None);
    add("local:o1", "o1", Some(("local:other", "local:other", 1)));
    add("local:owner-dup", "dup", None);
    World {
        _home: home,
        shared,
    }
}

type Chan = tonic::transport::Channel;

/// A client of `space`'s channel (as the guest dials it).
async fn channel(shared: &Arc<Shared>, space: &str) -> Chan {
    let app = super::channel_app(shared.clone(), space.to_owned());
    tonic::transport::Endpoint::from_static("http://127.0.0.1:41234")
        .connect_with_connector(tower::service_fn(move |_: http::Uri| {
            let app = app.clone();
            async move {
                let (ours, theirs) = tokio::io::duplex(1 << 20);
                tokio::spawn(super::serve_stream(app, theirs));
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(ours))
            }
        }))
        .await
        .unwrap()
}

fn with<T>(token: &str, msg: T) -> Request<T> {
    let mut r = Request::new(msg);
    r.metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    r
}

fn mint(shared: &Shared, space: &str, root: &str, depth: u32) -> String {
    shared
        .spawn_authority()
        .unwrap()
        .mint(space, root, depth, 2, 3600)
}

#[tokio::test]
async fn a_space_sees_only_its_own_subtree() {
    let w = world();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let mut c = SpaceServiceClient::new(channel(&w.shared, "local:a").await);
    let list = c
        .list_spaces(with(&cap, pb::ListSpacesRequest::default()))
        .await
        .unwrap()
        .into_inner();
    let ids: Vec<_> = list.spaces.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["local:a1"]);

    // The root sees its whole tree, and nothing of the other tree or the
    // owner's own Spaces.
    let root_cap = mint(&w.shared, "local:root", "local:root", 0);
    let mut r = SpaceServiceClient::new(channel(&w.shared, "local:root").await);
    let mut ids: Vec<String> = r
        .list_spaces(with(&root_cap, pb::ListSpacesRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .spaces
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["local:a", "local:a1", "local:b"]);
}

#[tokio::test]
async fn siblings_ancestors_other_trees_and_guesses_read_as_missing() {
    let w = world();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let mut c = SpaceServiceClient::new(channel(&w.shared, "local:a").await);
    for target in [
        "local:b",
        "local:root",
        "local:a",
        "local:o1",
        "local:other",
        "local:owner-dup",
        "local:does-not-exist",
        "b",
        "root",
    ] {
        let e = c
            .resolve_space(with(
                &cap,
                pb::ResolveSpaceRequest {
                    space: target.into(),
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Code::NotFound, "{target}");
        assert_eq!(
            e.message(),
            super::api::not_found(target).message(),
            "{target}"
        );
        let e = c
            .delete_space(with(
                &cap,
                pb::DeleteSpaceRequest {
                    space: target.into(),
                    keep_children: false,
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Code::NotFound, "delete {target}");
        let e = c
            .call_space_tool(with(
                &cap,
                pb::CallSpaceToolRequest {
                    name: "space_bash".into(),
                    arguments_json: serde_json::json!({"space": target, "command": "id"})
                        .to_string(),
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Code::NotFound, "space_bash {target}");
        let e = c
            .set_space_lifetime(with(
                &cap,
                pb::SetSpaceLifetimeRequest {
                    space: target.into(),
                    ..Default::default()
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Code::NotFound, "lifetime {target}");
    }
    // Nothing was deleted.
    let left = w.shared.runtime.spaces().registry().list().unwrap().len();
    assert_eq!(left, 7);
}

#[tokio::test]
async fn a_name_shared_with_the_owners_space_means_the_child() {
    let w = world();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let mut c = SpaceServiceClient::new(channel(&w.shared, "local:a").await);
    let s = c
        .resolve_space(with(
            &cap,
            pb::ResolveSpaceRequest {
                space: "dup".into(),
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .space
        .unwrap();
    assert_eq!(s.id, "local:a1");
}

#[tokio::test]
async fn only_this_spaces_live_capability_opens_its_channel() {
    let w = world();
    let mut c = SpaceServiceClient::new(channel(&w.shared, "local:a").await);
    let list = |c: &mut SpaceServiceClient<Chan>, token: Option<String>| {
        let mut c = c.clone();
        async move {
            let mut r = Request::new(pb::ListSpacesRequest::default());
            if let Some(t) = token {
                r.metadata_mut()
                    .insert("authorization", format!("Bearer {t}").parse().unwrap());
            }
            c.list_spaces(r).await.map(|_| ()).map_err(|e| e.code())
        }
    };
    // None, garbage, the owner's daemon token.
    assert_eq!(list(&mut c, None).await, Err(Code::Unauthenticated));
    assert_eq!(
        list(&mut c, Some("nope".into())).await,
        Err(Code::Unauthenticated)
    );
    assert_eq!(
        list(&mut c, Some(w.shared.token.clone())).await,
        Err(Code::Unauthenticated)
    );
    // A sibling's, its parent's, its child's: live capabilities, but of
    // other Spaces (copied out of a snapshot, a fork, a shared folder).
    for (space, root, depth) in [
        ("local:b", "local:root", 1),
        ("local:root", "local:root", 0),
        ("local:a1", "local:root", 2),
    ] {
        let other = mint(&w.shared, space, root, depth);
        assert_eq!(
            list(&mut c, Some(other)).await,
            Err(Code::Unauthenticated),
            "{space}'s capability"
        );
    }
    // Its own, then replaced (renewed, or restored from a snapshot): the old
    // one is dead.
    let old = mint(&w.shared, "local:a", "local:root", 1);
    assert_eq!(list(&mut c, Some(old.clone())).await, Ok(()));
    let new = mint(&w.shared, "local:a", "local:root", 1);
    assert_eq!(list(&mut c, Some(old)).await, Err(Code::Unauthenticated));
    assert_eq!(list(&mut c, Some(new.clone())).await, Ok(()));
    // Revoked (children turned off, the Space deleted).
    w.shared.spawn_authority().unwrap().revoke("local:a");
    assert_eq!(list(&mut c, Some(new)).await, Err(Code::Unauthenticated));
}

#[tokio::test]
async fn owner_calls_are_refused_and_the_daemon_token_never_shows() {
    let w = world();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let chan = channel(&w.shared, "local:a").await;
    let mut c = SpaceServiceClient::new(chan.clone());
    let e = c
        .list_hosts(with(&cap, pb::ListHostsRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
    let e = c
        .add_space(with(&cap, pb::AddSpaceRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
    let e = c
        .set_space_egress(with(&cap, pb::SetSpaceEgressRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
    // Owner tools through the tool call: refused by name.
    for tool in [
        "share_space",
        "cloud_connect",
        "create_space",
        "agent_start",
    ] {
        let e = c
            .call_space_tool(with(
                &cap,
                pb::CallSpaceToolRequest {
                    name: tool.into(),
                    arguments_json: serde_json::json!({"space": "local:a1"}).to_string(),
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Code::PermissionDenied, "{tool}");
    }
    let info = DaemonServiceClient::new(chan)
        .get_info(with(&cap, pb::GetInfoRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert!(info.loopback_token.is_empty() && info.loopback_url.is_empty());
    assert!(info.socket_path.is_empty());
    assert!(info.features.contains(&super::CHILDREN_FEATURE.to_string()));
}

#[tokio::test]
async fn a_create_becomes_a_child_or_a_plain_refusal() {
    let w = world();
    let scoped = super::api::Scoped {
        shared: w.shared.clone(),
        space: "local:a".into(),
    };
    let claims = |depth| super::capability::Claims {
        space: "local:a".into(),
        root: "local:root".into(),
        depth,
        max_depth: 2,
        exp: u64::MAX,
        iat: 0,
        jti: String::new(),
        kid: String::new(),
    };
    let r = scoped
        .child_request(&claims(1), pb::CreateSpaceRequest::default())
        .unwrap();
    assert_eq!(r.location, "local");
    assert_eq!(r.name, "a.c1");
    // On this machine the record's tree fields place it, not labels.
    assert_eq!(r.parent, "local:a");
    assert!(r.labels.is_empty(), "{:?}", r.labels);

    let refuse = |depth, req: pb::CreateSpaceRequest| {
        scoped
            .child_request(&claims(depth), req)
            .unwrap_err()
            .code()
    };
    // Deeper than the tree allows.
    assert_eq!(refuse(2, Default::default()), Code::ResourceExhausted);
    // Another machine or a linked cloud is allowed (inside the budget); an
    // address is not; nor are reuse, durable and volumes.
    assert!(
        scoped
            .child_request(
                &claims(1),
                pb::CreateSpaceRequest {
                    location: "aws".into(),
                    ..Default::default()
                }
            )
            .is_ok()
    );
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                location: "direct:10.0.0.9:7400".into(),
                ..Default::default()
            }
        ),
        Code::FailedPrecondition
    );
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                reuse: true,
                ..Default::default()
            }
        ),
        Code::InvalidArgument
    );
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                durable: true,
                ..Default::default()
            }
        ),
        Code::InvalidArgument
    );
    // A forged tree position.
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                labels: [(LABEL_PARENT.to_string(), "local:other".to_string())].into(),
                ..Default::default()
            }
        ),
        Code::InvalidArgument
    );
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                parent: "local:other".into(),
                ..Default::default()
            }
        ),
        Code::InvalidArgument
    );
    // A network more open than the caller's.
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                widen: true,
                ..Default::default()
            }
        ),
        Code::PermissionDenied
    );
    // Egress: the parent's rules, never wider.
    w.shared
        .runtime
        .spaces()
        .registry()
        .update_meta("local:a", |m| {
            m.egress = cua_spaces::egress::EgressPolicy {
                allow: vec![],
                deny: vec!["0.0.0.0/0".into()],
            }
        })
        .unwrap();
    let r = scoped
        .child_request(&claims(1), pb::CreateSpaceRequest::default())
        .unwrap();
    assert_eq!(r.egress.unwrap().deny, ["0.0.0.0/0"]);
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                egress: Some(pb::EgressPolicy {
                    allow: vec!["0.0.0.0/0".into()],
                    deny: vec![],
                }),
                ..Default::default()
            }
        ),
        Code::PermissionDenied
    );
}

/// From inside a Space with network rules: a run that would escape them (a
/// native job, host services, the owner's cloud) is refused with an
/// actionable reason, on the real channel, and records nothing.
#[tokio::test]
async fn a_restricted_space_cannot_start_a_run_that_escapes_its_rules() {
    let w = world();
    w.shared
        .runtime
        .spaces()
        .registry()
        .update_meta("local:a", |m| {
            m.network = Some(cua_spaces::network::NetworkSettings::none());
        })
        .unwrap();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let mut c = SpaceServiceClient::new(channel(&w.shared, "local:a").await);
    for (what, params) in [
        (
            "a native job",
            serde_json::json!({"kind": "machine", "command": ["x"]}),
        ),
        (
            "an explicit native job",
            serde_json::json!({"jobs": [{"spec": {"kind": "machine", "argv": ["x"]}}]}),
        ),
        (
            "host services",
            serde_json::json!({"services": ["compat:e2b"], "command": ["x"]}),
        ),
        (
            "the owner's cloud",
            serde_json::json!({"on": "modal", "command": ["x"]}),
        ),
        // The Space lies about where it is: the daemon's word wins, so this is
        // still refused as a native job from a closed Space.
        (
            "a forged parent",
            serde_json::json!({"kind": "machine", "env": {"CUA_PARENT_NETWORK": "{}"}, "command": ["x"]}),
        ),
    ] {
        let e = c
            .pool_request(with(
                &cap,
                pb::PoolRequestRequest {
                    target: String::new(),
                    method: "submit".into(),
                    params_json: params.to_string(),
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Code::PermissionDenied, "{what}: {e}");
        assert!(
            e.message().contains("owner") || e.message().contains("network rules"),
            "{what}: the refusal says what to do: {e}"
        );
    }
    // Nothing was recorded for the refused runs.
    assert!(
        super::runs::Ledger::load(w.shared.runtime.spaces().home_dir())
            .runs
            .is_empty()
    );
}

#[tokio::test]
async fn a_childs_env_is_reachable_only_below_and_never_its_spawn_channel() {
    let w = world();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let app = super::channel_app(w.shared.clone(), "local:a".into());
    let get = |space: &str, rest: &str| {
        let key = crate::passthrough::space_key(space);
        http::Request::builder()
            .method("POST")
            .uri(format!("/v1/spaces/{key}/env/{rest}"))
            .header("authorization", format!("Bearer {cap}"))
            .header("content-type", "application/grpc")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    use tower::ServiceExt as _;
    let status = |r: http::Response<axum::body::Body>| {
        r.headers()
            .get("grpc-status")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    // A sibling's and the owner's: not found.
    for other in ["local:b", "local:owner-dup", "local:root"] {
        let r = app
            .clone()
            .oneshot(get(other, "cua.env.v1.SystemService/GetCapabilities"))
            .await
            .unwrap();
        assert_eq!(status(r).as_deref(), Some("5"), "{other}");
    }
    // Its own child's spawn channel belongs to the host.
    let r = app
        .clone()
        .oneshot(get("local:a1", "cua.env.v1.TunnelService/StartSpawn"))
        .await
        .unwrap();
    assert_eq!(status(r).as_deref(), Some("7"));
}

#[tokio::test]
async fn a_child_past_the_trees_budget_gets_a_plain_refusal() {
    let w = world();
    // root's tree holds a, a1 and b.
    w.shared
        .children
        .as_deref()
        .unwrap()
        .budget
        .set_budget(
            "local:root",
            super::budget::Budget {
                max_spaces: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
    let cap = mint(&w.shared, "local:a", "local:root", 1);
    let mut c = SpaceServiceClient::new(channel(&w.shared, "local:a").await);
    let e = c
        .create_space(with(&cap, pb::CreateSpaceRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::ResourceExhausted);
    assert!(e.message().contains("3 child Spaces"), "{}", e.message());
    // The refusal is in the audit log with who asked.
    let log = w
        .shared
        .runtime
        .spaces()
        .home_dir()
        .join("children-audit.jsonl");
    let text = std::fs::read_to_string(&log).expect("the refusal is audited");
    assert!(
        text.contains("\"refuse\"") && text.contains("space:local:a"),
        "{text}"
    );
    // Nothing was created, and nothing stays reserved.
    assert_eq!(
        w.shared.runtime.spaces().registry().list().unwrap().len(),
        7
    );

    // The Space reads its own tree's budget and use; it can't change it.
    let b = c
        .get_tree_budget(with(&cap, pb::GetTreeBudgetRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .budget
        .unwrap();
    assert_eq!(
        (b.root.as_str(), b.max_spaces, b.spaces),
        ("local:root", 3, 3)
    );
    assert_eq!(b.max_vcpus, super::budget::DEFAULT_MAX_VCPUS);
    let e = c
        .set_tree_budget(with(&cap, pb::SetTreeBudgetRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
    // Another tree's budget is not its to read: it only ever sees its own.
    let other = mint(&w.shared, "local:o1", "local:other", 1);
    let mut o = SpaceServiceClient::new(channel(&w.shared, "local:o1").await);
    let b = o
        .get_tree_budget(with(&other, pb::GetTreeBudgetRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .budget
        .unwrap();
    assert_eq!(
        (b.root.as_str(), b.spaces, b.max_spaces),
        ("local:other", 1, 16)
    );
}

#[test]
fn a_trees_use_counts_its_children_and_their_size() {
    let w = world();
    let u = super::tree_usage(&w.shared, "local:root");
    // a, a1 and b, with no recorded size: the create defaults.
    assert_eq!((u.spaces, u.vcpus, u.memory_mb), (3, 6, 3 * 4096));
    assert_eq!(super::tree_usage(&w.shared, "local:a").spaces, 1);
    assert_eq!(super::tree_usage(&w.shared, "local:owner-dup").spaces, 0);
}

#[test]
fn orphans_are_listed_and_only_provable_ones_will_end() {
    let w = world();
    let spaces = w.shared.runtime.spaces();
    for (id, parent) in [
        ("local:lost", "local:gone"),
        ("local:far", "relay:abcdefgh"),
    ] {
        spaces
            .registry()
            .upsert(
                pb::Space {
                    id: id.into(),
                    name: id.trim_start_matches("local:").into(),
                    ..Default::default()
                },
                Credential::default(),
            )
            .unwrap();
        spaces
            .registry()
            .update_meta(id, |m| {
                m.parent = Some(parent.into());
            })
            .unwrap();
    }
    let mut orphans = super::orphans(&w.shared);
    orphans.sort_by(|a, b| a.space.cmp(&b.space));
    let rows: Vec<(&str, &str, &str)> = orphans
        .iter()
        .map(|o| (o.space.as_str(), o.parent.as_str(), o.state.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("local:far", "relay:abcdefgh", "listed"),
            ("local:lost", "local:gone", "ending"),
        ]
    );
    assert_eq!(
        orphans[1].ends_in_secs,
        super::reaper::GRACE_SECS as i64,
        "the grace starts now"
    );
    // Children whose parents are here are not orphans.
    assert!(orphans.iter().all(|o| !o.space.starts_with("local:a")));
}

/// Marks `space` as an agent home with the given grants.
fn make_home(w: &World, space: &str, scope: super::home::HomeScope) {
    w.shared
        .runtime
        .spaces()
        .registry()
        .update_meta(space, |m| m.labels.extend(scope.to_labels()))
        .unwrap();
}

#[tokio::test]
async fn an_agent_home_lists_only_what_it_was_granted() {
    use super::home::HomeScope;
    let w = world();
    let cap = mint(&w.shared, "local:root", "local:root", 0);
    let ids = |w: &World, cap: String| {
        let shared = w.shared.clone();
        async move {
            let mut c = SpaceServiceClient::new(channel(&shared, "local:root").await);
            let mut ids: Vec<String> = c
                .list_spaces(with(&cap, pb::ListSpacesRequest::default()))
                .await
                .unwrap()
                .into_inner()
                .spaces
                .into_iter()
                .map(|s| s.id)
                .collect();
            ids.sort();
            ids
        }
    };
    // A Space that is no home sees its subtree only (as before).
    assert_eq!(
        ids(&w, cap.clone()).await,
        ["local:a", "local:a1", "local:b"]
    );

    // A new home: its tree and the Space the person @mentioned by name.
    make_home(
        &w,
        "local:root",
        HomeScope {
            is_home: true,
            agent: "claude-code".into(),
            mentions: ["other".to_owned()].into(),
            ..Default::default()
        },
    );
    assert_eq!(
        ids(&w, cap.clone()).await,
        ["local:a", "local:a1", "local:b", "local:other"]
    );

    // Listing all, once the person approved it.
    make_home(
        &w,
        "local:root",
        HomeScope {
            is_home: true,
            list_all: true,
            ..Default::default()
        },
    );
    assert_eq!(ids(&w, cap).await.len(), 7);
}

#[tokio::test]
async fn an_agent_home_uses_a_machine_only_when_granted() {
    use super::home::HomeScope;
    let w = world();
    let scoped = super::api::Scoped {
        shared: w.shared.clone(),
        space: "local:root".into(),
    };
    let claims = super::capability::Claims {
        space: "local:root".into(),
        root: "local:root".into(),
        depth: 0,
        max_depth: 2,
        exp: u64::MAX,
        iat: 0,
        jti: String::new(),
        kid: String::new(),
    };
    let on_host = || pb::CreateSpaceRequest {
        location: "host:my-mac".into(),
        ..Default::default()
    };
    // Not a home: unchanged.
    assert!(scoped.child_request(&claims, on_host()).is_ok());
    // A home that was not given the machine: refused in words the agent can act on.
    make_home(
        &w,
        "local:root",
        HomeScope {
            is_home: true,
            ..Default::default()
        },
    );
    let e = scoped.child_request(&claims, on_host()).unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
    assert!(e.message().contains("my-mac"), "{}", e.message());
    // `auto` and this machine need no grant.
    for loc in ["auto", "local"] {
        let r = pb::CreateSpaceRequest {
            location: loc.into(),
            ..Default::default()
        };
        assert!(scoped.child_request(&claims, r).is_ok(), "{loc}");
    }
    // Granted (or @mentioned).
    make_home(
        &w,
        "local:root",
        HomeScope {
            is_home: true,
            machines: ["my-mac".to_owned()].into(),
            ..Default::default()
        },
    );
    assert!(scoped.child_request(&claims, on_host()).is_ok());
}

#[tokio::test]
async fn a_grant_given_while_the_home_runs_takes_effect_at_once() {
    use super::home::{HomeScope, grants_path};
    let w = world();
    make_home(
        &w,
        "local:root",
        HomeScope {
            is_home: true,
            ..Default::default()
        },
    );
    let cap = mint(&w.shared, "local:root", "local:root", 0);
    let c = SpaceServiceClient::new(channel(&w.shared, "local:root").await);
    let count = |mut c: SpaceServiceClient<Chan>, cap: String| async move {
        c.list_spaces(with(&cap, pb::ListSpacesRequest::default()))
            .await
            .unwrap()
            .into_inner()
            .spaces
            .len()
    };
    assert_eq!(count(c.clone(), cap.clone()).await, 3, "its tree only");
    // The app writes the person's approval outside the home; no restart.
    let file = grants_path(w.shared.runtime.spaces().home_dir());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"local:root":{"list_all":true,"machines":[]}}"#).unwrap();
    assert_eq!(count(c.clone(), cap.clone()).await, 7);
    // Another home's grant does not widen this one.
    std::fs::write(&file, r#"{"local:other":{"list_all":true}}"#).unwrap();
    assert_eq!(count(c, cap).await, 3);
}
