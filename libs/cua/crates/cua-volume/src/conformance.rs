// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The backend conformance suite: the behavior every [`Backend`] must
//! share, so access checks, sync and leases work the same on the local
//! store and on S3. Run by the local backend's unit tests and by the
//! opt-in MinIO test against a real versioned bucket.

use crate::Error;
use crate::backend::{Backend, Condition};

macro_rules! check {
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err(format!($($msg)+));
        }
    };
}

/// Runs the suite under `prefix` (a fresh folder such as `conf-<id>/`).
pub async fn run(b: &dyn Backend, prefix: &str) -> Result<(), String> {
    let k = |s: &str| format!("{prefix}{s}");
    let tag = |e: Error| e.tag().to_string();

    // Create-only, compare-and-swap.
    let m1 = b
        .put(&k("a/b.txt"), b"one".to_vec(), Condition::IfNoneMatch)
        .await
        .map_err(|e| format!("create: {e}"))?;
    let again = b
        .put(&k("a/b.txt"), b"x".to_vec(), Condition::IfNoneMatch)
        .await
        .map_err(tag);
    check!(
        again == Err("precondition_failed".into()),
        "second create: {again:?}"
    );
    let m2 = b
        .put(
            &k("a/b.txt"),
            b"two".to_vec(),
            Condition::IfMatch(m1.etag.clone()),
        )
        .await
        .map_err(|e| format!("cas: {e}"))?;
    check!(m2.etag != m1.etag, "etag changes with content");
    let stale = b
        .put(
            &k("a/b.txt"),
            b"3".to_vec(),
            Condition::IfMatch(m1.etag.clone()),
        )
        .await
        .map_err(tag);
    check!(
        stale == Err("precondition_failed".into()),
        "stale cas: {stale:?}"
    );

    // A second object beside the folder. (A key and a folder of the same
    // name, `a` and `a/b.txt`, coexist on AWS S3, R2 and the local backend
    // but not on MinIO; the drive layout never needs both, so the suite
    // does not require it.)
    b.put(&k("c"), b"file c".to_vec(), Condition::None)
        .await
        .map_err(|e| format!("put c: {e}"))?;
    let (bytes, meta) = b
        .get(&k("a/b.txt"), None)
        .await
        .map_err(|e| format!("get: {e}"))?;
    check!(bytes == b"two", "current content: {bytes:?}");
    check!(meta.size == 3 && meta.etag == m2.etag, "meta: {meta:?}");
    let (old, _) = b
        .get(&k("a/b.txt"), Some(&m1.version))
        .await
        .map_err(|e| format!("get version: {e}"))?;
    check!(old == b"one", "old version: {old:?}");
    let head = b.head(&k("c")).await.map_err(|e| format!("head: {e}"))?;
    check!(head.is_some_and(|h| h.size == 6), "head c");

    // Listing is recursive, sorted and prefix-exact.
    let keys: Vec<String> = b
        .list(prefix)
        .await
        .map_err(|e| format!("list: {e}"))?
        .into_iter()
        .map(|m| m.key)
        .collect();
    check!(keys == [k("a/b.txt"), k("c")], "list: {keys:?}");
    let keys: Vec<String> = b
        .list(&k("a/"))
        .await
        .map_err(|e| format!("list a/: {e}"))?
        .into_iter()
        .map(|m| m.key)
        .collect();
    check!(keys == [k("a/b.txt")], "list a/: {keys:?}");

    // Deletes leave history.
    let wrong = b
        .delete(&k("a/b.txt"), Condition::IfMatch(m1.etag.clone()))
        .await
        .map_err(tag);
    check!(
        wrong == Err("precondition_failed".into()),
        "conditional delete: {wrong:?}"
    );
    b.delete(&k("a/b.txt"), Condition::IfMatch(m2.etag.clone()))
        .await
        .map_err(|e| format!("delete: {e}"))?;
    let gone = b
        .head(&k("a/b.txt"))
        .await
        .map_err(|e| format!("head: {e}"))?;
    check!(gone.is_none(), "deleted object still has a head");
    let missing = b.get(&k("a/b.txt"), None).await.map_err(tag);
    check!(
        missing.as_ref().err().map(String::as_str) == Some("not_found"),
        "get deleted: {:?}",
        missing.map(|_| ())
    );
    let hist = b
        .versions(&k("a/b.txt"))
        .await
        .map_err(|e| format!("versions: {e}"))?;
    check!(hist.len() == 3, "history length: {hist:?}");
    check!(hist[0].deleted, "newest is the delete marker: {hist:?}");
    check!(
        !hist.iter().any(|h| h.latest),
        "no latest after delete: {hist:?}"
    );
    check!(
        hist.iter()
            .filter(|h| !h.deleted)
            .any(|h| h.version == m2.version),
        "history has the second version"
    );
    // Restore is a put of an old version.
    let (bytes, _) = b
        .get(&k("a/b.txt"), Some(&m1.version))
        .await
        .map_err(|e| format!("get old: {e}"))?;
    b.put(&k("a/b.txt"), bytes, Condition::IfNoneMatch)
        .await
        .map_err(|e| format!("restore: {e}"))?;
    let (bytes, _) = b
        .get(&k("a/b.txt"), None)
        .await
        .map_err(|e| format!("get: {e}"))?;
    check!(bytes == b"one", "restored content");

    // Missing things.
    let d = b.delete(&k("missing"), Condition::None).await.map_err(tag);
    check!(d == Err("not_found".into()), "delete missing: {d:?}");
    let h = b
        .head(&k("missing"))
        .await
        .map_err(|e| format!("head missing: {e}"))?;
    check!(h.is_none(), "head missing");
    let v = b.versions(&k("missing")).await.map_err(tag);
    check!(
        v.as_ref().err().map(String::as_str) == Some("not_found"),
        "versions missing"
    );

    // Clean up the current objects (history stays on a versioned store).
    for key in [k("c"), k("a/b.txt")] {
        let _ = b.delete(&key, Condition::None).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_local_backend_conforms() {
        let dir = tempfile::tempdir().unwrap();
        let b = crate::fs::FsBackend::new(dir.path());
        super::run(&b, "conf/").await.unwrap();
    }
}
