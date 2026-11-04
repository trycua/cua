// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Turning replication on for a Volume that already exists, and living with
//! builds that do not know about it.
//!
//! Nothing about the Volume's own files changes: `data/` keeps its layout
//! (versions and heads, written the way every release has), so a build that
//! predates replication reads it exactly as before. Everything replication
//! adds is new and lives under `replica/` next to it:
//!
//! - `replica/format.json`: the format version of that directory. A build
//!   that finds a newer one than it understands refuses with a plain
//!   message instead of guessing (never corruption).
//! - `replica/backup/heads-<time>/`: a copy of the heads taken the first time
//!   replication is enabled on a Volume that has files, before anything
//!   moves. Versions are never deleted, so the heads are all a restore needs.
//! - `replica/heads.snap`: what the heads were at the last clean stop. A
//!   build that does not know about replication may write to `data/` while
//!   replication is off for it (an older Cua Spaces on the same home); on the
//!   next start the differences are found and handed to the log (or, on a
//!   machine that does not own the Volume, to the owner as changes), so they
//!   reach the other machines instead of silently diverging.
//!
//! Replication starts only when this account's machines list has another
//! machine and no bucket is configured ([`decide`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::era::Pos;
use super::oplog::{Op, OpLog};
use crate::backend::{Backend, ObjectMeta};
use crate::fs::FsBackend;
use crate::{Error, Result};

/// The format of the `replica/` directory this build writes and reads.
pub const FORMAT: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Format {
    format: u32,
    created_ms: u64,
}

/// Reads or creates `replica/format.json`. A directory written by a newer
/// format is refused.
pub fn check_format(dir: &Path) -> Result<()> {
    let p = dir.join("format.json");
    match std::fs::read(&p) {
        Ok(b) => {
            let f: Format = serde_json::from_slice(&b)
                .map_err(|e| Error::Invalid(format!("replica/format.json: {e}")))?;
            if f.format > FORMAT {
                return Err(Error::Invalid(format!(
                    "this Volume was prepared by a newer Cua Spaces (format {}, this one reads up to {FORMAT}); update Cua Spaces on this machine",
                    f.format
                )));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            cua_home::guard_write(&p)?;
            std::fs::write(
                &p,
                serde_json::to_vec(&Format {
                    format: FORMAT,
                    created_ms: crate::now_ms(),
                })?,
            )?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            n += copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copies the store's heads to `dir/backup/heads-<ms>/` (when it has any).
/// Returns where, and how many head files.
pub fn backup_heads(dir: &Path, store_root: &Path) -> Result<Option<(PathBuf, u64)>> {
    let heads = store_root.join("heads");
    if !heads.is_dir() {
        return Ok(None);
    }
    let to = dir
        .join("backup")
        .join(format!("heads-{}", crate::now_ms()));
    cua_home::guard_write(&to.join("x"))?;
    let n = copy_tree(&heads, &to)?;
    Ok(Some((to, n)))
}

/// What the heads were.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HeadsSnap {
    pub pos: Pos,
    pub heads: BTreeMap<String, ObjectMeta>,
}

fn snap_path(dir: &Path) -> PathBuf {
    dir.join("heads.snap")
}

/// Saves the heads as of `pos`.
pub async fn save_snap(dir: &Path, store: &FsBackend, pos: Pos) -> Result<()> {
    let heads: BTreeMap<String, ObjectMeta> = store
        .list("")
        .await?
        .into_iter()
        .map(|m| (m.key.clone(), m))
        .collect();
    let p = snap_path(dir);
    cua_home::guard_write(&p)?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(&HeadsSnap { pos, heads })?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// One change made behind replication's back.
#[derive(Debug, PartialEq, Eq)]
pub enum Unlogged {
    /// `meta` is the head now; `prev` was the head at the snapshot.
    Put {
        meta: ObjectMeta,
        prev: Option<ObjectMeta>,
    },
    /// `key` was deleted (a marker with `version`); `prev` was the head.
    Delete {
        key: String,
        version: String,
        prev: Option<ObjectMeta>,
    },
}

/// The changes to `store` since the snapshot that its log does not account
/// for. Empty when there is no snapshot (the Volume predates replication or
/// was never stopped cleanly) or the log no longer reaches back to it.
pub async fn find_unlogged(dir: &Path, store: &FsBackend, log: &OpLog) -> Result<Vec<Unlogged>> {
    let snap = std::fs::read(snap_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<HeadsSnap>(&b).ok());
    let Some(snap) = snap else {
        return find_unlogged_by_time(store, log).await;
    };
    let Some(after) = log.after(snap.pos, usize::MAX) else {
        // The log was trimmed past the snapshot: expectations cannot be rebuilt.
        return Ok(vec![]);
    };
    // What the heads should be: the snapshot plus what the log did since.
    let mut expect = snap.heads;
    for r in after {
        match r.op {
            Op::Put { meta, .. } => {
                expect.insert(meta.key.clone(), meta);
            }
            Op::Delete { key, .. } => {
                expect.remove(&key);
            }
        }
    }
    let actual: BTreeMap<String, ObjectMeta> = store
        .list("")
        .await?
        .into_iter()
        .map(|m| (m.key.clone(), m))
        .collect();
    let mut out = vec![];
    for (k, a) in &actual {
        match expect.get(k) {
            Some(e) if e.version == a.version => {}
            other => out.push(Unlogged::Put {
                meta: a.clone(),
                prev: other.cloned(),
            }),
        }
    }
    for (k, e) in &expect {
        if !actual.contains_key(k) {
            let version = store
                .versions(k)
                .await
                .ok()
                .and_then(|v| v.into_iter().find(|v| v.deleted).map(|v| v.version))
                .unwrap_or_default();
            if !version.is_empty() {
                out.push(Unlogged::Delete {
                    key: k.clone(),
                    version,
                    prev: Some(e.clone()),
                });
            }
        }
    }
    Ok(out)
}

/// Without a snapshot (the machine was stopped hard, or never stopped
/// cleanly): a head written after the newest log record, by anyone but the
/// log, is a change made behind its back. Only for stores that have a log.
async fn find_unlogged_by_time(store: &FsBackend, log: &OpLog) -> Result<Vec<Unlogged>> {
    let Some(newest) = log.newest_at_ms() else {
        return Ok(vec![]);
    };
    let known: std::collections::HashSet<(String, String)> = log
        .range(Pos::ZERO, log.tail())
        .into_iter()
        .filter_map(|r| match r.op {
            Op::Put { meta, .. } => Some((meta.key, meta.version)),
            _ => None,
        })
        .collect();
    let mut out = vec![];
    for m in store.list("").await? {
        if m.modified_ms > newest + 1000 && !known.contains(&(m.key.clone(), m.version.clone())) {
            out.push(Unlogged::Put {
                meta: m,
                prev: None,
            });
        }
    }
    Ok(out)
}

/// Whether replication should run at all.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    On,
    /// Off, and why (for a log line and a status).
    Off(&'static str),
}

/// Replication starts only when the user has signed in (a machines list
/// exists), has another machine on it, and has not pointed the Volume at a
/// bucket. Anything else leaves the Volume exactly as an earlier release
/// served it.
pub fn decide(bucket_configured: bool, signed_in: bool, other_machines: usize) -> Decision {
    if bucket_configured {
        Decision::Off("the Volume is on your own bucket")
    } else if !signed_in {
        Decision::Off("not signed in")
    } else if other_machines == 0 {
        Decision::Off("this is the only machine")
    } else {
        Decision::On
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_format_is_refused_and_an_older_or_missing_one_is_fine() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("replica");
        check_format(&dir).unwrap();
        check_format(&dir).unwrap();
        std::fs::write(dir.join("format.json"), br#"{"format":99,"created_ms":1}"#).unwrap();
        let e = check_format(&dir).unwrap_err();
        assert!(e.to_string().contains("newer Cua Spaces"), "{e}");
        std::fs::write(dir.join("format.json"), br#"{"format":0,"created_ms":1}"#).unwrap();
        check_format(&dir).unwrap();
    }

    #[test]
    fn replication_starts_only_after_signing_in_with_another_machine_and_no_bucket() {
        assert_eq!(
            decide(true, true, 3),
            Decision::Off("the Volume is on your own bucket")
        );
        assert_eq!(decide(false, false, 3), Decision::Off("not signed in"));
        assert_eq!(
            decide(false, true, 0),
            Decision::Off("this is the only machine")
        );
        assert_eq!(decide(false, true, 1), Decision::On);
    }

    #[tokio::test]
    async fn without_a_snapshot_a_head_newer_than_the_log_is_found() {
        use crate::backend::Condition;
        use crate::replica::oplog::Record;
        let d = tempfile::tempdir().unwrap();
        let store = FsBackend::new(d.path().join("data"));
        let state = d.path().join("replica");
        let mut log = OpLog::open(&state, 100).unwrap();
        let m = store
            .put("a/old", b"1".to_vec(), Condition::None)
            .await
            .unwrap();
        log.append(Record {
            pos: Pos { epoch: 1, seq: 1 },
            op: Op::Put {
                meta: m.clone(),
                prev: None,
            },
            at_ms: m.modified_ms + 1,
        })
        .unwrap();
        // The log knows a/old; a head made well after the last record is not its doing.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        store
            .put("a/new", b"2".to_vec(), Condition::None)
            .await
            .unwrap();
        let found = find_unlogged(&state, &store, &log).await.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(matches!(&found[0], Unlogged::Put { meta, .. } if meta.key == "a/new"));
    }

    #[tokio::test]
    async fn heads_are_backed_up_and_a_backdoor_write_is_found() {
        use crate::backend::Condition;
        let d = tempfile::tempdir().unwrap();
        let store = FsBackend::new(d.path().join("data"));
        store
            .put("a/one", b"1".to_vec(), Condition::None)
            .await
            .unwrap();
        store
            .put("a/two", b"2".to_vec(), Condition::None)
            .await
            .unwrap();
        let state = d.path().join("replica");
        let (to, n) = backup_heads(&state, store.root()).unwrap().unwrap();
        assert_eq!(n, 2);
        assert!(to.join("a.d").is_dir());
        let log = OpLog::open(&state, 100).unwrap();
        save_snap(&state, &store, log.tail()).await.unwrap();
        // An older build writes straight to the store.
        store
            .put("a/one", b"1 changed".to_vec(), Condition::None)
            .await
            .unwrap();
        store
            .put("a/three", b"3".to_vec(), Condition::None)
            .await
            .unwrap();
        store.delete("a/two", Condition::None).await.unwrap();
        let mut found = find_unlogged(&state, &store, &log).await.unwrap();
        found.sort_by_key(|u| match u {
            Unlogged::Put { meta, .. } => meta.key.clone(),
            Unlogged::Delete { key, .. } => key.clone(),
        });
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(
            matches!(&found[0], Unlogged::Put { meta, prev: Some(p) } if meta.key == "a/one" && p.size == 1)
        );
        assert!(matches!(&found[1], Unlogged::Put { meta, prev: None } if meta.key == "a/three"));
        assert!(matches!(&found[2], Unlogged::Delete { key, prev: Some(_), .. } if key == "a/two"));
    }
}
