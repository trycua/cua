// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What a directory watch reports for attribute changes and recursive
//! `mkdir -p`, and how a signal death is reported. Real inotify and real
//! processes, against the in-process server or any driver with
//! `CUA_ENV_TEST_TARGET` (see `common`). Linux/macOS guests only.
#![cfg(unix)]

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use cua_proto::env::v1::process_event::Event;
use cua_proto::env::v1::watch_dir_response::Message as WatchMessage;
use cua_proto::env::v1::*;
use cua_spacesd_client::{SpacesdClient, TransportPreference};

use common::*;

fn sh(script: &str) -> ProcessConfig {
    ProcessConfig {
        command: SH.into(),
        args: vec!["-c".into(), script.into()],
        ..Default::default()
    }
}

type Seen = BTreeSet<(String, i32)>;

/// Starts a watch on `dir` and returns its stream, once the backend is live.
///
/// `Started` only says the watch is registered. FSEvents (macOS) starts
/// delivering a moment later and loses what happens in between, so a change
/// made right after `Started` can vanish. The stream is primed: a sentinel
/// (`.mark-ready`, ignored by the readers) is touched until its event arrives.
async fn watch(
    c: &SpacesdClient,
    dir: &str,
    recursive: bool,
) -> tonic::Streaming<WatchDirResponse> {
    let mut stream = c
        .filesystem()
        .watch_dir(WatchDirRequest {
            path: dir.into(),
            recursive,
            keepalive_interval: None,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let first = stream.message().await.unwrap().unwrap();
    assert!(matches!(first.message, Some(WatchMessage::Started(_))));

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut live = false;
    while !live && Instant::now() < deadline {
        run_sh(c, &format!("touch '{dir}/.mark-ready'")).await;
        let until = Instant::now() + Duration::from_millis(500);
        while let Ok(Ok(Some(m))) = tokio::time::timeout(
            until.saturating_duration_since(Instant::now()),
            stream.message(),
        )
        .await
        {
            if let Some(WatchMessage::Event(e)) = m.message {
                live |= e.path.ends_with(".mark-ready");
            }
        }
    }
    assert!(live, "the watch never delivered an event");
    stream
}

/// The events (path relative to `dir`, kind) of what ran so far: writes a
/// marker file and reads until its creation arrives. A watch reports in
/// order, so everything before the marker has arrived by then; no waiting on
/// how fast the platform delivers (FSEvents batches for a second or more).
/// `want`: events reported a little later than the change itself (Linux
/// reports the levels of a new subtree from a scan once each is watched),
/// read until they arrive too.
async fn settled_with(
    c: &SpacesdClient,
    stream: &mut tonic::Streaming<WatchDirResponse>,
    dir: &str,
    want: &[(String, i32)],
) -> Seen {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let mark = format!(
        ".mark-{}",
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    run_sh(c, &format!("printf x > '{dir}/{mark}'")).await;
    let mut seen = Seen::new();
    let mut marked = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(marked && want.iter().all(|w| seen.contains(w))) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, stream.message()).await {
            Ok(Ok(Some(m))) => {
                if let Some(WatchMessage::Event(e)) = m.message {
                    let rel = e.path.strip_prefix(dir).unwrap_or(&e.path);
                    let rel = rel.trim_start_matches('/').to_owned();
                    if rel == mark && e.r#type == FsEventType::Create as i32 {
                        marked = true;
                        continue;
                    }
                    // The watched directory itself, and earlier markers.
                    if rel.is_empty() || rel.starts_with(".mark-") {
                        continue;
                    }
                    seen.insert((rel, e.r#type));
                }
            }
            // Out of time: the caller's assertions say what is missing.
            _ => break,
        }
    }
    assert!(marked, "the watch never reported {mark}; saw {seen:?}");
    seen
}

/// [`settled_with`], wanting nothing more than the marker.
async fn settled(
    c: &SpacesdClient,
    stream: &mut tonic::Streaming<WatchDirResponse>,
    dir: &str,
) -> Seen {
    settled_with(c, stream, dir, &[]).await
}

fn k(name: &str, kind: FsEventType) -> (String, i32) {
    (name.to_owned(), kind as i32)
}

async fn make_dir(c: &SpacesdClient, path: &str) {
    c.filesystem()
        .make_dir(MakeDirRequest {
            path: path.into(),
            parents: true,
            mode: 0,
            ..Default::default()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn attribute_changes_are_chmod_and_writes_are_write() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let dir = t.path("watch-kinds");
    make_dir(&c, &dir).await;
    let mut stream = watch(&c, &dir, false).await;

    // `touch` of a new file: create plus an attribute change, never a write.
    run_sh(&c, &format!("touch '{dir}/new.txt'")).await;
    let seen = settled(&c, &mut stream, &dir).await;
    assert!(
        seen.contains(&k("new.txt", FsEventType::Create)),
        "{seen:?}"
    );
    // inotify reports the new file's times as their own change; FSEvents
    // folds them into the creation.
    #[cfg(target_os = "linux")]
    assert!(seen.contains(&k("new.txt", FsEventType::Chmod)), "{seen:?}");
    assert!(
        !seen.contains(&k("new.txt", FsEventType::Write)),
        "{seen:?}"
    );

    // `touch` of an existing file and `chmod`: attribute changes only.
    run_sh(
        &c,
        &format!("touch '{dir}/new.txt'; chmod 600 '{dir}/new.txt'"),
    )
    .await;
    let seen = settled(&c, &mut stream, &dir).await;
    // FSEvents keeps a file's "created" flag on its later events, so macOS
    // may report the creation again; never a write.
    let mut seen = seen;
    if cfg!(target_os = "macos") {
        seen.remove(&k("new.txt", FsEventType::Create));
    }
    assert_eq!(seen, Seen::from([k("new.txt", FsEventType::Chmod)]));

    // A content write is a write.
    run_sh(&c, &format!("printf data > '{dir}/new.txt'")).await;
    let seen = settled(&c, &mut stream, &dir).await;
    assert!(seen.contains(&k("new.txt", FsEventType::Write)), "{seen:?}");
    // On macOS the earlier attribute change's flag stays on the file's
    // events, so only inotify can tell a write alone.
    #[cfg(target_os = "linux")]
    assert!(
        !seen.contains(&k("new.txt", FsEventType::Chmod)),
        "{seen:?}"
    );
    t.cleanup().await;
}

#[tokio::test]
async fn recursive_mkdir_reports_every_new_level_once() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let dir = t.path("watch-deep");
    make_dir(&c, &dir).await;
    let mut stream = watch(&c, &dir, true).await;

    run_sh(
        &c,
        &format!("mkdir -p '{dir}/deep/a/b' && printf x > '{dir}/deep/a/b/f.txt'"),
    )
    .await;
    let levels = ["deep", "deep/a", "deep/a/b", "deep/a/b/f.txt"];
    let want: Vec<_> = levels.iter().map(|n| k(n, FsEventType::Create)).collect();
    let seen = settled_with(&c, &mut stream, &dir, &want).await;
    for name in levels {
        assert!(
            seen.contains(&k(name, FsEventType::Create)),
            "{name}: {seen:?}"
        );
    }

    // The new subtree stays watched.
    run_sh(&c, &format!("printf y > '{dir}/deep/a/b/g.txt'")).await;
    let g = k("deep/a/b/g.txt", FsEventType::Create);
    let seen = settled_with(&c, &mut stream, &dir, std::slice::from_ref(&g)).await;
    assert!(
        seen.contains(&k("deep/a/b/g.txt", FsEventType::Create)),
        "{seen:?}"
    );
    t.cleanup().await;
}

/// Runs `script` and returns its end.
async fn end_of(c: &SpacesdClient, script: &str) -> ProcessEnd {
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(sh(script)),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(30), stream.message())
            .await
            .expect("process stream stalled")
            .unwrap()
            .expect("stream ended without ProcessEnd");
        if let Some(Event::End(end)) = m.event.and_then(|e| e.event) {
            return end;
        }
    }
}

#[tokio::test]
async fn signal_deaths_carry_the_raw_signal() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    // SEGV, ABRT and PIPE are outside the contract enum.
    for (sig, name) in [
        ("SEGV", "SEGV"),
        ("ABRT", "ABRT"),
        ("PIPE", "PIPE"),
        ("BUS", "BUS"),
    ] {
        let end = end_of(&c, &format!("kill -{sig} $$; sleep 5")).await;
        assert_eq!(end.exit_code, None, "{sig}: {end:?}");
        assert!(end.error.is_empty(), "{sig}: {end:?}");
        assert_eq!(end.signal, Signal::Unspecified as i32, "{sig}: {end:?}");
        assert_eq!(end.signal_name, name, "{end:?}");
        assert!(end.signal_number > 0, "{end:?}");
    }
    // Enum signals keep the enum and gain the raw fields.
    let end = end_of(&c, "kill -KILL $$; sleep 5").await;
    assert_eq!(end.signal, Signal::Kill as i32);
    assert_eq!((end.signal_number, end.signal_name.as_str()), (9, "KILL"));
    // A normal exit has none of them.
    let end = end_of(&c, "exit 3").await;
    assert_eq!(end.exit_code, Some(3));
    assert_eq!((end.signal_number, end.signal_name.as_str()), (0, ""));
    t.cleanup().await;
}
