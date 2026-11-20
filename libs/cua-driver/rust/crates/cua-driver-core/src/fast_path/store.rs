// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! In-memory read-time state for the guards: element fingerprints per
//! snapshot, and the latest window image plus window fingerprint per window.
//! Nothing goes to disk. Fingerprints of retired snapshots stay for the same
//! bounded window so an older token can be checked instead of refused.

use super::fingerprint::ElementFingerprint;
use super::pixel::GrayImage;
use super::window::WindowFingerprint;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Snapshots whose fingerprints are kept (8 per pid in the snapshot LRU,
/// times a few busy pids).
pub const SNAPSHOT_CAPACITY: usize = 64;
/// Windows whose read-time image is kept.
pub const IMAGE_CAPACITY: usize = 16;

#[derive(Clone, Debug)]
pub struct SnapshotFingerprints {
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

#[derive(Default)]
struct State {
    snapshots: HashMap<u32, SnapshotFingerprints>,
    snapshot_order: VecDeque<u32>,
    images: HashMap<(i32, u64), ReadImage>,
    image_order: VecDeque<(i32, u64)>,
    /// Latest window fingerprint even when no image was kept.
    windows: HashMap<(i32, u64), WindowFingerprint>,
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

    pub fn latest_stats(&self, pid: i32, window_id: u64) -> Option<(usize, usize)> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.snapshot_order
            .iter()
            .rev()
            .filter_map(|id| s.snapshots.get(id))
            .find(|f| f.pid == pid && f.window_id == window_id)
            .map(|f| (f.element_count, f.thin_frames))
    }

    /// The smallest element of the window's latest snapshot whose read-time
    /// frame contains the screen point: what the agent saw there.
    pub fn element_at(
        &self,
        pid: i32,
        window_id: u64,
        x: f64,
        y: f64,
    ) -> Option<ElementFingerprint> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let snap = s
            .snapshot_order
            .iter()
            .rev()
            .filter_map(|id| s.snapshots.get(id))
            .find(|f| f.pid == pid && f.window_id == window_id)?;
        snap.elements
            .iter()
            .filter(|fp| fp.frame.is_some_and(|f| f.contains(x, y) && f.area() > 0.0))
            .min_by(|a, b| {
                let (a, b) = (a.frame.unwrap().area(), b.frame.unwrap().area());
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    pub fn put_window(&self, pid: i32, window: WindowFingerprint) {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.windows.insert((pid, window.window_id), window);
        if s.windows.len() > 4 * IMAGE_CAPACITY {
            let keep: Vec<_> = s.image_order.iter().copied().collect();
            s.windows.retain(|k, _| keep.contains(k));
            s.windows.insert((pid, window.window_id), window);
        }
    }

    pub fn window(&self, pid: i32, window_id: u64) -> Option<WindowFingerprint> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.windows.get(&(pid, window_id)).copied()
    }

    pub fn put_image(&self, pid: i32, image: ReadImage) {
        let key = (pid, image.window.window_id);
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.windows.insert(key, image.window);
        if s.images.insert(key, image).is_none() {
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

    pub fn image(&self, pid: i32, window_id: u64) -> Option<ReadImage> {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.images.get(&(pid, window_id)).cloned()
    }

    pub fn forget_window(&self, pid: i32, window_id: u64) {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        s.images.remove(&(pid, window_id));
        s.image_order.retain(|k| *k != (pid, window_id));
        s.windows.remove(&(pid, window_id));
    }
}

/// The daemon-wide store.
pub fn global() -> &'static GuardStore {
    static STORE: OnceLock<GuardStore> = OnceLock::new();
    STORE.get_or_init(GuardStore::new)
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
        assert_eq!(store.latest_stats(1, 2), Some((1, 0)));
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
        assert_eq!(store.element_at(1, 2, 20.0, 20.0).unwrap().role, "AXButton");
        assert_eq!(
            store.element_at(1, 2, 300.0, 300.0).unwrap().role,
            "AXGroup"
        );
        assert!(store.element_at(1, 2, 900.0, 900.0).is_none());
        assert!(store.element_at(1, 3, 20.0, 20.0).is_none());
    }

    #[test]
    fn images_are_bounded_and_latest_wins() {
        let store = GuardStore::new();
        for id in 0..(IMAGE_CAPACITY as u64 + 3) {
            store.put_image(
                1,
                ReadImage {
                    snapshot_id: 1,
                    image: GrayImage::filled(2, 2, 0),
                    window: win(id),
                },
            );
        }
        assert!(store.image(1, 0).is_none());
        store.put_image(
            1,
            ReadImage {
                snapshot_id: 9,
                image: GrayImage::filled(2, 2, 0),
                window: win(5),
            },
        );
        assert_eq!(store.image(1, 5).unwrap().snapshot_id, 9);
        store.forget_window(1, 5);
        assert!(store.image(1, 5).is_none() && store.window(1, 5).is_none());
    }
}
