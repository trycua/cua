// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deploys without downtime (CUA-1774): one relay at a time owns the state
//! directory, and the next one waits beside it.
//!
//! The relay keeps its state in files it loads at start and rewrites as it
//! goes, so two processes must never use the same state directory at once.
//! [`StateLock`] enforces that with an exclusive `flock` on
//! `<state dir>/.relay.lock`. The kernel drops the lock when the process
//! exits, however it exits, so a crash never leaves it stale.
//!
//! A rolling deploy then hands over like this:
//!
//! 1. The new relay starts on the same node (same volume), binds its port
//!    and finds the lock taken. It waits in **standby**: it does not
//!    accept connections yet, so the kernel queues them (the listen
//!    backlog) instead of anyone seeing an error, and its health port
//!    answers, so Kubernetes routes to it and stops the old relay.
//! 2. The old relay gets SIGTERM, closes every machine tunnel and mesh
//!    connection so they reconnect at once, lets open requests finish
//!    (`CUA_RELAY_SHUTDOWN_GRACE_SECS`), saves usage and exits.
//! 3. The lock is free: the new relay loads the state the old one left and
//!    starts accepting, queued requests and reconnecting machines first.
//!    For a short while after start, a request for a known machine that
//!    is not back yet waits for it (`CUA_RELAY_RECONNECT_GRACE_SECS`)
//!    instead of failing.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use axum::{http::StatusCode, routing::get, Router};

/// The lock file's name in the state directory.
pub const LOCK_FILE: &str = ".relay.lock";

/// The lock file for state directory `dir`.
pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join(LOCK_FILE)
}

/// Exclusive ownership of a state directory, held until dropped or until
/// the process exits.
#[derive(Debug)]
pub struct StateLock {
    _file: File,
    path: PathBuf,
}

impl StateLock {
    fn open(path: &Path) -> std::io::Result<File> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(path)
    }

    /// Takes the lock if no one holds it; `None` when another process does.
    pub fn try_acquire(path: &Path) -> std::io::Result<Option<Self>> {
        let file = Self::open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self {
                _file: file,
                path: path.to_path_buf(),
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    /// Takes the lock, waiting (standby) for as long as another process
    /// holds it.
    pub async fn acquire(path: &Path) -> std::io::Result<Self> {
        if let Some(lock) = Self::try_acquire(path)? {
            return Ok(lock);
        }
        tracing::info!(
            lock = %path.display(),
            "standby: another relay owns the state directory; waiting for it to exit"
        );
        let started = std::time::Instant::now();
        let owned = path.to_path_buf();
        let lock = tokio::task::spawn_blocking(move || {
            let file = Self::open(&owned)?;
            file.lock()?;
            Ok::<_, std::io::Error>(Self {
                _file: file,
                path: owned,
            })
        })
        .await
        .map_err(std::io::Error::other)??;
        tracing::info!(
            waited_ms = started.elapsed().as_millis() as u64,
            "took over the state directory from the previous relay"
        );
        Ok(lock)
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Serves `GET /healthz` (204) on `listener` until the process exits: the
/// probe port, answering in standby too, when the main port does not
/// accept yet.
pub async fn serve_health(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let router = Router::new().route("/healthz", get(|| async { StatusCode::NO_CONTENT }));
    axum::serve(listener, router).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_owner_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = lock_path(dir.path());
        let first = StateLock::try_acquire(&path).unwrap().expect("free");
        assert!(StateLock::try_acquire(&path).unwrap().is_none());
        drop(first);
        assert!(StateLock::try_acquire(&path).unwrap().is_some());
    }

    #[tokio::test]
    async fn standby_takes_over_when_released() {
        let dir = tempfile::tempdir().unwrap();
        let path = lock_path(dir.path());
        let first = StateLock::try_acquire(&path).unwrap().expect("free");
        let waiter = tokio::spawn({
            let path = path.clone();
            async move { StateLock::acquire(&path).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!waiter.is_finished(), "took the lock while it was held");
        drop(first);
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .expect("took over")
            .unwrap()
            .unwrap();
        assert_eq!(second.path(), path);
    }
}
