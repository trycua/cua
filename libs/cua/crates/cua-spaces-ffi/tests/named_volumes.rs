// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Named volumes through the SDK, embedded and through a daemon (Unix
//! socket), with the Cua Spaces extensions registered and a throwaway cua
//! home: create, list, mount records, delete (refused while mounted),
//! and the refusals a create with volumes gives. Nothing mounts anything.

use std::sync::Arc;
use std::time::Duration;

use cua_daemon::server::{self, ServerConfig};
use cua_daemon::{Runtime, RuntimeConfig};
use cua_sdk::{Cua, CuaError, SpaceCreateOptions, VolumeMount};

fn mount(name: &str, path: &str, read_only: bool) -> VolumeMount {
    VolumeMount {
        name: name.into(),
        path: path.into(),
        read_only,
        subpath: String::new(),
    }
}

#[derive(Clone, Copy, Debug)]
enum Topology {
    Embedded,
    // Only the Unix test runs it (the daemon listens on a Unix socket).
    #[cfg_attr(not(unix), allow(dead_code))]
    Daemon,
}

async fn world(t: Topology) -> (Arc<Cua>, tempfile::TempDir, Option<server::DaemonHandle>) {
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        extensions: vec![Arc::new(
            cua_spaces_ext::daemon::CuaSpacesDaemon::with_test_presence(Arc::new(
                cua_keyvault::broker::FakePresence::new(true),
            )),
        )],
        ..Default::default()
    })
    .unwrap();
    // A registered Space that mounts `shared` read-only.
    runtime
        .spaces()
        .registry()
        .upsert(
            cua_proto::daemon::v1::Space {
                id: "local:reader".into(),
                name: "reader".into(),
                volumes: vec![
                    (&cua_spaces::named_volumes::VolumeMount {
                        name: "shared".into(),
                        path: "/mnt/shared".into(),
                        read_only: true,
                        subpath: "data".into(),
                    })
                        .into(),
                ],
                ..Default::default()
            },
            cua_spaces::registry::Credential::default(),
        )
        .unwrap();
    match t {
        Topology::Embedded => (Cua::from_runtime(runtime), dirs, None),
        Topology::Daemon => {
            let cfg = ServerConfig {
                socket_path: Some(dirs.path().join("cua.sock")),
                loopback: None,
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            };
            let h = server::start(runtime, cfg).await.unwrap();
            let cua = Cua::connect(
                Some(h.socket_path.as_ref().unwrap().display().to_string()),
                None,
            )
            .unwrap();
            (cua, dirs, Some(h))
        }
    }
}

async fn suite(t: Topology) {
    let (cua, _dirs, _daemon) = world(t).await;
    let spaces = cua.spaces();
    assert!(spaces.volumes().await.unwrap().is_empty());

    // Make, list, refuse a second one and bad names.
    let v = spaces.create_volume("shared".into()).await.unwrap();
    assert_eq!(v.name, "shared");
    spaces.create_volume("scratch".into()).await.unwrap();
    let e = spaces.create_volume("shared".into()).await.unwrap_err();
    assert!(e.to_string().contains("already exists"), "{t:?}: {e}");
    for bad in ["Shared", "a/b", ""] {
        assert!(spaces.create_volume(bad.into()).await.is_err(), "{bad}");
    }
    let listed = spaces.volumes().await.unwrap();
    assert_eq!(
        listed.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        ["scratch", "shared"]
    );
    // The record of the Space that mounts it says which Space does.
    assert_eq!(listed[1].mounted_in, vec!["local:reader".to_string()]);
    assert!(listed[0].mounted_in.is_empty());

    // The Space's record carries its mounts through every topology.
    let reader = spaces
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == "local:reader")
        .unwrap();
    assert_eq!(
        reader.volumes,
        vec![VolumeMount {
            name: "shared".into(),
            path: "/mnt/shared".into(),
            read_only: true,
            subpath: "data".into(),
        }]
    );

    // A volume is its folder of the Cua Volume.
    spaces
        .volume_write(
            "volumes/shared/a.txt".into(),
            b"a".to_vec(),
            None,
            false,
            None,
        )
        .await
        .unwrap();
    let l = spaces
        .volume_ls(Some("volumes/".into()), None)
        .await
        .unwrap();
    assert_eq!(
        l.entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["scratch", "shared"]
    );

    // Deleting a mounted volume is refused unless forced; an unmounted one
    // goes at once, files and all.
    let e = spaces
        .delete_volume("shared".into(), false)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("local:reader"), "{t:?}: {e}");
    assert_eq!(
        spaces.delete_volume("scratch".into(), false).await.unwrap(),
        0
    );
    assert_eq!(
        spaces.delete_volume("shared".into(), true).await.unwrap(),
        1
    );
    assert!(spaces.volumes().await.unwrap().is_empty());
    let e = spaces
        .delete_volume("shared".into(), false)
        .await
        .unwrap_err();
    assert!(matches!(e, CuaError::NotFound(_)), "{t:?}: {e:?}");

    // A create with volumes is checked before anything is made.
    spaces.create_volume("data".into()).await.unwrap();
    let create = |volumes: Vec<VolumeMount>| {
        let spaces = spaces.clone();
        async move {
            spaces
                .create(SpaceCreateOptions {
                    name: Some("vol-user".into()),
                    volumes,
                    ..Default::default()
                })
                .await
        }
    };
    let e = create(vec![mount("ghost", "/mnt/ghost", false)])
        .await
        .unwrap_err();
    assert!(e.to_string().contains("does not exist"), "{t:?}: {e}");
    let e = create(vec![mount("data", "mnt/data", false)])
        .await
        .unwrap_err();
    assert!(
        matches!(e, CuaError::InvalidArgument(_)) && e.to_string().contains("absolute"),
        "{t:?}: {e:?}"
    );
    let e = create(vec![
        mount("data", "/mnt/x", false),
        mount("data", "/mnt/x/y", true),
    ])
    .await
    .unwrap_err();
    assert!(e.to_string().contains("inside"), "{t:?}: {e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_volumes_embedded() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::Embedded))
        .await
        .expect("embedded suite");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_volumes_through_a_daemon() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::Daemon))
        .await
        .expect("daemon suite");
}
