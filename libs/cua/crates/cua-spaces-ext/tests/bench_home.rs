// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Benchmark: saving and restoring a persistent agent home, by size, against
//! a real Linux Space. Opt-in: run through
//! `libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh --test bench_home`, which exports
//! `CUA_SPACES_E2E_URL` and `CUA_SPACES_E2E_TOKEN`; without them the test
//! prints why and passes. Writes `home.json` and `home.md` to
//! `CUA_BENCH_OUT` (default: a temp dir).
//!
//! For each size it measures, three runs each (median reported):
//! - full save (the drive has nothing yet);
//! - no-change save (hashing only);
//! - one-file-changed save (incremental);
//! - full restore into an empty home (a new Space).
//!
//! Host effects: a temporary drive directory only.

use std::time::Duration;

use cua_spaces::Spaces;
use cua_spaces_ext::persistent::home;

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some((
        url,
        std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default(),
    ))
}

fn median(mut v: Vec<u64>) -> u64 {
    v.sort_unstable();
    v[v.len() / 2]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench_home_save_and_restore() {
    let Some((url, token)) = target() else {
        eprintln!(
            "skipped: set CUA_SPACES_E2E_URL (run libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh --test bench_home)"
        );
        return;
    };
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(reg.path()).build();
    let info = spaces
        .add(&url, Some(token), Some("bench".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    let guest = space.spacesd().unwrap().clone();
    let home_dir = "/tmp/cua-bench-home/agents/bench";
    // (label, files, bytes per file)
    let shapes: &[(&str, u64, u64)] = &[
        ("100 KiB (100 x 1 KiB)", 100, 1024),
        ("1 MiB (1 x 1 MiB)", 1, 1 << 20),
        ("10 MiB (10 x 1 MiB)", 10, 1 << 20),
        ("100 MiB (100 x 1 MiB)", 100, 1 << 20),
        ("10 MiB (1000 x 10 KiB)", 1000, 10 * 1024),
    ];
    let mut rows = vec![];
    for (label, files, size) in shapes {
        let mut full = vec![];
        let mut same = vec![];
        let mut one = vec![];
        let mut restore = vec![];
        for round in 0..3 {
            // A fresh drive and a fresh home for every round.
            let drive_dir = tempfile::tempdir().unwrap();
            let drive = cua_volume::Drive::open_local(drive_dir.path());
            let session = drive.session(cua_volume::Context::agent("bench", Some(&info.id)));
            let make = format!(
                "rm -rf /tmp/cua-bench-home && mkdir -p {home_dir}/memory && cd {home_dir}/memory && \
                 i=0; while [ $i -lt {files} ]; do head -c {size} /dev/urandom > f$i.bin; i=$((i+1)); done"
            );
            let out = space.bash(&make, Duration::from_secs(600)).await.unwrap();
            assert!(out.success(), "{}", out.render());
            let t = home::save(&guest, &session, "bench", home_dir)
                .await
                .unwrap();
            assert_eq!(t.files as u64, *files, "{label} round {round}");
            full.push(t.millis);
            let t = home::save(&guest, &session, "bench", home_dir)
                .await
                .unwrap();
            assert_eq!(t.files, 0);
            same.push(t.millis);
            space
                .bash(
                    &format!("head -c {size} /dev/urandom > {home_dir}/memory/f0.bin"),
                    Duration::from_secs(60),
                )
                .await
                .unwrap();
            let t = home::save(&guest, &session, "bench", home_dir)
                .await
                .unwrap();
            assert_eq!(t.files, 1);
            one.push(t.millis);
            space
                .bash("rm -rf /tmp/cua-bench-home", Duration::from_secs(60))
                .await
                .unwrap();
            let t = home::restore(&guest, &session, "bench", home_dir)
                .await
                .unwrap();
            assert_eq!(t.files as u64, *files);
            restore.push(t.millis);
        }
        let total = files * size;
        let mbps = |ms: u64| {
            if ms == 0 {
                0.0
            } else {
                total as f64 / 1_048_576.0 / (ms as f64 / 1000.0)
            }
        };
        let row = serde_json::json!({
            "home": label, "bytes": total, "files": files,
            "full_save_ms": median(full.clone()), "full_save_mib_s": mbps(median(full)),
            "unchanged_save_ms": median(same), "one_file_save_ms": median(one),
            "restore_ms": median(restore.clone()), "restore_mib_s": mbps(median(restore)),
        });
        println!("{row}");
        rows.push(row);
    }
    let out = std::path::PathBuf::from(
        std::env::var("CUA_BENCH_OUT")
            .unwrap_or_else(|_| std::env::temp_dir().join("cua-bench").display().to_string()),
    );
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("home.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    let mut md = String::from(
        "| Home | Full save ms | MiB/s | Unchanged save ms | One file changed ms | Restore ms | MiB/s |\n|---|---|---|---|---|---|---|\n",
    );
    for r in &rows {
        md.push_str(&format!(
            "| {} | {} | {:.1} | {} | {} | {} | {:.1} |\n",
            r["home"].as_str().unwrap(),
            r["full_save_ms"],
            r["full_save_mib_s"].as_f64().unwrap(),
            r["unchanged_save_ms"],
            r["one_file_save_ms"],
            r["restore_ms"],
            r["restore_mib_s"].as_f64().unwrap()
        ));
    }
    std::fs::write(out.join("home.md"), &md).unwrap();
    println!("{md}");
}
