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
            metadata: Default::default(),
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
        // A poller reports a content change as an mtime change (`WriteTime`):
        // that is a WRITE. Every other metadata change is CHMOD: inotify
        // reports each attribute change (chmod, chown, utimes, a `touch`,
        // xattrs, link count) as the unspecific `Any`, and the closest
        // contract kind for time, ownership and permission changes is CHMOD.
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)) => event
            .paths
            .iter()
            .map(|p| make(p, FsEventType::Write, None))
            .collect(),
        EventKind::Modify(ModifyKind::Metadata(_)) => event
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

/// Creates for what already exists in a directory that appeared while a
/// recursive native watch was running (Linux).
///
/// inotify has no recursive mode: `notify` adds the watch of a new
/// subdirectory only after it delivered the directory's own creation, so
/// `mkdir -p a/b/c && touch a/b/c/f` would report `a` and nothing below it.
/// [`deep::DeepScan`] scans each new directory shortly afterwards, once its
/// watch exists, and reports the entries nobody reported yet; entries the
/// watch reports itself are never reported twice, in either order.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod deep {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use cua_proto::env::v1::{FsEvent, FsEventType};

    use super::WatchMessage;
    use crate::util::now_ts;

    /// How long after a directory appeared its content is scanned. `notify`
    /// adds the watch right after the event batch that announced the
    /// directory; this is generous for a loaded machine.
    const SCAN_DELAY: Duration = Duration::from_millis(100);
    /// Entries one scan reports at most (a runaway tree must not stall the
    /// worker; the consumer sees an overflow instead).
    const SCAN_LIMIT: usize = 100_000;
    /// How long a reported path is remembered for de-duplication.
    const MEMORY: Duration = Duration::from_secs(30);
    /// Remembered paths beyond which expired ones are dropped.
    const PRUNE_ABOVE: usize = 4096;

    #[derive(Default)]
    struct Seen {
        /// Created entries the watch reported.
        watched: HashMap<PathBuf, Instant>,
        /// Created entries the scan reported; the watch may still report
        /// them, which is then suppressed.
        scanned: HashMap<PathBuf, Instant>,
    }

    fn prune(map: &mut HashMap<PathBuf, Instant>) {
        if map.len() > PRUNE_ABOVE {
            map.retain(|_, at| at.elapsed() < MEMORY);
        }
    }

    /// De-duplication state and the scan worker of one watch.
    pub struct DeepScan {
        seen: Arc<Mutex<Seen>>,
        tx: Mutex<Sender<(PathBuf, Instant)>>,
    }

    impl DeepScan {
        /// Starts the worker; it ends when the `DeepScan` is dropped.
        pub fn new(deliver: Arc<dyn Fn(WatchMessage) + Send + Sync>) -> Self {
            let seen = Arc::new(Mutex::new(Seen::default()));
            let (tx, rx) = channel::<(PathBuf, Instant)>();
            let worker_seen = seen.clone();
            let spawned = std::thread::Builder::new()
                .name("fs-watch-scan".into())
                .spawn(move || {
                    while let Ok((dir, announced)) = rx.recv() {
                        std::thread::sleep(SCAN_DELAY.saturating_sub(announced.elapsed()));
                        scan(&dir, &worker_seen, &*deliver);
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!(%error, "directory scan worker did not start");
            }
            Self {
                seen,
                tx: Mutex::new(tx),
            }
        }

        /// Filters the messages of one native event: drops creates the scan
        /// already reported, remembers the others, and queues a scan for each
        /// directory that appeared.
        pub fn filter(&self, messages: Vec<WatchMessage>) -> Vec<WatchMessage> {
            let create = FsEventType::Create as i32;
            let mut out = Vec::with_capacity(messages.len());
            for message in messages {
                let WatchMessage::Event(e) = &message else {
                    out.push(message);
                    continue;
                };
                let path = PathBuf::from(&e.path);
                let mut seen = self.seen.lock().expect("watch dedupe");
                if e.r#type == create {
                    let duplicate = seen
                        .scanned
                        .remove(&path)
                        .is_some_and(|at| at.elapsed() < MEMORY);
                    prune(&mut seen.watched);
                    seen.watched.insert(path.clone(), Instant::now());
                    if duplicate {
                        continue;
                    }
                    drop(seen);
                    let is_dir = std::fs::symlink_metadata(&path)
                        .map(|m| m.is_dir())
                        .unwrap_or(false);
                    if is_dir {
                        let _ = self
                            .tx
                            .lock()
                            .expect("scan queue")
                            .send((path, Instant::now()));
                    }
                } else if e.r#type == FsEventType::Remove as i32
                    || e.r#type == FsEventType::Rename as i32
                {
                    // A later create of the same path is a new entry.
                    seen.scanned.remove(&path);
                    seen.watched.remove(&path);
                    if !e.old_path.is_empty() {
                        let old = PathBuf::from(&e.old_path);
                        seen.scanned.remove(&old);
                        seen.watched.remove(&old);
                    }
                }
                out.push(message);
            }
            out
        }
    }

    fn scan(dir: &Path, seen: &Mutex<Seen>, deliver: &(dyn Fn(WatchMessage) + Send + Sync)) {
        let mut pending = vec![dir.to_path_buf()];
        let mut count = 0usize;
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                count += 1;
                if count > SCAN_LIMIT {
                    deliver(WatchMessage::Overflow);
                    return;
                }
                let path = entry.path();
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    pending.push(path.clone());
                }
                {
                    let mut seen = seen.lock().expect("watch dedupe");
                    if seen.watched.contains_key(&path) || seen.scanned.contains_key(&path) {
                        continue;
                    }
                    prune(&mut seen.scanned);
                    seen.scanned.insert(path.clone(), Instant::now());
                }
                deliver(WatchMessage::Event(FsEvent {
                    path: path.display().to_string(),
                    r#type: FsEventType::Create as i32,
                    old_path: String::new(),
                    observed_at: Some(now_ts()),
                    metadata: Default::default(),
                }));
            }
        }
    }
}

/// Arms a watch on `path`, delivering to `sink`. `sink` must not block.
///
/// `namespace` is the full extended-attribute prefix whose values are
/// attached to each event as `FsEvent.metadata` (see [`super::xattr`]).
pub fn arm(
    path: &Path,
    recursive: bool,
    force_poll: bool,
    namespace: &str,
    sink: Arc<dyn Fn(WatchMessage) + Send + Sync>,
) -> notify::Result<ArmedWatch> {
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    // Every message leaves through here: event metadata is read at delivery.
    let deliver: Arc<dyn Fn(WatchMessage) + Send + Sync> = {
        let sink = sink.clone();
        let namespace = namespace.to_owned();
        Arc::new(move |mut message| {
            if let WatchMessage::Event(e) = &mut message {
                if e.r#type != FsEventType::Remove as i32 {
                    e.metadata = super::xattr::read(Path::new(&e.path), &namespace, false);
                }
            }
            sink(message)
        })
    };
    let make_handler = |native: bool| {
        let deliver = deliver.clone();
        // The poller rescans the whole tree itself and reports every new
        // entry; only a native recursive watch on Linux needs the scan of new
        // directories.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let deep = (native && recursive).then(|| Arc::new(deep::DeepScan::new(deliver.clone())));
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let _ = native;
        let handler = move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                #[allow(unused_mut)]
                let mut messages = map_event(event);
                #[cfg(any(target_os = "linux", target_os = "android"))]
                if let Some(deep) = &deep {
                    messages = deep.filter(messages);
                }
                for message in messages {
                    deliver(message);
                }
            }
            Err(error) => {
                tracing::debug!(%error, "watch error");
                deliver(WatchMessage::Overflow);
            }
        };
        handler
    };
    if !force_poll {
        match notify::recommended_watcher(make_handler(true)).and_then(|mut watcher| {
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
    let mut watcher = notify::PollWatcher::new(make_handler(false), config)?;
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
    pub fn new(
        path: &Path,
        recursive: bool,
        force_poll: bool,
        namespace: &str,
    ) -> notify::Result<Self> {
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
        let armed = arm(path, recursive, force_poll, namespace, sink)?;
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
        let watcher = BufferedWatcher::new(
            &root,
            true,
            force_poll,
            super::super::xattr::DEFAULT_NAMESPACE,
        )
        .unwrap();
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

    fn kind_of(kind: EventKind) -> Vec<i32> {
        let event = notify::Event::new(kind).add_path(PathBuf::from("/x/f"));
        map_event(event)
            .into_iter()
            .map(|m| match m {
                WatchMessage::Event(e) => e.r#type,
                WatchMessage::Overflow => -1,
            })
            .collect()
    }

    #[test]
    fn attribute_changes_are_chmod_and_content_changes_are_write() {
        use notify::event::DataChange;
        let chmod = vec![FsEventType::Chmod as i32];
        let write = vec![FsEventType::Write as i32];
        // inotify reports every IN_ATTRIB (chmod, chown, utimes, touch) as Any.
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))),
            chmod
        );
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::Permissions
            ))),
            chmod
        );
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::Ownership
            ))),
            chmod
        );
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::Extended
            ))),
            chmod
        );
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::AccessTime
            ))),
            chmod
        );
        // The poller's content change is an mtime change: still a write.
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::WriteTime
            ))),
            write
        );
        assert_eq!(
            kind_of(EventKind::Modify(ModifyKind::Data(DataChange::Any))),
            write
        );
        assert_eq!(kind_of(EventKind::Modify(ModifyKind::Any)), write);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn collect(watcher: &BufferedWatcher, settle: Duration) -> Vec<(String, i32)> {
        tokio::time::sleep(settle).await;
        let (events, _) = watcher.drain(0);
        let root = watcher.path.display().to_string();
        events
            .into_iter()
            .map(|e| {
                let rel = e.path.strip_prefix(&root).unwrap_or(&e.path);
                (rel.trim_start_matches('/').to_owned(), e.r#type)
            })
            .collect()
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn native_touch_is_create_then_chmod_and_write_is_write() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let watcher =
            BufferedWatcher::new(&root, false, false, super::super::xattr::DEFAULT_NAMESPACE)
                .unwrap();
        let file = root.join("t.txt");
        std::fs::File::create(&file).unwrap();
        // touch: utimensat(NULL times = now) on the existing file.
        let c_path = std::ffi::CString::new(file.to_str().unwrap()).unwrap();
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), std::ptr::null(), 0) };
        assert_eq!(rc, 0);
        let seen = collect(&watcher, Duration::from_millis(500)).await;
        let create = FsEventType::Create as i32;
        let chmod = FsEventType::Chmod as i32;
        assert!(seen.contains(&("t.txt".into(), create)), "{seen:?}");
        assert!(seen.contains(&("t.txt".into(), chmod)), "{seen:?}");
        assert!(
            !seen.iter().any(|(_, k)| *k == FsEventType::Write as i32),
            "an attribute change is not a write: {seen:?}"
        );
        std::fs::write(&file, b"data").unwrap();
        let seen = collect(&watcher, Duration::from_millis(500)).await;
        assert!(
            seen.contains(&("t.txt".into(), FsEventType::Write as i32)),
            "{seen:?}"
        );
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        let seen = collect(&watcher, Duration::from_millis(500)).await;
        assert_eq!(seen, vec![("t.txt".into(), chmod)]);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn native_recursive_mkdir_reports_every_new_level() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let watcher =
            BufferedWatcher::new(&root, true, false, super::super::xattr::DEFAULT_NAMESPACE)
                .unwrap();
        // A directory tree with a file in it, created as fast as the kernel
        // allows: the file lands before the watch of its directory exists.
        for i in 0..20 {
            let tree = root.join(format!("d{i}/a/b"));
            std::fs::create_dir_all(&tree).unwrap();
            std::fs::write(tree.join("f.txt"), b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("deep/a/b")).unwrap();
        std::fs::write(root.join("deep/a/b/f.txt"), b"x").unwrap();
        let seen = collect(&watcher, Duration::from_millis(1000)).await;
        let create = FsEventType::Create as i32;
        let mut want = vec!["deep".to_owned()];
        for tree in std::iter::once("deep".to_owned()).chain((0..20).map(|i| format!("d{i}"))) {
            if tree != "deep" {
                want.push(tree.clone());
            }
            want.extend(["a", "a/b", "a/b/f.txt"].map(|n| format!("{tree}/{n}")));
        }
        want.sort();
        want.dedup();
        for name in &want {
            let count = seen
                .iter()
                .filter(|e| e.0 == *name && e.1 == create)
                .count();
            assert_eq!(count, 1, "{name} is created exactly once: {seen:?}");
        }
        // Writes to the files may be seen too (when the watch was in time),
        // but no create is repeated or invented.
        let creates = seen.iter().filter(|e| e.1 == create).count();
        assert_eq!(creates, want.len(), "{seen:?}");
        // The new subtree is watched from here on, and nothing is repeated.
        std::fs::write(root.join("deep/a/b/g.txt"), b"y").unwrap();
        let seen = collect(&watcher, Duration::from_millis(500)).await;
        assert_eq!(
            seen.iter()
                .filter(|e| e.0 == "deep/a/b/g.txt" && e.1 == create)
                .count(),
            1,
            "{seen:?}"
        );
        assert!(seen.iter().all(|e| e.0 == "deep/a/b/g.txt"), "{seen:?}");
    }

    #[test]
    fn buffer_overflow_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = BufferedWatcher::new(
            dir.path(),
            false,
            true,
            super::super::xattr::DEFAULT_NAMESPACE,
        )
        .unwrap();
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
