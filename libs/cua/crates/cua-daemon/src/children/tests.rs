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

use super::api::{LABEL_DEPTH, LABEL_PARENT, LABEL_ROOT};
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
                    m.labels.insert(LABEL_PARENT.into(), parent.into());
                    m.labels.insert(LABEL_ROOT.into(), root.into());
                    m.labels.insert(LABEL_DEPTH.into(), depth.to_string());
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
    assert_eq!(r.labels[LABEL_PARENT], "local:a");
    assert_eq!(r.labels[LABEL_ROOT], "local:root");
    assert_eq!(r.labels[LABEL_DEPTH], "2");

    let refuse = |depth, req: pb::CreateSpaceRequest| {
        scoped
            .child_request(&claims(depth), req)
            .unwrap_err()
            .code()
    };
    // Deeper than the tree allows.
    assert_eq!(refuse(2, Default::default()), Code::ResourceExhausted);
    // Another machine or a cloud, reuse, durable, volumes.
    assert_eq!(
        refuse(
            1,
            pb::CreateSpaceRequest {
                location: "aws".into(),
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
                m.labels.insert(LABEL_PARENT.into(), parent.into());
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
