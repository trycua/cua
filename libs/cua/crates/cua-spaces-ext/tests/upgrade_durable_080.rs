// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A durable run's journal written by the cua 0.8.0 runner (before journals
//! went to the Cua Volume) is shipped by this version's host-side shipper and
//! restored byte for byte on another host, and a re-drive there fences the
//! old drive (CUA-2155). The journal is the one in
//! `cua-agents/runner/test/fixtures/durable-0.8.0/` (its README says how the
//! 0.8.0 runner wrote it); `upgrade-080.test.mjs` re-drives it with the new
//! runner.

use std::path::Path;
use std::sync::Arc;

use cua_spaces_ext::session_journal::{self, JournalDir as _, MemDir, Shipper};

const FILES: [&str; 4] = ["events.jsonl", "tools.jsonl", "state.json", "epoch"];

fn fixture(file: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cua-agents/runner/test/fixtures/durable-0.8.0/state")
            .join(file),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_080_journal_ships_restores_and_fences() {
    let vol = tempfile::tempdir().unwrap();
    let backend: Arc<dyn cua_volume::Backend> =
        Arc::new(cua_volume::fs::FsBackend::new(vol.path()));
    // The state dir as the 0.8.0 runner left it in the Space.
    let dir = MemDir::default();
    for f in FILES {
        dir.append(f, &fixture(f));
    }

    // The new host ships it at the epoch it was written with.
    let mut ship = Shipper::new(backend.clone(), "local:dev", "s1", 1).unwrap();
    let mut lines = 0;
    for _ in 0..10 {
        let r = ship.tick(&dir).await.unwrap();
        assert_eq!(r.fenced, None);
        if r.lines == 0 && r.chunk.is_none() {
            break;
        }
        lines += r.lines;
    }
    let journal_lines = |f: &str| {
        String::from_utf8(fixture(f))
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .count()
    };
    assert!(
        lines >= journal_lines("events.jsonl") + journal_lines("tools.jsonl"),
        "shipped {lines} lines"
    );
    // Shipping wrote only the ack beside the 0.8.0 files.
    for f in FILES {
        assert_eq!(dir.get(f).unwrap(), fixture(f), "{f} changed");
    }
    assert!(dir.get(session_journal::ACK_FILE).is_some());

    // Another host rebuilds the state dir from the Volume: the same journal.
    let elsewhere = MemDir::default();
    let restored = session_journal::restore_into(backend.as_ref(), "local:dev", "s1", &elsewhere)
        .await
        .unwrap();
    assert_eq!(restored.epoch, 1);
    assert_eq!(restored.dropped, 0);
    for f in ["events.jsonl", "tools.jsonl", "state.json"] {
        assert_eq!(
            elsewhere.get(f).unwrap(),
            fixture(f),
            "{f} restored differently"
        );
    }

    // A re-drive there raises the fence: the old drive's shipper stops and
    // tells its runner (the epoch file) to stop.
    session_journal::raise_fence(backend.as_ref(), "local:dev", "s1", 2, "host-b")
        .await
        .unwrap();
    dir.append("events.jsonl", b"{\"seq\":99,\"type\":\"late\"}\n");
    let r = ship.tick(&dir).await.unwrap();
    assert_eq!(r.fenced, Some(2));
    assert_eq!(
        String::from_utf8(dir.read("epoch", 0, 16).await.unwrap())
            .unwrap()
            .trim(),
        "2"
    );
}
