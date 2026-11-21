// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! In-memory read-time state for the guards: element fingerprints per
//! snapshot, and the latest window image plus window fingerprint per session
//! and window. Each runtime's tool registry owns one store and clears a
//! session's entries when that session ends. Nothing goes to disk.
//! Fingerprints of retired snapshots stay for the same bounded window so an
//! older token can be checked instead of refused.

use super::fingerprint::ElementFingerprint;
use super::pixel::GrayImage;
use super::window::WindowFingerprint;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// Snapshots whose fingerprints are kept (8 per pid in the snapshot LRU,
/// times a few busy pids).
pub const SNAPSHOT_CAPACITY: usize = 64;
/// Windows whose read-time image is kept.
pub const IMAGE_CAPACITY: usize = 16;

#[derive(Clone, Debug)]
pub struct SnapshotFingerprints {
    /// The session whose read published the snapshot.
    pub session: Option<String>,
    pub pid: i32,
    pub window_id: u64,
    pub elements: Vec<ElementFingerprint>,
    /// Cheap stats for target classification.
    pub element_count: usize,
    pub thin_frames: usize,
}

#[derive(Clone, Debug)]
pub struct ReadImage {
    pub snapshot_id: u32,
    pub image: GrayImage,
    pub window: WindowFingerprint,
}

/// `(session, pid, window_id)`: each session keeps its own read of a window.
type WindowKey = (Option<String>, i32, u64);

fn key(session: Option<&str>, pid: i32, window_id: u64) -> WindowKey {
    (session.map(str::to_owned), pid, window_id)
}

#[derive(Default)]
struct State {
    snapshots: HashMap<u32, SnapshotFingerprints>,
    snapshot_order: VecDeque<u32>,
    images: HashMap<WindowKey, ReadImage>,
    image_order: VecDeque<WindowKey>,
    /// Latest window fingerprint even when no image was kept.
    windows: HashMap<WindowKey, WindowFingerprint>,
}

impl State {
    /// The session's latest snapshot of the window.
    fn latest(
        &self,
        session: Option<&str>,
        pid: i32,
        window_id: u64,
    ) -> Option<&SnapshotFingerprints> {
        self.snapshot_order
            .iter()
            .rev()
            .filter_map(|id| self.snapshots.get(id))
            .find(|f| f.session.as_deref() == session && f.pid == pid && f.window_id == window_id)
    }
}

#[derive(Default)]
pub struct GuardStore {
    inner: Mutex<State>,
}

impl GuardStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put_snapshot(&self, snapshot_id: u32, fps: SnapshotFingerprints) {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if s.snapshots.insert(snapshot_id, fps).is_none() {
            s.snapshot_order.push_back(snapshot_id);
        }
        while s.snapshot_order.len() > SNAPSHOT_CAPACITY {
            if let Some(old) = s.snapshot_order.pop_front() {
                s.snapshots.remove(&old);
            }
        }
    }

    pub fn element(
        &self,
        snapshot_id: u32,
        index: usize,
    ) -> Option<(i32, u64, ElementFingerprint)> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let snap = s.snapshots.get(&snapshot_id)?;
        snap.elements
            .get(index)
            .map(|fp| (snap.pid, snap.window_id, fp.clone()))
    }

    pub fn stats(&self, snapshot_id: u32) -> Option<(usize, usize)> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.snapshots
            .get(&snapshot_id)
            .map(|f| (f.element_count, f.thin_frames))
    }

    pub fn latest_stats(
        &self,
        session: Option<&str>,
        pid: i32,
        window_id: u64,
    ) -> Option<(usize, usize)> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.latest(session, pid, window_id)
            .map(|f| (f.element_count, f.thin_frames))
    }

    /// The smallest element of the session's latest snapshot of the window
    /// whose read-time frame contains the screen point: what the agent saw
    /// there.
    pub fn element_at(
        &self,
        session: Option<&str>,
        pid: i32,
        window_id: u64,
        x: f64,
        y: f64,
    ) -> Option<ElementFingerprint> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let snap = s.latest(session, pid, window_id)?;
        snap.elements
            .iter()
            .filter(|fp| fp.frame.is_some_and(|f| f.contains(x, y) && f.area() > 0.0))
            .min_by(|a, b| {
                let (a, b) = (a.frame.unwrap().area(), b.frame.unwrap().area());
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    pub fn put_window(&self, session: Option<&str>, pid: i32, window: WindowFingerprint) {
        let key = key(session, pid, window.window_id);
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.windows.insert(key.clone(), window);
        if s.windows.len() > 4 * IMAGE_CAPACITY {
            let keep: Vec<_> = s.image_order.iter().cloned().collect();
            s.windows.retain(|k, _| keep.contains(k));
            s.windows.insert(key, window);
        }
    }

    pub fn window(
        &self,
        session: Option<&str>,
        pid: i32,
        window_id: u64,
    ) -> Option<WindowFingerprint> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.windows.get(&key(session, pid, window_id)).copied()
    }

    pub fn put_image(&self, session: Option<&str>, pid: i32, image: ReadImage) {
        let key = key(session, pid, image.window.window_id);
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.windows.insert(key.clone(), image.window);
        if s.images.insert(key.clone(), image).is_none() {
            s.image_order.push_back(key);
        } else {
            s.image_order.retain(|k| *k != key);
            s.image_order.push_back(key);
        }
        while s.image_order.len() > IMAGE_CAPACITY {
            if let Some(old) = s.image_order.pop_front() {
                s.images.remove(&old);
            }
        }
    }

    pub fn image(&self, session: Option<&str>, pid: i32, window_id: u64) -> Option<ReadImage> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.images.get(&key(session, pid, window_id)).cloned()
    }

    /// Drop everything `session`'s reads recorded.
    pub fn retire_session(&self, session: &str) {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let owned = |k: &WindowKey| k.0.as_deref() == Some(session);
        s.images.retain(|k, _| !owned(k));
        s.image_order.retain(|k| !owned(k));
        s.windows.retain(|k, _| !owned(k));
        s.snapshots
            .retain(|_, f| f.session.as_deref() != Some(session));
        let State {
            snapshots,
            snapshot_order,
            ..
        } = &mut *s;
        snapshot_order.retain(|id| snapshots.contains_key(id));
    }
}

#[cfg(test)]
mod tests {
    use super::super::Rect;
    use super::*;

    fn win(id: u64) -> WindowFingerprint {
        WindowFingerprint {
            window_id: id,
            pid: 1,
            frame: Rect::new(0.0, 0.0, 10.0, 10.0),
            scale: 1.0,
        }
    }

    #[test]
    fn retired_snapshot_fingerprints_stay_until_capacity() {
        let store = GuardStore::new();
        for id in 0..(SNAPSHOT_CAPACITY as u32 + 5) {
            store.put_snapshot(
                id,
                SnapshotFingerprints {
                    session: None,
                    pid: 1,
                    window_id: 2,
                    elements: vec![ElementFingerprint {
                        role: format!("r{id}"),
                        ..Default::default()
                    }],
                    element_count: 1,
                    thin_frames: 0,
                },
            );
        }
        assert!(store.element(0, 0).is_none());
        assert_eq!(store.element(10, 0).unwrap().2.role, "r10");
        assert!(store.element(10, 1).is_none());
        assert_eq!(store.latest_stats(None, 1, 2), Some((1, 0)));
    }

    #[test]
    fn element_at_picks_the_smallest_containing_frame() {
        let store = GuardStore::new();
        let el = |role: &str, f: Rect| ElementFingerprint {
            role: role.into(),
            frame: Some(f),
            ..Default::default()
        };
        store.put_snapshot(
            1,
            SnapshotFingerprints {
                session: None,
                pid: 1,
                window_id: 2,
                elements: vec![
                    el("AXGroup", Rect::new(0.0, 0.0, 500.0, 500.0)),
                    el("AXButton", Rect::new(10.0, 10.0, 80.0, 24.0)),
                ],
                element_count: 2,
                thin_frames: 0,
            },
        );
        assert_eq!(
            store.element_at(None, 1, 2, 20.0, 20.0).unwrap().role,
            "AXButton"
        );
        assert_eq!(
            store.element_at(None, 1, 2, 300.0, 300.0).unwrap().role,
            "AXGroup"
        );
        assert!(store.element_at(None, 1, 2, 900.0, 900.0).is_none());
        assert!(store.element_at(None, 1, 3, 20.0, 20.0).is_none());
    }

    #[test]
    fn images_are_bounded_and_latest_wins() {
        let store = GuardStore::new();
        for id in 0..(IMAGE_CAPACITY as u64 + 3) {
            store.put_image(
                None,
                1,
                ReadImage {
                    snapshot_id: 1,
                    image: GrayImage::filled(2, 2, 0),
                    window: win(id),
                },
            );
        }
        assert!(store.image(None, 1, 0).is_none());
        store.put_image(
            None,
            1,
            ReadImage {
                snapshot_id: 9,
                image: GrayImage::filled(2, 2, 0),
                window: win(5),
            },
        );
        assert_eq!(store.image(None, 1, 5).unwrap().snapshot_id, 9);
    }

    fn read(store: &GuardStore, session: &str, snapshot_id: u32) {
        store.put_snapshot(
            snapshot_id,
            SnapshotFingerprints {
                session: Some(session.into()),
                pid: 1,
                window_id: 2,
                elements: vec![ElementFingerprint {
                    role: session.into(),
                    frame: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
                    ..Default::default()
                }],
                element_count: 1,
                thin_frames: 0,
            },
        );
        store.put_image(
            Some(session),
            1,
            ReadImage {
                snapshot_id,
                image: GrayImage::filled(2, 2, 0),
                window: win(2),
            },
        );
    }

    #[test]
    fn each_session_sees_only_its_own_reads() {
        let store = GuardStore::new();
        read(&store, "a", 1);
        read(&store, "b", 2);
        assert_eq!(store.image(Some("a"), 1, 2).unwrap().snapshot_id, 1);
        assert_eq!(store.image(Some("b"), 1, 2).unwrap().snapshot_id, 2);
        assert!(store.image(None, 1, 2).is_none());
        assert_eq!(
            store.element_at(Some("a"), 1, 2, 5.0, 5.0).unwrap().role,
            "a"
        );
        assert_eq!(
            store.element_at(Some("b"), 1, 2, 5.0, 5.0).unwrap().role,
            "b"
        );
        assert!(store.latest_stats(Some("c"), 1, 2).is_none());
    }

    #[test]
    fn retiring_a_session_drops_only_its_reads() {
        let store = GuardStore::new();
        read(&store, "a", 1);
        read(&store, "b", 2);
        store.retire_session("a");
        assert!(store.image(Some("a"), 1, 2).is_none());
        assert!(store.window(Some("a"), 1, 2).is_none());
        assert!(store.element(1, 0).is_none());
        assert!(store.latest_stats(Some("a"), 1, 2).is_none());
        assert_eq!(store.image(Some("b"), 1, 2).unwrap().snapshot_id, 2);
        assert_eq!(store.element(2, 0).unwrap().2.role, "b");
    }
}
