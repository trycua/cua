// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The presence cursor-shape prober: what shape the guest would show at each
//! participant's cursor (PRESENCE.md section 3).
//!
//! One worker thread resolves requests from the hub, latest-wins per
//! participant, in this order:
//!
//! 1. **System**: the participant owns the real pointer (its principal's input
//!    was the last injected, within [`ProberConfig::own_window`], and the
//!    pointer is still where that input and the participant's cursor are).
//!    The OS's real cursor is read.
//! 2. **Cache**: a result for the same region (the hit element's rect, else a
//!    [`ProberConfig::cell`]-sized cell) younger than
//!    [`ProberConfig::region_ttl`].
//! 3. **Probe**: the per-Space setting is on, the backend can warp and read
//!    the pointer, the pointer is idle (no injection pending and none within
//!    [`ProberConfig::idle_quiet`]) and the pointer lock is free:
//!    `probe_by_warp` moves the pointer there, reads it, and restores it,
//!    aborting the moment an injection is announced.
//! 4. **Hit-test**: the accessibility element under the point.
//!
//! Non-cached work for one participant runs at most every
//! [`ProberConfig::per_participant`]; only one probe runs at a time (one
//! worker). Nothing here holds the hub lock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use cua_driver_core::cursor_shape::{ResizeAxis, SystemCursorShape};
use cua_driver_core::pointer_shape::{
    probe_by_warp, BackendNames, HitTest, PointerShapeBackend, ProbeConfig, ProbeOutcome,
    ScreenRect,
};
use cua_proto::env::v1::{CursorPosition, CursorShape, CursorShapeSource};
use cua_spacesd_session::input_activity::InputActivity;

use super::presence::{PresenceHub, ShapeRequest, ShapeRequests};

/// Maps a presence cursor to global screen coordinates (the backend's space).
pub trait ScreenMapper: Send + Sync {
    /// The point, and the scope id (window id or display id) its region
    /// cache is keyed by.
    fn to_screen(&self, cursor: &CursorPosition) -> Option<(f64, f64, String)>;
}

/// Largest element rect (in backend units squared) reused from the cache.
const MAX_CACHED_RECT_AREA: f64 = 256.0 * 256.0;

/// Prober tunables.
#[derive(Debug, Clone, Copy)]
pub struct ProberConfig {
    pub region_ttl: Duration,
    pub cell: f64,
    pub per_participant: Duration,
    pub idle_quiet: Duration,
    pub own_window: Duration,
    /// How close the real pointer must be to the recorded input position.
    pub own_tolerance: f64,
    /// How close the real pointer must be to the participant's cursor.
    pub cursor_tolerance: f64,
    pub probe: ProbeConfig,
}

impl Default for ProberConfig {
    fn default() -> Self {
        Self {
            region_ttl: Duration::from_secs(1),
            cell: 16.0,
            per_participant: Duration::from_millis(100),
            idle_quiet: Duration::from_millis(750),
            own_window: Duration::from_secs(1),
            own_tolerance: 2.0,
            cursor_tolerance: 3.0,
            probe: ProbeConfig::default(),
        }
    }
}

/// The capability attributes of a backend.
pub fn capability_attributes(names: BackendNames, probe_on: bool) -> Vec<(String, String)> {
    let or_none = |name: &str| {
        if name.is_empty() {
            "none".to_owned()
        } else {
            name.to_owned()
        }
    };
    let probe = if names.probe.is_empty() || names.system.is_empty() {
        "none".to_owned()
    } else if probe_on {
        names.probe.to_owned()
    } else {
        "off".to_owned()
    };
    vec![
        ("hit_test".into(), or_none(names.hit_test)),
        ("system".into(), or_none(names.system)),
        ("probe".into(), probe),
    ]
}

/// The protocol shape of a driver shape; `None` for "cannot tell".
pub fn proto_shape(shape: &SystemCursorShape) -> Option<CursorShape> {
    Some(match shape {
        SystemCursorShape::Default => CursorShape::Arrow,
        SystemCursorShape::Text | SystemCursorShape::VerticalText => CursorShape::Text,
        SystemCursorShape::Pointer => CursorShape::Pointer,
        SystemCursorShape::Grab => CursorShape::Grab,
        SystemCursorShape::Grabbing => CursorShape::Grabbing,
        SystemCursorShape::Crosshair => CursorShape::Crosshair,
        SystemCursorShape::Wait => CursorShape::Wait,
        SystemCursorShape::Progress => CursorShape::Progress,
        SystemCursorShape::NotAllowed => CursorShape::NotAllowed,
        SystemCursorShape::Resize(axis) => match axis {
            ResizeAxis::NorthSouth | ResizeAxis::Row => CursorShape::ResizeNs,
            ResizeAxis::EastWest | ResizeAxis::Column => CursorShape::ResizeEw,
            ResizeAxis::NorthEastSouthWest => CursorShape::ResizeNesw,
            ResizeAxis::NorthWestSouthEast => CursorShape::ResizeNwse,
            ResizeAxis::All => CursorShape::Move,
        },
        // An app-specific bitmap has no portable name: the arrow.
        SystemCursorShape::Custom { .. } => CursorShape::Arrow,
        SystemCursorShape::Unknown => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Resolved {
    shape: CursorShape,
    source: CursorShapeSource,
}

struct CellEntry {
    resolved: Resolved,
    at: Instant,
}

struct RectEntry {
    scope: String,
    rect: ScreenRect,
    resolved: Resolved,
    at: Instant,
}

#[derive(Default)]
struct Queue {
    pending: HashMap<String, ShapeRequest>,
    last_work: HashMap<String, Instant>,
    cells: HashMap<(String, i64, i64), CellEntry>,
    rects: Vec<RectEntry>,
}

struct Inner {
    backend: Arc<dyn PointerShapeBackend>,
    mapper: Arc<dyn ScreenMapper>,
    activity: Arc<InputActivity>,
    probe_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    hub: Weak<PresenceHub>,
    config: ProberConfig,
    queue: Mutex<Queue>,
    wake: Condvar,
    stop: AtomicBool,
}

/// See the module docs. Dropping it stops the worker.
pub struct ShapeProber {
    inner: Arc<Inner>,
}

/// The hub's handle to the prober.
struct Sink(Weak<Inner>);

impl ShapeRequests for Sink {
    fn request(&self, request: ShapeRequest) {
        if let Some(inner) = self.0.upgrade() {
            inner
                .queue
                .lock()
                .unwrap()
                .pending
                .insert(request.participant_id.clone(), request);
            inner.wake.notify_one();
        }
    }
}

impl ShapeProber {
    /// Start the worker and route `hub`'s shape requests to it.
    pub fn start(
        hub: &Arc<PresenceHub>,
        backend: Arc<dyn PointerShapeBackend>,
        mapper: Arc<dyn ScreenMapper>,
        activity: Arc<InputActivity>,
        probe_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        config: ProberConfig,
    ) -> Arc<Self> {
        let inner = Arc::new(Inner {
            backend,
            mapper,
            activity,
            probe_enabled,
            hub: Arc::downgrade(hub),
            config,
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        hub.set_shape_requests(Arc::new(Sink(Arc::downgrade(&inner))));
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("presence-shape".into())
            .spawn(move || worker.run())
            .expect("spawning the presence shape worker");
        Arc::new(Self { inner })
    }

    /// Backend names (capability attributes).
    pub fn names(&self) -> BackendNames {
        self.inner.backend.names()
    }

    /// Backend limitation.
    pub fn limitation(&self) -> Option<String> {
        self.inner.backend.limitation()
    }

    /// Whether the idle probe is currently enabled.
    pub fn probe_enabled(&self) -> bool {
        (self.inner.probe_enabled)()
    }
}

impl Drop for ShapeProber {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.wake.notify_all();
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

impl Inner {
    fn run(self: Arc<Self>) {
        loop {
            if self.stop.load(Ordering::SeqCst) || self.hub.strong_count() == 0 {
                return;
            }
            let next = {
                let mut queue = self.queue.lock().unwrap();
                let now = Instant::now();
                let due = queue
                    .pending
                    .keys()
                    .find(|id| {
                        queue.last_work.get(*id).is_none_or(|last| {
                            now.duration_since(*last) >= self.config.per_participant
                        })
                    })
                    .cloned();
                match due {
                    Some(id) => queue.pending.remove(&id),
                    None => {
                        // Sleep until woken or the soonest throttle ends.
                        let wait = if queue.pending.is_empty() {
                            Duration::from_millis(250)
                        } else {
                            Duration::from_millis(10)
                        };
                        let _ = self.wake.wait_timeout(queue, wait).unwrap();
                        None
                    }
                }
            };
            let Some(request) = next else {
                continue;
            };
            let Some(resolved) = self.resolve(&request) else {
                continue;
            };
            let Some(hub) = self.hub.upgrade() else {
                return;
            };
            hub.set_shape(&request.participant_id, resolved.shape, resolved.source);
        }
    }

    fn resolve(&self, request: &ShapeRequest) -> Option<Resolved> {
        if !request.cursor.visible {
            return None;
        }
        let (x, y, scope) = self.mapper.to_screen(&request.cursor)?;
        let names = self.backend.names();
        // 1. System: this participant's input put the pointer here.
        if !names.system.is_empty() && self.owns_pointer(&request.principal_id, (x, y)) {
            if let Some(shape) = proto_shape(&self.backend.system_shape()) {
                return Some(Resolved {
                    shape,
                    source: CursorShapeSource::System,
                });
            }
        }
        // 2. Cache.
        let cell = (
            scope.clone(),
            (x / self.config.cell).floor() as i64,
            (y / self.config.cell).floor() as i64,
        );
        {
            let mut queue = self.queue.lock().unwrap();
            let ttl = self.config.region_ttl;
            queue.rects.retain(|entry| entry.at.elapsed() < ttl);
            queue.cells.retain(|_, entry| entry.at.elapsed() < ttl);
            if let Some(entry) = queue
                .rects
                .iter()
                .find(|entry| entry.scope == scope && entry.rect.contains(x, y))
            {
                return Some(entry.resolved);
            }
            if let Some(entry) = queue.cells.get(&cell) {
                return Some(entry.resolved);
            }
            queue
                .last_work
                .insert(request.participant_id.clone(), Instant::now());
        }
        // 3. Idle probe.
        if let Some(shape) = self.probe((x, y), names) {
            let resolved = Resolved {
                shape,
                source: CursorShapeSource::Probe,
            };
            self.queue.lock().unwrap().cells.insert(
                cell,
                CellEntry {
                    resolved,
                    at: Instant::now(),
                },
            );
            return Some(resolved);
        }
        // 4. Hit-test.
        let hit = self.backend.hit_test(x, y)?;
        let resolved = Resolved {
            shape: proto_shape(&hit.shape).unwrap_or(CursorShape::Arrow),
            source: CursorShapeSource::HitTest,
        };
        self.remember(&scope, cell, &hit, resolved);
        Some(resolved)
    }

    fn remember(&self, scope: &str, cell: (String, i64, i64), hit: &HitTest, resolved: Resolved) {
        let mut queue = self.queue.lock().unwrap();
        let now = Instant::now();
        // Reuse an element's rect only for control-sized elements inside
        // their window: a large rect (a desktop background, a whole terminal)
        // may be partly covered by other windows, which the cache cannot
        // see, so those answer per 16 px cell instead.
        let reusable = |rect: &cua_driver_core::pointer_shape::ScreenRect| {
            rect.area() > 0.0
                && rect.area() <= MAX_CACHED_RECT_AREA
                && hit.window.is_some_and(|w| {
                    rect.x >= w.x
                        && rect.y >= w.y
                        && rect.x + rect.width <= w.x + w.width
                        && rect.y + rect.height <= w.y + w.height
                })
        };
        match hit.element.filter(reusable) {
            Some(rect) => {
                // Bounded: at most 256 live element rects.
                if queue.rects.len() >= 256 {
                    queue.rects.remove(0);
                }
                queue.rects.push(RectEntry {
                    scope: scope.to_owned(),
                    rect,
                    resolved,
                    at: now,
                });
            }
            None => {
                queue.cells.insert(cell, CellEntry { resolved, at: now });
            }
        }
    }

    fn owns_pointer(&self, principal: &str, cursor: (f64, f64)) -> bool {
        if principal.is_empty() || self.activity.pending() > 0 {
            return false;
        }
        let Some(last) = self.activity.last() else {
            return false;
        };
        if last.principal != principal || last.at.elapsed() > self.config.own_window {
            return false;
        }
        let Some(pointer) = self.backend.pointer_position() else {
            return false;
        };
        if let Some(input) = last.position {
            if distance(pointer, input) > self.config.own_tolerance {
                return false;
            }
        }
        distance(pointer, cursor) <= self.config.cursor_tolerance
    }

    fn probe(&self, target: (f64, f64), names: BackendNames) -> Option<CursorShape> {
        if names.probe.is_empty() || names.system.is_empty() || !(self.probe_enabled)() {
            return None;
        }
        if !self.activity.idle_for(self.config.idle_quiet) {
            return None;
        }
        let _lock = self.activity.try_lock_pointer()?;
        let activity = self.activity.clone();
        match probe_by_warp(
            self.backend.as_ref(),
            target,
            self.config.probe,
            &move || activity.pending() == 0,
        ) {
            ProbeOutcome::Shape(shape) => proto_shape(&shape),
            ProbeOutcome::Aborted | ProbeOutcome::Unsupported => None,
        }
    }
}

/// Adapts the installed `&'static` backend to the prober's `Arc`.
pub struct InstalledBackend(pub &'static dyn PointerShapeBackend);

impl PointerShapeBackend for InstalledBackend {
    fn names(&self) -> BackendNames {
        self.0.names()
    }
    fn hit_test(&self, x: f64, y: f64) -> Option<HitTest> {
        self.0.hit_test(x, y)
    }
    fn system_shape(&self) -> SystemCursorShape {
        self.0.system_shape()
    }
    fn pointer_position(&self) -> Option<(f64, f64)> {
        self.0.pointer_position()
    }
    fn warp_pointer(&self, x: f64, y: f64) -> bool {
        self.0.warp_pointer(x, y)
    }
    fn limitation(&self) -> Option<String> {
        self.0.limitation()
    }
}

/// Install this platform's pointer-shape backend (idempotent) and return it.
pub fn install_platform_backend() -> Option<Arc<dyn PointerShapeBackend>> {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        #[cfg(target_os = "linux")]
        let _ = platform_linux::install_pointer_shape_backend();
        #[cfg(target_os = "macos")]
        let _ = platform_macos::install_pointer_shape_backend();
        #[cfg(target_os = "windows")]
        let _ = platform_windows::install_pointer_shape_backend();
    });
    cua_driver_core::pointer_shape::pointer_shape_backend()
        .map(|backend| Arc::new(InstalledBackend(backend)) as Arc<dyn PointerShapeBackend>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_proto::env::v1::{join_response, Point, Principal, PrincipalKind};
    use std::sync::atomic::AtomicUsize;

    /// A fake screen: x < 100 is a text field (element rect 0..100), 100..200
    /// a link (no rect), else nothing. The real cursor follows the pointer.
    struct Fake {
        pointer: Mutex<(f64, f64)>,
        moves: Mutex<Vec<(f64, f64)>>,
        warp: bool,
        hits: AtomicUsize,
        system_reads: AtomicUsize,
        /// Called on every warp (to inject "real input" mid-probe).
        on_warp: Mutex<Option<Box<dyn Fn() + Send>>>,
    }

    impl Fake {
        fn new(warp: bool) -> Arc<Self> {
            Arc::new(Self {
                pointer: Mutex::new((900.0, 900.0)),
                moves: Mutex::new(Vec::new()),
                warp,
                hits: AtomicUsize::new(0),
                system_reads: AtomicUsize::new(0),
                on_warp: Mutex::new(None),
            })
        }
        fn shape_at(x: f64) -> SystemCursorShape {
            if x < 100.0 {
                SystemCursorShape::Text
            } else if x < 200.0 {
                SystemCursorShape::Pointer
            } else {
                SystemCursorShape::Default
            }
        }
    }

    impl PointerShapeBackend for Fake {
        fn names(&self) -> BackendNames {
            BackendNames {
                hit_test: "fake",
                system: "fake",
                probe: if self.warp { "fake" } else { "" },
            }
        }
        fn hit_test(&self, x: f64, _y: f64) -> Option<HitTest> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            // The hit-test "misreads" the link as the arrow, so tests can
            // tell a hit-test answer from a probe answer.
            Some(HitTest {
                shape: if x < 100.0 {
                    SystemCursorShape::Text
                } else {
                    SystemCursorShape::Default
                },
                role: "fake".into(),
                window: Some(ScreenRect::new(0.0, 0.0, 1000.0, 1000.0)),
                // A text field at x < 100; beyond x 800 the "desktop", whose
                // element is the whole screen.
                element: if x < 100.0 {
                    Some(ScreenRect::new(0.0, 0.0, 100.0, 600.0))
                } else if x > 800.0 {
                    Some(ScreenRect::new(0.0, 0.0, 1000.0, 1000.0))
                } else {
                    None
                },
            })
        }
        fn system_shape(&self) -> SystemCursorShape {
            self.system_reads.fetch_add(1, Ordering::SeqCst);
            Self::shape_at(self.pointer.lock().unwrap().0)
        }
        fn pointer_position(&self) -> Option<(f64, f64)> {
            Some(*self.pointer.lock().unwrap())
        }
        fn warp_pointer(&self, x: f64, y: f64) -> bool {
            if !self.warp {
                return false;
            }
            self.moves.lock().unwrap().push((x, y));
            *self.pointer.lock().unwrap() = (x, y);
            if let Some(hook) = self.on_warp.lock().unwrap().as_ref() {
                hook();
            }
            true
        }
    }

    /// Normalized 0..1 over a 1000x1000 display.
    struct Display;

    impl ScreenMapper for Display {
        fn to_screen(&self, cursor: &CursorPosition) -> Option<(f64, f64, String)> {
            let point = cursor.position.as_ref()?;
            Some((point.x * 1000.0, point.y * 1000.0, "display:0".into()))
        }
    }

    fn fast() -> ProberConfig {
        ProberConfig {
            per_participant: Duration::from_millis(0),
            idle_quiet: Duration::from_millis(50),
            probe: ProbeConfig {
                dwell: Duration::from_millis(5),
                poll: Duration::from_millis(1),
                restore_tolerance: 0.5,
            },
            ..ProberConfig::default()
        }
    }

    struct Rig {
        hub: Arc<PresenceHub>,
        fake: Arc<Fake>,
        activity: Arc<InputActivity>,
        probe_on: Arc<AtomicBool>,
        _prober: Arc<ShapeProber>,
        rx: tokio::sync::mpsc::Receiver<cua_proto::env::v1::JoinResponse>,
        id: String,
    }

    fn rig(warp: bool) -> Rig {
        rig_with(warp, fast())
    }

    fn rig_with(warp: bool, config: ProberConfig) -> Rig {
        let hub = PresenceHub::new();
        let fake = Fake::new(warp);
        let activity = InputActivity::new();
        let probe_on = Arc::new(AtomicBool::new(true));
        let flag = probe_on.clone();
        let prober = ShapeProber::start(
            &hub,
            fake.clone(),
            Arc::new(Display),
            activity.clone(),
            Arc::new(move || flag.load(Ordering::SeqCst)),
            config,
        );
        let (me, _, rx) = hub.join_with(
            Principal {
                id: "alice".into(),
                display_name: "Alice".into(),
                color: String::new(),
                kind: PrincipalKind::Human as i32,
            },
            super::super::presence::JoinOptions {
                cursor_shapes: true,
                ..Default::default()
            },
        );
        Rig {
            hub,
            fake,
            activity,
            probe_on,
            _prober: prober,
            rx,
            id: me.participant_id,
        }
    }

    fn at(x: f64, y: f64) -> CursorPosition {
        CursorPosition {
            position: Some(Point { x, y }),
            visible: true,
            ..CursorPosition::default()
        }
    }

    /// The next shape change for the owner, bounded.
    async fn next_shape(rig: &mut Rig) -> (i32, i32) {
        for _ in 0..50 {
            let event = tokio::time::timeout(Duration::from_secs(2), rig.rx.recv())
                .await
                .expect("a shape within 2 s")
                .expect("stream open");
            if let Some(join_response::Event::CursorShapeChanged(changed)) = event.event {
                return (changed.shape, changed.source);
            }
        }
        panic!("no shape change");
    }

    #[tokio::test]
    async fn an_idle_pointer_is_probed_and_restored_exactly() {
        let mut rig = rig(true);
        rig.hub.update_cursor(&rig.id, at(0.15, 0.5));
        assert_eq!(
            next_shape(&mut rig).await,
            (CursorShape::Pointer as i32, CursorShapeSource::Probe as i32),
            "the probe reads the link's real cursor"
        );
        assert_eq!(
            rig.fake.pointer_position(),
            Some((900.0, 900.0)),
            "restored"
        );
        assert_eq!(
            *rig.fake.moves.lock().unwrap(),
            vec![(150.0, 500.0), (900.0, 900.0)]
        );
    }

    #[tokio::test]
    async fn recent_or_pending_input_skips_the_probe_for_the_hit_test() {
        // A quiet window a slow runner cannot outlast between the input and
        // the cursor update (50 ms flaked on Windows CI).
        let mut rig = rig_with(
            true,
            ProberConfig {
                idle_quiet: Duration::from_millis(500),
                ..fast()
            },
        );
        rig.activity.note("someone-else", Some((5.0, 5.0)));
        rig.hub.update_cursor(&rig.id, at(0.15, 0.5));
        assert_eq!(
            next_shape(&mut rig).await,
            (CursorShape::Arrow as i32, CursorShapeSource::HitTest as i32)
        );
        assert!(rig.fake.moves.lock().unwrap().is_empty(), "never moved");
        // A pending injection also blocks it, once the input is no longer
        // recent.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let announced = rig.activity.announce("bob");
        rig.hub.update_cursor(&rig.id, at(0.05, 0.5));
        assert_eq!(
            next_shape(&mut rig).await,
            (CursorShape::Text as i32, CursorShapeSource::HitTest as i32)
        );
        assert!(rig.fake.moves.lock().unwrap().is_empty());
        drop(announced);
    }

    #[tokio::test]
    async fn input_during_a_probe_aborts_restores_and_falls_back() {
        let mut rig = rig(true);
        let activity = rig.activity.clone();
        let held: Arc<Mutex<Vec<cua_spacesd_session::input_activity::Announced>>> = Arc::default();
        let keep = held.clone();
        // Real input arrives the moment the probe moves the pointer.
        *rig.fake.on_warp.lock().unwrap() = Some(Box::new(move || {
            let mut held = keep.lock().unwrap();
            if held.is_empty() {
                held.push(activity.announce("bob"));
            }
        }));
        rig.hub.update_cursor(&rig.id, at(0.15, 0.5));
        assert_eq!(
            next_shape(&mut rig).await,
            (CursorShape::Arrow as i32, CursorShapeSource::HitTest as i32),
            "aborted probe falls back to the hit-test"
        );
        assert_eq!(
            rig.fake.pointer_position(),
            Some((900.0, 900.0)),
            "restored first"
        );
        held.lock().unwrap().clear();
    }

    #[tokio::test]
    async fn setting_off_or_no_warp_uses_the_hit_test() {
        let mut rig = rig(true);
        rig.probe_on.store(false, Ordering::SeqCst);
        rig.hub.update_cursor(&rig.id, at(0.15, 0.5));
        assert_eq!(
            next_shape(&mut rig).await.1,
            CursorShapeSource::HitTest as i32
        );
        assert!(rig.fake.moves.lock().unwrap().is_empty());

        let mut rig = rig_no_warp();
        rig.hub.update_cursor(&rig.id, at(0.05, 0.5));
        assert_eq!(
            next_shape(&mut rig).await,
            (CursorShape::Text as i32, CursorShapeSource::HitTest as i32)
        );
    }

    fn rig_no_warp() -> Rig {
        rig(false)
    }

    #[tokio::test]
    async fn the_participant_driving_the_pointer_gets_the_real_cursor() {
        let mut rig = rig(true);
        // Alice's own input put the pointer on the link.
        *rig.fake.pointer.lock().unwrap() = (150.0, 500.0);
        rig.activity.note("alice", Some((150.0, 500.0)));
        rig.hub.update_cursor(&rig.id, at(0.15, 0.5));
        assert_eq!(
            next_shape(&mut rig).await,
            (
                CursorShape::Pointer as i32,
                CursorShapeSource::System as i32
            )
        );
        assert!(rig.fake.moves.lock().unwrap().is_empty(), "no probe needed");
    }

    #[tokio::test]
    async fn results_are_cached_per_region() {
        let mut rig = rig(false);
        rig.hub.update_cursor(&rig.id, at(0.05, 0.5));
        next_shape(&mut rig).await;
        let hits = rig.fake.hits.load(Ordering::SeqCst);
        // Same element rect, another cell: no new hit-test.
        rig.hub.update_cursor(&rig.id, at(0.06, 0.55));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(rig.fake.hits.load(Ordering::SeqCst), hits);
        // Leaving the element costs one hit-test.
        rig.hub.update_cursor(&rig.id, at(0.5, 0.5));
        assert_eq!(next_shape(&mut rig).await.0, CursorShape::Arrow as i32);
        assert_eq!(rig.fake.hits.load(Ordering::SeqCst), hits + 1);
    }

    /// Regression (live on XFCE): the desktop background's element is the
    /// whole screen; reusing it answered the arrow over every window above
    /// it for a second.
    #[tokio::test]
    async fn a_screen_sized_element_is_not_reused_over_other_windows() {
        let mut rig = rig(false);
        rig.hub.update_cursor(&rig.id, at(0.9, 0.9));
        assert_eq!(next_shape(&mut rig).await.0, CursorShape::Arrow as i32);
        let hits = rig.fake.hits.load(Ordering::SeqCst);
        // Inside the desktop's rect, but over the text field.
        rig.hub.update_cursor(&rig.id, at(0.05, 0.5));
        assert_eq!(next_shape(&mut rig).await.0, CursorShape::Text as i32);
        assert_eq!(rig.fake.hits.load(Ordering::SeqCst), hits + 1);
    }

    #[tokio::test]
    async fn probes_per_participant_are_rate_limited() {
        let hub = PresenceHub::new();
        let fake = Fake::new(false);
        let prober = ShapeProber::start(
            &hub,
            fake.clone(),
            Arc::new(Display),
            InputActivity::new(),
            Arc::new(|| true),
            ProberConfig {
                per_participant: Duration::from_millis(100),
                region_ttl: Duration::from_millis(0),
                ..fast()
            },
        );
        let (me, _, _rx) = hub.join_with(
            Principal {
                id: "a".into(),
                ..Principal::default()
            },
            super::super::presence::JoinOptions {
                cursor_shapes: true,
                ..Default::default()
            },
        );
        let started = Instant::now();
        for step in 0..60u32 {
            hub.update_cursor(&me.participant_id, at(f64::from(step % 50) / 50.0, 0.5));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        let elapsed = started.elapsed().as_secs_f64();
        let hits = fake.hits.load(Ordering::SeqCst) as f64;
        assert!(
            hits <= elapsed * 10.0 + 2.0,
            "{hits} hit-tests in {elapsed:.2}s"
        );
        assert!(hits >= 2.0);
        drop(prober);
    }

    #[test]
    fn capability_attributes_name_the_backends() {
        let names = BackendNames {
            hit_test: "atspi",
            system: "xfixes",
            probe: "xtest",
        };
        let attrs = |on| capability_attributes(names, on);
        assert!(attrs(true).contains(&("probe".into(), "xtest".into())));
        assert!(attrs(false).contains(&("probe".into(), "off".into())));
        let wayland = BackendNames {
            hit_test: "atspi",
            system: "",
            probe: "",
        };
        let attrs = capability_attributes(wayland, true);
        assert!(attrs.contains(&("system".into(), "none".into())));
        assert!(attrs.contains(&("probe".into(), "none".into())));
    }

    #[test]
    fn driver_shapes_map_to_the_protocol() {
        assert_eq!(
            proto_shape(&SystemCursorShape::Default),
            Some(CursorShape::Arrow)
        );
        assert_eq!(
            proto_shape(&SystemCursorShape::Resize(ResizeAxis::All)),
            Some(CursorShape::Move)
        );
        assert_eq!(
            proto_shape(&SystemCursorShape::Resize(ResizeAxis::NorthWestSouthEast)),
            Some(CursorShape::ResizeNwse)
        );
        assert_eq!(
            proto_shape(&SystemCursorShape::Progress),
            Some(CursorShape::Progress)
        );
        assert_eq!(proto_shape(&SystemCursorShape::Unknown), None);
    }
}
