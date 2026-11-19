// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! In-process runtime below PiP, recording, and remote transport bindings.
//!
//! Producers never call consumers directly. Each subscriber owns a mailbox
//! with an ordered control queue and a single replaceable frame slot. A slow
//! renderer therefore observes the newest frame without applying backpressure
//! to ScreenCaptureKit or another platform capture callback.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

pub mod input_activity;
pub mod media;
mod service;

pub use service::{Connection, OutboundPacket, ServerRuntime};

pub use cua_media_protocol::{CodecEpoch, FrameSequence, GeometryEpoch, SurfaceGeometry};
pub use cua_spacesd_provider_api::ProviderTargetId as WindowTarget;

#[derive(Debug, Clone)]
pub enum FramePayload {
    Png(Arc<[u8]>),
    /// H.264 access unit in Annex B format. Keyframes contain SPS/PPS before
    /// the IDR so a new codec epoch can be decoded independently.
    H264 {
        bytes: Arc<[u8]>,
        codec_epoch: CodecEpoch,
        keyframe: bool,
        encode_duration_us: Option<u32>,
    },
    /// Packed, top-down BGRA. Platform callbacks copy into this owned form so
    /// native buffer lifetimes never escape their framework callback.
    Bgra {
        bytes: Arc<[u8]>,
        bytes_per_row: u32,
    },
}

#[derive(Debug, Clone)]
pub struct PendingFrame {
    pub target: WindowTarget,
    pub geometry: SurfaceGeometry,
    pub capture_timestamp_us: u64,
    pub payload: FramePayload,
}

#[derive(Debug, Clone)]
pub struct SessionFrame {
    pub target: WindowTarget,
    pub sequence: FrameSequence,
    pub geometry_epoch: GeometryEpoch,
    pub geometry: SurfaceGeometry,
    pub capture_timestamp_us: u64,
    /// Actions for which this is the first captured frame afterward.
    /// This provides ordering only; it does not claim the UI visibly changed.
    pub first_after_actions: Vec<ActionSequence>,
    pub payload: FramePayload,
}

#[derive(Debug, Clone)]
pub struct ActionDispatch {
    pub target: Option<WindowTarget>,
    pub tool: String,
    pub label: String,
    pub wall_timestamp_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionSequence(pub u64);

#[derive(Debug, Clone)]
pub struct ActionObservation {
    pub sequence: ActionSequence,
    pub target: Option<WindowTarget>,
    pub tool: String,
    pub label: String,
    pub wall_timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TargetSnapshot {
    pub geometry: Option<SurfaceGeometry>,
    pub geometry_epoch: GeometryEpoch,
    pub frame_sequence: FrameSequence,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeLifecycleEvent {
    Opened {
        target: WindowTarget,
        geometry_epoch: GeometryEpoch,
        geometry: SurfaceGeometry,
    },
    GeometryChanged {
        target: WindowTarget,
        geometry_epoch: GeometryEpoch,
        geometry: SurfaceGeometry,
    },
    TitleChanged {
        target: WindowTarget,
        title: String,
    },
    Suspended {
        target: WindowTarget,
        reason: String,
    },
    Resumed {
        target: WindowTarget,
    },
    Closed {
        target: WindowTarget,
    },
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Action(ActionObservation),
    Lifecycle(RuntimeLifecycleEvent),
    /// Lossless action-to-frame ordering metadata. The corresponding video
    /// frame may be replaced in a slow subscriber's latest-frame slot.
    FirstFrameAfterActions {
        target: WindowTarget,
        frame_sequence: FrameSequence,
        actions: Vec<ActionSequence>,
    },
}

pub trait WindowSessionSubscriber: Send + Sync + 'static {
    fn on_frame(&self, _frame: Arc<SessionFrame>) {}
    fn on_event(&self, _event: Arc<SessionEvent>) {}
}

/// Platform capture ownership used by PiP policy and explicit sessions.
/// Owner IDs are daemon-local and never cross the protocol boundary.
pub trait WindowCaptureBackend: Send + Sync + 'static {
    fn acquire(&self, owner_id: &str, target: WindowTarget) -> Result<(), String>;
    fn release(&self, owner_id: &str);
}

pub enum PollingCaptureOutcome {
    Frame {
        geometry: SurfaceGeometry,
        capture_timestamp_us: u64,
        payload: FramePayload,
    },
    Suspended(String),
    Closed,
}

/// Adapter boundary for platforms that already have a reliable one-frame
/// window capture primitive. It provides a portable persistent source now;
/// WGC/PipeWire event-driven sources can replace it behind the same ownership
/// contract later.
pub trait PollingWindowSource: Send + Sync + 'static {
    fn poll(&self, target: &WindowTarget) -> Result<PollingCaptureOutcome, String>;

    fn frame_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(67)
    }
}

struct PollingTargetWorker {
    owners: std::collections::HashSet<String>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct PollingOwnership {
    owners: HashMap<String, WindowTarget>,
    targets: HashMap<WindowTarget, PollingTargetWorker>,
}

struct PollingCaptureInner {
    hub: WindowSessionHub,
    source: Arc<dyn PollingWindowSource>,
    ownership: Mutex<PollingOwnership>,
}

impl Drop for PollingCaptureInner {
    fn drop(&mut self) {
        let workers = self
            .ownership
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .targets
            .drain()
            .map(|(_, worker)| worker)
            .collect::<Vec<_>>();
        for mut worker in workers {
            worker.stop.store(true, Ordering::Release);
            if let Some(join) = worker.join.take() {
                let _ = join.join();
            }
        }
    }
}

#[derive(Clone)]
pub struct PollingWindowCaptureManager {
    inner: Arc<PollingCaptureInner>,
}

impl PollingWindowCaptureManager {
    pub fn new(hub: WindowSessionHub, source: Arc<dyn PollingWindowSource>) -> Self {
        Self {
            inner: Arc::new(PollingCaptureInner {
                hub,
                source,
                ownership: Mutex::new(PollingOwnership::default()),
            }),
        }
    }

    fn retire(mut worker: PollingTargetWorker) {
        worker.stop.store(true, Ordering::Release);
        if let Some(join) = worker.join.take() {
            std::thread::spawn(move || {
                let _ = join.join();
            });
        }
    }

    fn spawn_worker(&self, target: WindowTarget) -> PollingTargetWorker {
        let hub = self.inner.hub.clone();
        let source = self.inner.source.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_target = target.clone();
        let join = std::thread::Builder::new()
            .name("rcdp-capture-poll".to_owned())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    match source.poll(&worker_target) {
                        Ok(PollingCaptureOutcome::Frame {
                            geometry,
                            capture_timestamp_us,
                            payload,
                        }) => {
                            hub.publish_live_frame(
                                worker_target.clone(),
                                geometry,
                                capture_timestamp_us,
                                payload,
                            );
                        }
                        Ok(PollingCaptureOutcome::Suspended(reason)) => {
                            hub.mark_source_suspended(&worker_target, reason);
                        }
                        Ok(PollingCaptureOutcome::Closed) => {
                            hub.mark_target_closed(&worker_target);
                            break;
                        }
                        Err(error) => {
                            hub.mark_source_suspended(
                                &worker_target,
                                format!("capture_failed:{error}"),
                            );
                        }
                    }
                    std::thread::sleep(source.frame_interval());
                }
            })
            .expect("failed to start polling window capture worker");
        PollingTargetWorker {
            owners: std::collections::HashSet::new(),
            stop,
            join: Some(join),
        }
    }
}

impl WindowCaptureBackend for PollingWindowCaptureManager {
    fn acquire(&self, owner_id: &str, target: WindowTarget) -> Result<(), String> {
        let mut retired = None;
        let mut ownership = self
            .inner
            .ownership
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = ownership.owners.get(owner_id).cloned() {
            if previous == target {
                return Ok(());
            }
            if let Some(worker) = ownership.targets.get_mut(&previous) {
                worker.owners.remove(owner_id);
                if worker.owners.is_empty() {
                    retired = ownership.targets.remove(&previous);
                }
            }
        }
        if !ownership.targets.contains_key(&target) {
            ownership
                .targets
                .insert(target.clone(), self.spawn_worker(target.clone()));
        }
        ownership
            .targets
            .get_mut(&target)
            .expect("target worker was inserted")
            .owners
            .insert(owner_id.to_owned());
        ownership.owners.insert(owner_id.to_owned(), target);
        drop(ownership);
        if let Some(worker) = retired {
            Self::retire(worker);
        }
        Ok(())
    }

    fn release(&self, owner_id: &str) {
        let retired = {
            let mut ownership = self
                .inner
                .ownership
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(target) = ownership.owners.remove(owner_id) else {
                return;
            };
            let empty = ownership.targets.get_mut(&target).is_some_and(|worker| {
                worker.owners.remove(owner_id);
                worker.owners.is_empty()
            });
            empty.then(|| ownership.targets.remove(&target)).flatten()
        };
        if let Some(worker) = retired {
            Self::retire(worker);
        }
    }
}

#[derive(Debug, Clone)]
pub struct WindowEncoderConfig {
    pub max_fps: u16,
    pub max_dimension: u32,
}

#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub sequence: FrameSequence,
    pub geometry_epoch: GeometryEpoch,
    pub codec_epoch: u64,
    pub width_px: u32,
    pub height_px: u32,
    pub capture_timestamp_us: u64,
    pub keyframe: bool,
    pub bytes: Arc<[u8]>,
}

pub trait EncodedFrameSink: Send + Sync + 'static {
    fn on_encoded_frame(&self, frame: EncodedVideoFrame);
}

/// A bounded asynchronous encoder. `submit` must never block a capture or hub
/// subscriber thread; implementations replace or drop obsolete queued frames.
pub trait WindowFrameEncoder: Send + Sync + 'static {
    fn submit(&self, frame: Arc<SessionFrame>);
    fn request_keyframe(&self);
}

pub trait WindowEncoderFactory: Send + Sync + 'static {
    fn create(
        &self,
        target: &WindowTarget,
        config: WindowEncoderConfig,
        sink: Arc<dyn EncodedFrameSink>,
    ) -> Result<Arc<dyn WindowFrameEncoder>, String>;
}

#[derive(Default)]
struct TargetState {
    geometry: Option<SurfaceGeometry>,
    geometry_epoch: u64,
    next_frame_sequence: u64,
    live_source: bool,
    source_suspended: bool,
    pending_actions: Vec<ActionSequence>,
}

#[derive(Default)]
struct MailboxState {
    control: VecDeque<Arc<SessionEvent>>,
    latest_frame: Option<Arc<SessionFrame>>,
    closed: bool,
}

struct SubscriberMailbox {
    state: Mutex<MailboxState>,
    ready: Condvar,
}

impl SubscriberMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(MailboxState::default()),
            ready: Condvar::new(),
        }
    }

    fn push_event(&self, event: Arc<SessionEvent>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.control.push_back(event);
        self.ready.notify_one();
    }

    fn replace_frame(&self, frame: Arc<SessionFrame>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.latest_frame = Some(frame);
        self.ready.notify_one();
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        self.ready.notify_one();
    }
}

struct HubInner {
    subscribers: Mutex<HashMap<u64, Arc<SubscriberMailbox>>>,
    targets: Mutex<HashMap<WindowTarget, TargetState>>,
    publish_order: Mutex<()>,
    next_subscriber_id: AtomicU64,
    next_action_sequence: AtomicU64,
}

#[derive(Clone)]
pub struct WindowSessionHub {
    inner: Arc<HubInner>,
}

impl Default for WindowSessionHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSessionHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(HubInner {
                subscribers: Mutex::new(HashMap::new()),
                targets: Mutex::new(HashMap::new()),
                publish_order: Mutex::new(()),
                next_subscriber_id: AtomicU64::new(1),
                next_action_sequence: AtomicU64::new(1),
            }),
        }
    }

    /// Register a consumer. Process-lifetime consumers may ignore the ID;
    /// connection-owned consumers must pass it to [`Self::unsubscribe`].
    pub fn subscribe(
        &self,
        name: impl Into<String>,
        subscriber: impl WindowSessionSubscriber,
    ) -> u64 {
        let id = self
            .inner
            .next_subscriber_id
            .fetch_add(1, Ordering::Relaxed);
        let mailbox = Arc::new(SubscriberMailbox::new());
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, mailbox.clone());

        let name = name.into();
        std::thread::Builder::new()
            .name(format!("cua-spacesd-session-{name}"))
            .spawn(move || subscriber_loop(mailbox, subscriber))
            .unwrap_or_else(|error| {
                panic!("failed to start window session subscriber {name}: {error}")
            });
        id
    }

    /// Remove a consumer and wake its worker so it can exit after delivering
    /// control events that were already queued.
    pub fn unsubscribe(&self, id: u64) -> bool {
        let mailbox = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        if let Some(mailbox) = mailbox {
            mailbox.close();
            true
        } else {
            false
        }
    }

    pub fn has_subscribers(&self) -> bool {
        !self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub fn target_snapshot(&self, target: &WindowTarget) -> Option<TargetSnapshot> {
        self.inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(target)
            .map(|state| TargetSnapshot {
                geometry: state.geometry.clone(),
                geometry_epoch: GeometryEpoch(state.geometry_epoch),
                frame_sequence: FrameSequence(state.next_frame_sequence),
            })
    }

    pub fn publish_action(&self, action: ActionDispatch) -> ActionSequence {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.publish_action_locked(action)
    }

    /// Whether an action still needs a fallback capture. Callers use this
    /// before doing synchronous screenshot work; publication rechecks the
    /// same conditions under the ordering lock.
    pub fn action_needs_fallback(
        &self,
        action: ActionSequence,
        target: Option<&WindowTarget>,
    ) -> bool {
        let Some(target) = target else {
            return true;
        };
        self.inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(target)
            .is_some_and(|state| !state.live_source && state.pending_actions.contains(&action))
    }

    /// Publish a post-dispatch fallback if no live or fallback frame has
    /// already crossed this action's boundary.
    pub fn publish_fallback_after_action(
        &self,
        action: ActionSequence,
        frame: PendingFrame,
    ) -> Option<Arc<SessionFrame>> {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let should_publish = self
            .inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&frame.target)
            .is_some_and(|state| !state.live_source && state.pending_actions.contains(&action));
        if should_publish {
            Some(self.publish_frame_locked(frame))
        } else {
            None
        }
    }

    fn publish_action_locked(&self, action: ActionDispatch) -> ActionSequence {
        let sequence = ActionSequence(
            self.inner
                .next_action_sequence
                .fetch_add(1, Ordering::Relaxed),
        );
        if let Some(target) = action.target.as_ref() {
            self.inner
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(target.clone())
                .or_default()
                .pending_actions
                .push(sequence);
        }
        self.broadcast_event(SessionEvent::Action(ActionObservation {
            sequence,
            target: action.target,
            tool: action.tool,
            label: action.label,
            wall_timestamp_ms: action.wall_timestamp_ms,
        }));
        sequence
    }

    pub fn publish_event(&self, event: SessionEvent) {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.broadcast_event(event);
    }

    fn broadcast_event(&self, event: SessionEvent) {
        let event = Arc::new(event);
        let subscribers: Vec<_> = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for subscriber in subscribers {
            subscriber.push_event(event.clone());
        }
    }

    pub fn publish_frame(
        &self,
        target: WindowTarget,
        geometry: SurfaceGeometry,
        capture_timestamp_us: u64,
        payload: FramePayload,
    ) -> Arc<SessionFrame> {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.publish_frame_locked(PendingFrame {
            target,
            geometry,
            capture_timestamp_us,
            payload,
        })
    }

    /// Publish the first or next frame from a persistent source and mark that
    /// source live before releasing publication order. Fallback rechecks can
    /// therefore never interleave between the frame and its liveness state.
    pub fn publish_live_frame(
        &self,
        target: WindowTarget,
        geometry: SurfaceGeometry,
        capture_timestamp_us: u64,
        payload: FramePayload,
    ) -> Arc<SessionFrame> {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = self.publish_frame_locked(PendingFrame {
            target: target.clone(),
            geometry,
            capture_timestamp_us,
            payload,
        });
        self.mark_live_source_locked(target);
        frame
    }

    fn publish_frame_locked(&self, pending: PendingFrame) -> Arc<SessionFrame> {
        let PendingFrame {
            target,
            geometry,
            capture_timestamp_us,
            payload,
        } = pending;
        let (sequence, geometry_epoch, first_after_actions, lifecycle) = {
            let mut targets = self
                .inner
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = targets.entry(target.clone()).or_default();
            let lifecycle = match &state.geometry {
                None => Some(RuntimeLifecycleEvent::Opened {
                    target: target.clone(),
                    geometry_epoch: GeometryEpoch(state.geometry_epoch),
                    geometry: geometry.clone(),
                }),
                Some(previous) if previous != &geometry => {
                    state.geometry_epoch = state.geometry_epoch.saturating_add(1);
                    Some(RuntimeLifecycleEvent::GeometryChanged {
                        target: target.clone(),
                        geometry_epoch: GeometryEpoch(state.geometry_epoch),
                        geometry: geometry.clone(),
                    })
                }
                Some(_) => None,
            };
            state.geometry = Some(geometry.clone());
            state.next_frame_sequence = state.next_frame_sequence.saturating_add(1);
            (
                FrameSequence(state.next_frame_sequence),
                GeometryEpoch(state.geometry_epoch),
                std::mem::take(&mut state.pending_actions),
                lifecycle,
            )
        };

        if let Some(event) = lifecycle {
            self.broadcast_event(SessionEvent::Lifecycle(event));
        }

        let frame = Arc::new(SessionFrame {
            target,
            sequence,
            geometry_epoch,
            geometry,
            capture_timestamp_us,
            first_after_actions,
            payload,
        });
        if !frame.first_after_actions.is_empty() {
            self.broadcast_event(SessionEvent::FirstFrameAfterActions {
                target: frame.target.clone(),
                frame_sequence: frame.sequence,
                actions: frame.first_after_actions.clone(),
            });
        }
        let subscribers: Vec<_> = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for subscriber in subscribers {
            subscriber.replace_frame(frame.clone());
        }
        frame
    }

    pub fn mark_live_source(&self, target: WindowTarget) {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.mark_live_source_locked(target);
    }

    fn mark_live_source_locked(&self, target: WindowTarget) {
        let resumed = {
            let mut targets = self
                .inner
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = targets.entry(target.clone()).or_default();
            let resumed = state.source_suspended;
            state.live_source = true;
            state.source_suspended = false;
            resumed
        };
        if resumed {
            self.broadcast_event(SessionEvent::Lifecycle(RuntimeLifecycleEvent::Resumed {
                target,
            }));
        }
    }

    pub fn mark_source_suspended(&self, target: &WindowTarget, reason: impl Into<String>) {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let should_publish = {
            let mut targets = self
                .inner
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = targets.entry(target.clone()).or_default();
            let was_live = state.live_source;
            let was_suspended = state.source_suspended;
            state.live_source = false;
            state.source_suspended = true;
            was_live || !was_suspended
        };
        if should_publish {
            self.broadcast_event(SessionEvent::Lifecycle(RuntimeLifecycleEvent::Suspended {
                target: target.clone(),
                reason: reason.into(),
            }));
        }
    }

    pub fn mark_geometry_changed(&self, target: &WindowTarget, geometry: SurfaceGeometry) {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let geometry_epoch = {
            let mut targets = self
                .inner
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = targets.entry(target.clone()).or_default();
            if state.geometry.as_ref() == Some(&geometry) {
                return;
            }
            state.geometry = Some(geometry.clone());
            state.geometry_epoch = state.geometry_epoch.saturating_add(1).max(1);
            GeometryEpoch(state.geometry_epoch)
        };
        self.broadcast_event(SessionEvent::Lifecycle(
            RuntimeLifecycleEvent::GeometryChanged {
                target: target.clone(),
                geometry_epoch,
                geometry,
            },
        ));
    }

    pub fn mark_title_changed(&self, target: &WindowTarget, title: impl Into<String>) {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self
            .inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(target)
        {
            self.broadcast_event(SessionEvent::Lifecycle(
                RuntimeLifecycleEvent::TitleChanged {
                    target: target.clone(),
                    title: title.into(),
                },
            ));
        }
    }

    pub fn mark_target_closed(&self, target: &WindowTarget) -> bool {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let existed = self
            .inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(target)
            .is_some();
        if existed {
            self.broadcast_event(SessionEvent::Lifecycle(RuntimeLifecycleEvent::Closed {
                target: target.clone(),
            }));
        }
        existed
    }

    /// Remove connection-owned stream state after an explicit session close
    /// without synthesizing a remote-window lifecycle event.
    pub fn forget_target(&self, target: &WindowTarget) -> bool {
        let _publish_order = self
            .inner
            .publish_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(target)
            .is_some()
    }

    pub fn is_live_source(&self, target: &WindowTarget) -> bool {
        self.inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(target)
            .is_some_and(|state| state.live_source)
    }

    pub fn is_action_pending(&self, target: &WindowTarget, action: ActionSequence) -> bool {
        self.inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(target)
            .is_some_and(|state| state.pending_actions.contains(&action))
    }
}

fn subscriber_loop(mailbox: Arc<SubscriberMailbox>, subscriber: impl WindowSessionSubscriber) {
    loop {
        enum Delivery {
            Event(Arc<SessionEvent>),
            Frame(Arc<SessionFrame>),
            Closed,
        }

        let delivery = {
            let mut state = mailbox
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while state.control.is_empty() && state.latest_frame.is_none() && !state.closed {
                state = mailbox
                    .ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if let Some(event) = state.control.pop_front() {
                Delivery::Event(event)
            } else if let Some(frame) = state.latest_frame.take() {
                Delivery::Frame(frame)
            } else {
                Delivery::Closed
            }
        };

        match delivery {
            Delivery::Event(event) => subscriber.on_event(event),
            Delivery::Frame(frame) => subscriber.on_frame(frame),
            Delivery::Closed => return,
        }
    }
}

static GLOBAL_HUB: OnceLock<WindowSessionHub> = OnceLock::new();

pub fn global_hub() -> &'static WindowSessionHub {
    GLOBAL_HUB.get_or_init(WindowSessionHub::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn target() -> WindowTarget {
        WindowTarget {
            key: cua_spacesd_provider_api::BackendTargetKey::new("test-target-7"),
            epoch: cua_media_protocol::TargetEpoch(1),
        }
    }

    fn geometry(width: u32) -> SurfaceGeometry {
        SurfaceGeometry {
            width_px: width,
            height_px: 100,
            scale_factor: 2.0,
        }
    }

    struct FrameSink(mpsc::Sender<u64>);

    impl WindowSessionSubscriber for FrameSink {
        fn on_frame(&self, frame: Arc<SessionFrame>) {
            self.0.send(frame.sequence.0).unwrap();
        }
    }

    #[test]
    fn frame_sequence_increments_and_geometry_change_bumps_epoch() {
        let hub = WindowSessionHub::new();
        let first = hub.publish_frame(
            target(),
            geometry(200),
            1,
            FramePayload::Png(Arc::from([1u8].as_slice())),
        );
        let second = hub.publish_frame(
            target(),
            geometry(200),
            2,
            FramePayload::Png(Arc::from([2u8].as_slice())),
        );
        let resized = hub.publish_frame(
            target(),
            geometry(300),
            3,
            FramePayload::Png(Arc::from([3u8].as_slice())),
        );

        assert_eq!(first.sequence, FrameSequence(1));
        assert_eq!(second.sequence, FrameSequence(2));
        assert_eq!(second.geometry_epoch, GeometryEpoch(0));
        assert_eq!(resized.geometry_epoch, GeometryEpoch(1));
    }

    #[test]
    fn subscriber_receives_frames_off_the_producer_thread() {
        let hub = WindowSessionHub::new();
        let (tx, rx) = mpsc::channel();
        hub.subscribe("test", FrameSink(tx));

        hub.publish_frame(
            target(),
            geometry(200),
            1,
            FramePayload::Png(Arc::from([1u8].as_slice())),
        );

        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    }

    struct BlockingSink {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        received: mpsc::Sender<u64>,
    }

    impl WindowSessionSubscriber for BlockingSink {
        fn on_frame(&self, frame: Arc<SessionFrame>) {
            self.received.send(frame.sequence.0).unwrap();
            if frame.sequence == FrameSequence(1) {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            }
        }
    }

    #[test]
    fn slow_subscriber_gets_latest_pending_frame() {
        let hub = WindowSessionHub::new();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (received_tx, received_rx) = mpsc::channel();
        hub.subscribe(
            "blocking",
            BlockingSink {
                entered: entered_tx,
                release: Mutex::new(release_rx),
                received: received_tx,
            },
        );

        hub.publish_frame(target(), geometry(200), 1, FramePayload::Png(Arc::from([])));
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        hub.publish_frame(target(), geometry(200), 2, FramePayload::Png(Arc::from([])));
        hub.publish_frame(target(), geometry(200), 3, FramePayload::Png(Arc::from([])));
        release_tx.send(()).unwrap();

        assert_eq!(received_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert_eq!(received_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 3);
    }

    #[derive(Debug, PartialEq, Eq)]
    enum OrderedObservation {
        Action(ActionSequence),
        Boundary {
            frame: FrameSequence,
            actions: Vec<ActionSequence>,
        },
    }

    struct MarkerSink {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        observations: mpsc::Sender<OrderedObservation>,
    }

    impl WindowSessionSubscriber for MarkerSink {
        fn on_frame(&self, frame: Arc<SessionFrame>) {
            if frame.sequence == FrameSequence(1) {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            }
        }

        fn on_event(&self, event: Arc<SessionEvent>) {
            match event.as_ref() {
                SessionEvent::Action(action) => self
                    .observations
                    .send(OrderedObservation::Action(action.sequence))
                    .unwrap(),
                SessionEvent::FirstFrameAfterActions {
                    frame_sequence,
                    actions,
                    ..
                } => self
                    .observations
                    .send(OrderedObservation::Boundary {
                        frame: *frame_sequence,
                        actions: actions.clone(),
                    })
                    .unwrap(),
                SessionEvent::Lifecycle(_) => {}
            }
        }
    }

    #[test]
    fn action_frame_boundary_survives_lossy_frame_replacement() {
        let hub = WindowSessionHub::new();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (observation_tx, observation_rx) = mpsc::channel();
        hub.subscribe(
            "marker",
            MarkerSink {
                entered: entered_tx,
                release: Mutex::new(release_rx),
                observations: observation_tx,
            },
        );

        hub.publish_frame(target(), geometry(200), 1, FramePayload::Png(Arc::from([])));
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let action = hub.publish_action(ActionDispatch {
            target: Some(target()),
            tool: "click".into(),
            label: "click (4, 5)".into(),
            wall_timestamp_ms: 1,
        });
        hub.publish_frame(target(), geometry(200), 2, FramePayload::Png(Arc::from([])));
        hub.publish_frame(target(), geometry(200), 3, FramePayload::Png(Arc::from([])));
        release_tx.send(()).unwrap();

        assert_eq!(
            observation_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            OrderedObservation::Action(action)
        );
        assert_eq!(
            observation_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            OrderedObservation::Boundary {
                frame: FrameSequence(2),
                actions: vec![action],
            }
        );
    }

    #[test]
    fn live_source_state_is_target_scoped() {
        let hub = WindowSessionHub::new();
        let a = target();
        let b = WindowTarget {
            key: cua_spacesd_provider_api::BackendTargetKey::new("test-target-8"),
            epoch: cua_media_protocol::TargetEpoch(1),
        };
        hub.mark_live_source(a.clone());
        assert!(hub.is_live_source(&a));
        assert!(!hub.is_live_source(&b));
        hub.mark_source_suspended(&a, "switching target");
        assert!(!hub.is_live_source(&a));
    }

    struct LifecycleSink(mpsc::Sender<RuntimeLifecycleEvent>);

    impl WindowSessionSubscriber for LifecycleSink {
        fn on_event(&self, event: Arc<SessionEvent>) {
            if let SessionEvent::Lifecycle(lifecycle) = event.as_ref() {
                self.0.send(lifecycle.clone()).unwrap();
            }
        }
    }

    #[test]
    fn title_change_is_target_scoped_and_lossless() {
        let hub = WindowSessionHub::new();
        let (tx, rx) = mpsc::channel();
        hub.subscribe("lifecycle", LifecycleSink(tx));
        hub.publish_frame(target(), geometry(200), 1, FramePayload::Png(Arc::from([])));
        let _opened = rx.recv_timeout(Duration::from_secs(1)).unwrap();

        hub.mark_title_changed(&target(), "Renamed Window");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            RuntimeLifecycleEvent::TitleChanged {
                target: target(),
                title: "Renamed Window".into(),
            }
        );
    }

    #[test]
    fn frame_records_first_after_action_ordering_once() {
        let hub = WindowSessionHub::new();
        let action = hub.publish_action(ActionDispatch {
            target: Some(target()),
            tool: "click".into(),
            label: "click (4, 5)".into(),
            wall_timestamp_ms: 1,
        });
        let first = hub.publish_frame(target(), geometry(200), 2, FramePayload::Png(Arc::from([])));
        let second =
            hub.publish_frame(target(), geometry(200), 3, FramePayload::Png(Arc::from([])));

        assert_eq!(first.first_after_actions, vec![action]);
        assert!(second.first_after_actions.is_empty());
    }

    #[test]
    fn fallback_is_suppressed_if_source_became_live_during_capture() {
        let hub = WindowSessionHub::new();
        let action = hub.publish_action(ActionDispatch {
            target: Some(target()),
            tool: "click".into(),
            label: "click (4, 5)".into(),
            wall_timestamp_ms: 1,
        });
        assert!(hub.action_needs_fallback(action, Some(&target())));

        let live =
            hub.publish_live_frame(target(), geometry(200), 2, FramePayload::Png(Arc::from([])));
        let fallback = hub.publish_fallback_after_action(
            action,
            PendingFrame {
                target: target(),
                geometry: geometry(160),
                capture_timestamp_us: 3,
                payload: FramePayload::Png(Arc::from([])),
            },
        );

        assert!(fallback.is_none());
        assert_eq!(live.first_after_actions, vec![action]);
        assert!(!hub.action_needs_fallback(action, Some(&target())));
    }

    #[test]
    fn post_dispatch_fallback_crosses_pending_action_boundary() {
        let hub = WindowSessionHub::new();
        let action = hub.publish_action(ActionDispatch {
            target: Some(target()),
            tool: "click".into(),
            label: "click (4, 5)".into(),
            wall_timestamp_ms: 1,
        });
        let fallback = hub
            .publish_fallback_after_action(
                action,
                PendingFrame {
                    target: target(),
                    geometry: geometry(160),
                    capture_timestamp_us: 2,
                    payload: FramePayload::Png(Arc::from([])),
                },
            )
            .expect("pending action should receive its fallback");

        assert_eq!(fallback.first_after_actions, vec![action]);
        assert!(!hub.action_needs_fallback(action, Some(&target())));
    }

    struct FakePollingSource {
        polls: AtomicU64,
    }

    impl PollingWindowSource for FakePollingSource {
        fn poll(&self, _target: &WindowTarget) -> Result<PollingCaptureOutcome, String> {
            let timestamp = self.polls.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(PollingCaptureOutcome::Frame {
                geometry: geometry(200),
                capture_timestamp_us: timestamp,
                payload: FramePayload::Png(Arc::from([1u8].as_slice())),
            })
        }

        fn frame_interval(&self) -> Duration {
            Duration::from_millis(5)
        }
    }

    #[test]
    fn polling_capture_shares_one_worker_until_the_last_owner_releases() {
        let hub = WindowSessionHub::new();
        let (tx, rx) = mpsc::channel();
        hub.subscribe("polling", FrameSink(tx));
        let source = Arc::new(FakePollingSource {
            polls: AtomicU64::new(0),
        });
        let manager = PollingWindowCaptureManager::new(hub, source);

        manager.acquire("owner-a", target()).unwrap();
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        manager.acquire("owner-b", target()).unwrap();
        {
            let ownership = manager
                .inner
                .ownership
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(ownership.targets.len(), 1);
            assert_eq!(ownership.targets.get(&target()).unwrap().owners.len(), 2);
        }

        manager.release("owner-a");
        assert_eq!(
            manager
                .inner
                .ownership
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .targets
                .get(&target())
                .unwrap()
                .owners
                .len(),
            1
        );
        manager.release("owner-b");
        assert!(manager
            .inner
            .ownership
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .targets
            .is_empty());
    }

    struct DropSink(mpsc::Sender<()>);

    impl WindowSessionSubscriber for DropSink {}

    impl Drop for DropSink {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn unsubscribe_removes_mailbox_and_stops_worker() {
        let hub = WindowSessionHub::new();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let id = hub.subscribe("drop", DropSink(dropped_tx));

        assert!(hub.has_subscribers());
        assert!(hub.unsubscribe(id));
        assert!(!hub.has_subscribers());
        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber worker should exit after unsubscribe");
        assert!(!hub.unsubscribe(id));
    }

    #[test]
    fn close_prunes_target_state_and_emits_once() {
        let hub = WindowSessionHub::new();
        let action = hub.publish_action(ActionDispatch {
            target: Some(target()),
            tool: "click".into(),
            label: "click".into(),
            wall_timestamp_ms: 1,
        });

        assert!(hub.is_action_pending(&target(), action));
        assert!(hub.mark_target_closed(&target()));
        assert!(!hub.is_action_pending(&target(), action));
        assert!(!hub.is_live_source(&target()));
        assert!(!hub.mark_target_closed(&target()));
    }
}
