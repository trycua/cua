// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Directory watching: native events (inotify / FSEvents /
//! ReadDirectoryChangesW via `notify`) with a polling fallback.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_proto::env::v1::{FsEvent, FsEventType};
use notify::event::{EventKind, MetadataKind, ModifyKind, RenameMode};
use notify::{RecursiveMode, Watcher};

use crate::util::now_ts;

/// Interval of the polling fallback.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Buffer size of a polling-API watcher.
pub const WATCHER_BUFFER: usize = 10_000;

/// What a watch delivers to its consumer.
#[derive(Debug)]
pub enum WatchMessage {
    /// A change.
    Event(FsEvent),
    /// Events were lost (kernel queue overflow or a full buffer).
    Overflow,
}

/// Which backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Kernel notifications.
    Native,
    /// Periodic rescans.
    Poll,
}

/// An armed watch. Dropping it stops watching.
pub struct ArmedWatch {
    _watcher: Mutex<Box<dyn Watcher + Send>>,
    /// Backend in use.
    pub backend: Backend,
}

fn map_event(event: notify::Event) -> Vec<WatchMessage> {
    if event.need_rescan() {
        return vec![WatchMessage::Overflow];
    }
    let observed = now_ts();
    let make = |path: &Path, kind: FsEventType, old: Option<&Path>| {
        WatchMessage::Event(FsEvent {
            path: path.display().to_string(),
            r#type: kind as i32,
            old_path: old.map(|p| p.display().to_string()).unwrap_or_default(),
            observed_at: Some(observed),
        })
    };
    match event.kind {
        EventKind::Create(_) => event
            .paths
            .iter()
            .map(|p| make(p, FsEventType::Create, None))
            .collect(),
        EventKind::Remove(_) => event
            .paths
            .iter()
            .map(|p| make(p, FsEventType::Remove, None))
            .collect(),
        // Pollers (and some kernels) report content changes as an mtime
        // change; only permission/ownership changes are CHMOD.
        EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Permissions | MetadataKind::Ownership | MetadataKind::Extended,
        )) => event
            .paths
            .iter()
            .map(|p| make(p, FsEventType::Chmod, None))
            .collect(),
        EventKind::Modify(ModifyKind::Name(mode)) => match (mode, event.paths.as_slice()) {
            (RenameMode::Both, [from, to]) => vec![make(to, FsEventType::Rename, Some(from))],
            (RenameMode::From, paths) => paths
                .iter()
                .map(|p| make(p, FsEventType::Remove, None))
                .collect(),
            (RenameMode::To, paths) => paths
                .iter()
                .map(|p| make(p, FsEventType::Create, None))
                .collect(),
            // FSEvents reports renames without direction; report the path
            // as renamed and let clients stat it.
            (_, paths) => paths
                .iter()
                .map(|p| make(p, FsEventType::Rename, None))
                .collect(),
        },
        EventKind::Modify(_) => event
            .paths
            .iter()
            .map(|p| make(p, FsEventType::Write, None))
            .collect(),
        EventKind::Access(_) | EventKind::Any | EventKind::Other => Vec::new(),
    }
}

/// Arms a watch on `path`, delivering to `sink`. `sink` must not block.
pub fn arm(
    path: &Path,
    recursive: bool,
    force_poll: bool,
    sink: Arc<dyn Fn(WatchMessage) + Send + Sync>,
) -> notify::Result<ArmedWatch> {
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    let handler = {
        let sink = sink.clone();
        move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                for message in map_event(event) {
                    sink(message);
                }
            }
            Err(error) => {
                tracing::debug!(%error, "watch error");
                sink(WatchMessage::Overflow);
            }
        }
    };
    if !force_poll {
        match notify::recommended_watcher(handler.clone()).and_then(|mut watcher| {
            watcher.watch(path, mode)?;
            Ok(watcher)
        }) {
            Ok(watcher) => {
                return Ok(ArmedWatch {
                    _watcher: Mutex::new(Box::new(watcher)),
                    backend: Backend::Native,
                })
            }
            Err(error) => {
                tracing::info!(%error, path = %path.display(), "native watch failed; polling instead");
            }
        }
    }
    let config = notify::Config::default()
        .with_poll_interval(POLL_INTERVAL)
        .with_compare_contents(false);
    let mut watcher = notify::PollWatcher::new(handler, config)?;
    watcher.watch(path, mode)?;
    Ok(ArmedWatch {
        _watcher: Mutex::new(Box::new(watcher)),
        backend: Backend::Poll,
    })
}

/// A server-side watcher buffering events for `GetWatcherEvents`.
pub struct BufferedWatcher {
    /// Root path.
    pub path: PathBuf,
    buffer: Arc<Mutex<VecDeque<FsEvent>>>,
    overflowed: Arc<AtomicBool>,
    last_poll: Mutex<Instant>,
    _armed: ArmedWatch,
}

impl BufferedWatcher {
    /// Creates and arms a buffered watcher.
    pub fn new(path: &Path, recursive: bool, force_poll: bool) -> notify::Result<Self> {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let overflowed = Arc::new(AtomicBool::new(false));
        let sink: Arc<dyn Fn(WatchMessage) + Send + Sync> = {
            let buffer = buffer.clone();
            let overflowed = overflowed.clone();
            Arc::new(move |message| match message {
                WatchMessage::Event(event) => {
                    let mut buffer = buffer.lock().expect("watch buffer");
                    if buffer.len() >= WATCHER_BUFFER {
                        buffer.pop_front();
                        overflowed.store(true, Ordering::SeqCst);
                    }
                    buffer.push_back(event);
                }
                WatchMessage::Overflow => overflowed.store(true, Ordering::SeqCst),
            })
        };
        let armed = arm(path, recursive, force_poll, sink)?;
        Ok(Self {
            path: path.to_path_buf(),
            buffer,
            overflowed,
            last_poll: Mutex::new(Instant::now()),
            _armed: armed,
        })
    }

    /// Drains up to `max` events (0 = all) and the overflow flag.
    pub fn drain(&self, max: usize) -> (Vec<FsEvent>, bool) {
        *self.last_poll.lock().expect("poll lock") = Instant::now();
        let mut buffer = self.buffer.lock().expect("watch buffer");
        let take = if max == 0 {
            buffer.len()
        } else {
            max.min(buffer.len())
        };
        let events = buffer.drain(..take).collect();
        (events, self.overflowed.swap(false, Ordering::SeqCst))
    }

    /// Time since the last drain.
    pub fn idle(&self) -> Duration {
        self.last_poll.lock().expect("poll lock").elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_for(watcher: &BufferedWatcher, want: impl Fn(&FsEvent) -> bool) -> Vec<FsEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let (events, _) = watcher.drain(0);
            seen.extend(events);
            if seen.iter().any(&want) {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("event not observed; saw {seen:?}");
    }

    async fn exercise(force_poll: bool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let watcher = BufferedWatcher::new(&root, true, force_poll).unwrap();
        // Let the watch settle (FSEvents latency, first poll scan).
        tokio::time::sleep(Duration::from_millis(700)).await;
        let file = root.join("a.txt");
        std::fs::write(&file, b"one").unwrap();
        let name = |e: &FsEvent| e.path.ends_with("a.txt");
        wait_for(&watcher, |e| {
            name(e)
                && (e.r#type == FsEventType::Create as i32 || e.r#type == FsEventType::Write as i32)
        })
        .await;
        std::fs::write(&file, b"two, longer").unwrap();
        // The poller compares mtimes, and two writes can land in the same
        // timestamp tick (coarse-granularity filesystems, a loaded CI box).
        // Move the mtime well clear of the first write so the change is
        // observable on every backend without depending on timing.
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        wait_for(&watcher, |e| {
            name(e) && e.r#type == FsEventType::Write as i32
        })
        .await;
        std::fs::remove_file(&file).unwrap();
        wait_for(&watcher, |e| {
            name(e)
                && (e.r#type == FsEventType::Remove as i32
                    || e.r#type == FsEventType::Rename as i32)
        })
        .await;
    }

    #[tokio::test]
    async fn native_watcher_sees_create_write_remove() {
        exercise(false).await;
    }

    #[tokio::test]
    async fn polling_watcher_sees_create_write_remove() {
        exercise(true).await;
    }

    #[test]
    fn buffer_overflow_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = BufferedWatcher::new(dir.path(), false, true).unwrap();
        {
            let mut buffer = watcher.buffer.lock().unwrap();
            for i in 0..WATCHER_BUFFER {
                buffer.push_back(FsEvent {
                    path: i.to_string(),
                    ..Default::default()
                });
            }
        }
        watcher.overflowed.store(true, Ordering::SeqCst);
        let (events, overflowed) = watcher.drain(10);
        assert_eq!(events.len(), 10);
        assert!(overflowed);
        let (_, again) = watcher.drain(0);
        assert!(!again, "overflow flag resets after a drain");
    }
}
