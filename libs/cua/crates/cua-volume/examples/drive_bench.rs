// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The drive benchmark's worker: one cua home's drive, driven over stdin
//! with one JSON command per line, answering one JSON line each. Run by
//! `crates/cua-volume/bench/drive_bench.py`; not a product surface.
//!
//! ```text
//! {"cmd":"storage","backend":"s3","endpoint":"http://127.0.0.1:9000","bucket":"b","root":"run/"}
//! {"cmd":"put","key":"media/a.mp4","file":"/path/a.mp4"}
//! {"cmd":"download","key":"media/a.mp4","out":"/path/copy.mp4","parallel":8}
//! {"cmd":"mount","path":"/path/Cua Volume"}      {"cmd":"unmount"}
//! {"cmd":"cache_clear"}  {"cmd":"cache_stats"}
//! {"cmd":"fill","folder":"public/many/","count":10000}
//! {"cmd":"ls","folder":"public/many/"}
//! {"cmd":"write","key":"public/x","size":1024}
//! {"cmd":"events","since":0,"wait_ms":10000}
//! {"cmd":"quit"}
//! ```
//!
//! Every answer carries `t_ms` (unix ms when the work finished) so the
//! orchestrator can measure across processes.

use std::io::BufRead as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_volume::backend::Condition;
use cua_volume::config::S3Settings;
use cua_volume::service::{DriveService, EnvKeys, StorageUpdate};
use cua_volume::{Context, Drive, now_ms};
use serde_json::{Value, json};

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

async fn handle(svc: &Arc<DriveService>, drive: &Drive, v: &Value) -> Result<Value, String> {
    let user = drive.session(Context::user());
    let t0 = Instant::now();
    let out = match v["cmd"].as_str().unwrap_or("") {
        "storage" => {
            let backend = s(v, "backend");
            let check = svc
                .set_storage(StorageUpdate {
                    backend: backend.clone(),
                    s3: (backend == "s3").then(|| S3Settings {
                        endpoint: Some(s(v, "endpoint")),
                        region: "us-east-1".into(),
                        bucket: s(v, "bucket"),
                        root: s(v, "root"),
                        path_style: true,
                    }),
                    // From the environment (EnvKeys): the worker has no
                    // credential store.
                    access_key_id: None,
                    secret_access_key: None,
                    dry_run: false,
                })
                .await
                .map_err(|e| e.to_string())?;
            json!(check)
        }
        "put" => {
            let m = user
                .write_file(
                    &s(v, "key"),
                    std::path::Path::new(&s(v, "file")),
                    Condition::None,
                )
                .await
                .map_err(|e| e.to_string())?;
            json!({"size": m.size, "version": m.version})
        }
        "download" => {
            // The download-first baseline: the whole object to a local file,
            // with parallel ranged GETs (what `aws s3 cp` does).
            let key = s(v, "key");
            let meta = user.open(&key, None).await.map_err(|e| e.to_string())?;
            let parallel = v["parallel"].as_u64().unwrap_or(8) as usize;
            let out = std::fs::File::create(s(v, "out")).map_err(|e| e.to_string())?;
            out.set_len(meta.size).map_err(|e| e.to_string())?;
            let out = Arc::new(out);
            let backend = drive.backend();
            let part = 16u64 << 20;
            let sem = Arc::new(tokio::sync::Semaphore::new(parallel));
            let mut tasks = tokio::task::JoinSet::new();
            let mut off = 0;
            while off < meta.size {
                let (b, k, ver, o, f, permit) = (
                    backend.clone(),
                    key.clone(),
                    meta.version.clone(),
                    off,
                    out.clone(),
                    sem.clone().acquire_owned().await.unwrap(),
                );
                tasks.spawn(async move {
                    let _p = permit;
                    let bytes = b
                        .get_range(&k, &ver, o, part)
                        .await
                        .map_err(|e| e.to_string())?;
                    write_all_at(f.as_ref(), &bytes, o).map_err(|e| e.to_string())
                });
                while let Some(r) = tasks.try_join_next() {
                    r.map_err(|e| e.to_string())??;
                }
                off += part;
            }
            while let Some(r) = tasks.join_next().await {
                r.map_err(|e| e.to_string())??;
            }
            json!({"size": meta.size})
        }
        "mount" => {
            svc.set_mount_path(Some(s(v, "path").into()))
                .map_err(|e| e.to_string())?;
            json!(svc.mount().await.map_err(|e| e.to_string())?)
        }
        "unmount" => json!(svc.unmount().await.map_err(|e| e.to_string())?),
        "cache_clear" => json!(svc.cache_clear()),
        "cache_stats" => json!(svc.cache_stats()),
        "fill" => {
            let folder = s(v, "folder");
            let n = v["count"].as_u64().unwrap_or(0);
            let sem = Arc::new(tokio::sync::Semaphore::new(32));
            let mut tasks = tokio::task::JoinSet::new();
            for i in 0..n {
                let (u, k, permit) = (
                    user.clone(),
                    format!("{folder}f{i:05}.txt"),
                    sem.clone().acquire_owned().await.unwrap(),
                );
                tasks.spawn(async move {
                    let _p = permit;
                    u.write(&k, format!("file {i}\n").into_bytes(), Condition::None)
                        .await
                        .map_err(|e| e.to_string())
                });
            }
            while let Some(r) = tasks.join_next().await {
                r.map_err(|e| e.to_string())??;
            }
            json!({"count": n})
        }
        "ls" => {
            let entries = user.ls(&s(v, "folder")).await.map_err(|e| e.to_string())?;
            json!({"entries": entries.len()})
        }
        "write" => {
            let size = v["size"].as_u64().unwrap_or(1) as usize;
            let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let m = user
                .write(&s(v, "key"), bytes, Condition::None)
                .await
                .map_err(|e| e.to_string())?;
            json!({"size": m.size})
        }
        "events" => {
            let (events, next) = svc
                .sync_events(
                    v["since"].as_u64().unwrap_or(0),
                    Duration::from_millis(v["wait_ms"].as_u64().unwrap_or(0)),
                )
                .await;
            let events: Vec<Value> = events
                .into_iter()
                .map(|e| json!({"kind": e.kind, "path": e.path, "ts_ms": e.ts_ms}))
                .collect();
            json!({"events": events, "next": next})
        }
        "sync_status" => json!(svc.sync_status().await),
        "feed_published" => json!({"published": svc.feed().published()}),
        other => return Err(format!("unknown command {other:?}")),
    };
    Ok(json!({"ok": true, "secs": t0.elapsed().as_secs_f64(), "t_ms": now_ms(), "out": out}))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    let home = std::path::PathBuf::from(std::env::var("CUA_HOME").expect("set CUA_HOME"));
    let drive = Drive::open_local(&home);
    let svc = DriveService::new(drive.clone(), &home, Arc::new(EnvKeys)).expect("service");
    svc.start().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    println!(
        "{}",
        json!({"ok": true, "ready": true, "device": svc.sync_status().await.device_id})
    );
    while let Some(line) = rx.recv().await {
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                println!("{}", json!({"ok": false, "error": e.to_string()}));
                continue;
            }
        };
        if v["cmd"] == "quit" {
            break;
        }
        match handle(&svc, &drive, &v).await {
            Ok(out) => println!("{out}"),
            Err(e) => println!("{}", json!({"ok": false, "error": e})),
        }
    }
    svc.shutdown().await;
}

/// Writes all of `buf` at `offset` without moving a shared cursor, so the
/// parallel range downloads can share one file handle.
#[cfg(unix)]
fn write_all_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn write_all_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt as _;
    while !buf.is_empty() {
        match file.seek_write(buf, offset)? {
            0 => return Err(std::io::ErrorKind::WriteZero.into()),
            n => {
                buf = &buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}
