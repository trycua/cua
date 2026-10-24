//! Batched forks through the SDK (`Sandboxes.fork_many` / `fork_groups` /
//! `publish_checkpoint`), embedded and through a `cua daemon` on a Unix
//! socket. Hermetic: no VM starts (the checkpoints do not exist, so every
//! fork is reported failed, each on its own), which exercises the whole path (request,
//! placement, per-fork events, the summary) and the bindings' types. Real
//! forks are covered by `cua-daemon/tests/fork_batch_live.rs`.

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, DaemonHandle, ServerConfig},
};
use cua_sdk::{Cua, CuaError, ForkBatchEvent, ForkGroup, ForkManyOptions, SnapshotFallbackOptions};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug)]
enum Topology {
    Embedded,
    #[cfg_attr(not(unix), allow(dead_code))]
    DaemonSocket,
}

async fn world(t: Topology) -> (tempfile::TempDir, Option<DaemonHandle>, Arc<Cua>) {
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    match t {
        Topology::Embedded => (dirs, None, Cua::from_runtime(runtime)),
        Topology::DaemonSocket => {
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
            (dirs, Some(h), cua)
        }
    }
}

async fn suite(t: Topology) {
    let (dirs, _daemon, cua) = world(t).await;
    let sbx = cua.sandboxes();

    // 3 forks of a checkpoint that does not exist, in groups of 2: two
    // placements, three per-fork failures (each on its own), then the
    // summary.
    let batch = sbx
        .fork_many(
            "no-such-checkpoint".into(),
            3,
            Some(ForkManyOptions {
                on: Some("local".into()),
                group_size: 2,
                name_prefix: "fb".into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let mut placements = 0;
    let mut items = vec![];
    let mut done = None;
    while let Some(ev) = tokio::time::timeout(Duration::from_secs(60), batch.next())
        .await
        .unwrap()
        .unwrap()
    {
        match ev {
            ForkBatchEvent::Placement { placement } => {
                assert_eq!(placement.host, "local");
                assert_eq!(placement.attempt, 1);
                placements += 1;
            }
            ForkBatchEvent::Item { item } => items.push(item),
            ForkBatchEvent::Sync { .. } => panic!("nothing to copy on one machine"),
            ForkBatchEvent::Done { summary } => done = Some(summary),
        }
    }
    assert_eq!(placements, 2, "{t:?}");
    let summary = done.expect("the summary comes last");
    assert_eq!((summary.ok, summary.failed), (0, 3), "{t:?}");
    items.sort_by_key(|i| i.index);
    assert_eq!(
        items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        ["fb-g0-0", "fb-g0-1", "fb-g1-0"]
    );
    for i in &items {
        assert_eq!(i.error_code, "sync_failed", "{i:?}");
        assert!(i.error.contains("no machine of yours holds it"), "{i:?}");
        assert!(i.space.is_empty());
        assert_eq!(i.source, "no-such-checkpoint");
    }
    // The stream ends after the summary.
    assert!(batch.next().await.unwrap().is_none());
    // `collect` after `next` drained the batch returns what was read: the
    // forks and the summary, not an error.
    let again = batch.collect().await.unwrap();
    assert_eq!((again.summary.ok, again.summary.failed), (0, 3), "{t:?}");
    assert_eq!(
        again
            .items
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        ["fb-g0-0", "fb-g0-1", "fb-g1-0"]
    );
    // And after reading only part of a batch, it returns the whole of it.
    let part = sbx
        .fork_many(
            "no-such-checkpoint".into(),
            2,
            Some(ForkManyOptions {
                on: Some("local".into()),
                name_prefix: "fp".into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let first = loop {
        if let Some(ForkBatchEvent::Item { item }) = part.next().await.unwrap() {
            break item;
        }
    };
    let rest = part.collect().await.unwrap();
    assert_eq!(rest.items.len(), 2, "{t:?}");
    assert!(rest.items.iter().any(|i| i.index == first.index));
    // Collecting twice answers the same.
    assert_eq!(part.collect().await.unwrap(), rest);

    // `collect` waits for everything; `fork_groups` takes several
    // checkpoints.
    let all = sbx
        .fork_groups(
            vec![
                ForkGroup {
                    source: "w-a".into(),
                    count: 2,
                    name_prefix: "a".into(),
                },
                ForkGroup {
                    source: "w-b".into(),
                    count: 1,
                    name_prefix: String::new(),
                },
            ],
            None,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(all.items.len(), 3);
    assert_eq!(all.summary.failed, 3);
    assert_eq!(all.items[0].name, "a-0");
    assert!(all.items[2].name.starts_with("w-b-"), "{:?}", all.items[2]);

    // Bad requests are refused up front.
    for (source, count, on) in [("w", 0, "local"), ("w", 1, "cloud"), ("", 1, "local")] {
        let e = sbx
            .fork_many(
                source.into(),
                count,
                Some(ForkManyOptions {
                    on: Some(on.into()),
                    ..Default::default()
                }),
            )
            .await;
        let e = match e {
            Ok(b) => b.next().await.expect_err("refused"),
            Err(e) => e,
        };
        assert!(matches!(e, CuaError::InvalidArgument(_)), "{t:?} {e:?}");
    }

    // Publishing a checkpoint that does not exist says so.
    let e = sbx
        .publish_checkpoint(
            "no-such-checkpoint".into(),
            SnapshotFallbackOptions {
                dir: dirs.path().join("volume").display().to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(e, CuaError::NotFound(_)), "{t:?} {e:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded() {
    suite(Topology::Embedded).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_over_unix_socket() {
    suite(Topology::DaemonSocket).await;
}
