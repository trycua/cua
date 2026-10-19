// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Media plane tests with fake providers (no platform code, host-safe).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{
    AudioCodecName, AudioTrackStateKind, ClientMessage, ErrorCode, FrameAck, MediaTargetRef,
    ServerMessage,
};
use cua_media_protocol::{
    ActionCapability, ActionDeliveryGuarantee, InputKeyState, InteractiveInputBatch,
    InteractiveInputEvent, SessionPolicy, SurfaceGeometry, TargetEpoch, TargetGrant, TargetHandle,
    VideoCodec, WindowDescriptor, WindowGeometryControl, WindowLifecycleEvent, WindowSessionId,
};
use cua_media_transport::audio::decode_audio_packet;
use cua_spacesd_provider_api::*;

use super::audio::*;
use super::leases::InputLeases;
use super::*;

const KEYFRAME: &[u8] = &[
    0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 3,
];
const DELTA: &[u8] = &[0, 0, 0, 1, 0x41, 4];

#[derive(Default)]
struct FakeCapture {
    sinks: Mutex<Vec<Arc<dyn CaptureSink>>>,
    keyframe_requests: AtomicU64,
    bitrate: AtomicU64,
    /// Last `set_paused` value (0 never called, 1 paused, 2 resumed).
    paused: AtomicU64,
}

impl FakeCapture {
    fn emit(&self, keyframe: bool, width: u32, height: u32, epoch: u64) {
        let sink = self.sinks.lock().unwrap().last().cloned();
        if let Some(sink) = sink {
            sink.on_event(CaptureEvent::Frame(OwnedFrame {
                bytes: Arc::from(if keyframe { KEYFRAME } else { DELTA }),
                format: PixelFormat::H264AnnexB,
                width_px: width,
                height_px: height,
                bytes_per_row: None,
                capture_timestamp_us: clock::now_us(),
                encode_duration_us: Some(100),
                codec_epoch: epoch,
                keyframe,
            }));
        }
    }

    fn event(&self, event: CaptureEvent) {
        let sink = self.sinks.lock().unwrap().last().cloned();
        if let Some(sink) = sink {
            sink.on_event(event);
        }
    }
}

struct FakeLease(Arc<FakeCapture>);

impl CaptureLease for FakeLease {
    fn request_keyframe(&self) {
        self.0.keyframe_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn stop(&self) {}

    fn set_target_bitrate_kbps(&self, kbps: u32) {
        self.0.bitrate.store(u64::from(kbps), Ordering::SeqCst);
    }

    fn set_paused(&self, paused: bool) {
        self.0
            .paused
            .store(if paused { 1 } else { 2 }, Ordering::SeqCst);
    }

    fn encoder_name(&self) -> Option<String> {
        Some("fake".into())
    }
}

struct CaptureProviderImpl(Arc<FakeCapture>);

impl CaptureProvider for CaptureProviderImpl {
    fn formats(&self, _: &ProviderTargetId) -> Result<Vec<PixelFormat>, ProviderError> {
        Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8])
    }

    fn start(
        &self,
        _: &ProviderTargetId,
        _: &CaptureConfig,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Arc<dyn CaptureLease>, ProviderError> {
        self.0.sinks.lock().unwrap().push(sink);
        Ok(Arc::new(FakeLease(self.0.clone())))
    }
}

fn window_target() -> ProviderTarget {
    ProviderTarget {
        id: ProviderTargetId {
            key: BackendTargetKey::new("fake:window"),
            epoch: TargetEpoch(1),
        },
        descriptor: WindowDescriptor {
            window: TargetHandle("w1".into()),
            target_epoch: TargetEpoch(1),
            app_name: "Fixture".into(),
            title: "Fixture".into(),
            geometry: SurfaceGeometry {
                width_px: 320,
                height_px: 200,
                scale_factor: 1.0,
            },
            visible: true,
        },
        grant: Some(TargetGrant("g".into())),
    }
}

struct Targets;

impl TargetProvider for Targets {
    fn enumerate(&self, _: &TargetQuery) -> Result<Vec<ProviderTarget>, ProviderError> {
        Ok(vec![window_target()])
    }

    fn pick(&self, _: &PickTargetRequest) -> Result<ProviderTarget, ProviderError> {
        Ok(window_target())
    }

    fn restore(&self, _: &TargetGrant) -> Result<ProviderTarget, ProviderError> {
        Ok(window_target())
    }

    fn resolve(
        &self,
        handle: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<ProviderTarget, ProviderError> {
        if handle.0 == "w1" && epoch == TargetEpoch(1) {
            Ok(window_target())
        } else {
            Err(ProviderError::new(ProviderErrorCode::StaleTarget, "stale"))
        }
    }
}

struct Displays;

impl DisplayProvider for Displays {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError> {
        Ok(vec![ProviderDisplay {
            id: "0".into(),
            name: "fake".into(),
            primary: true,
            bounds: (0.0, 0.0, 640.0, 480.0),
            native_width_px: 640,
            native_height_px: 480,
            scale_factor: 1.0,
            refresh_rate_hz: 0,
        }])
    }

    fn display_target(&self, display_id: &str) -> Result<ProviderTarget, ProviderError> {
        let mut target = window_target();
        target.id.key = BackendTargetKey::new(format!("display:{display_id}"));
        target.descriptor.window = TargetHandle(format!("display-{display_id}"));
        target.descriptor.geometry = SurfaceGeometry {
            width_px: 640,
            height_px: 480,
            scale_factor: 1.0,
        };
        Ok(target)
    }
}

struct Actions;

impl ActionProvider for Actions {
    fn capabilities(&self, _: &ProviderTargetId) -> Result<Vec<ActionCapability>, ProviderError> {
        Ok(vec![ActionCapability {
            action: "click".into(),
            guarantee: ActionDeliveryGuarantee::Background,
        }])
    }

    fn perform<'a>(
        &'a self,
        _: &'a ProviderTargetId,
        _: ActionInvocation,
        _: SessionPolicy,
    ) -> ProviderFuture<'a, Result<ActionOutcome, ProviderError>> {
        Box::pin(async {
            Ok(ActionOutcome {
                delivered: true,
                detail: None,
            })
        })
    }
}

struct Accessibility;

impl AccessibilityProvider for Accessibility {
    fn snapshot<'a>(
        &'a self,
        _: &'a ProviderTargetId,
    ) -> ProviderFuture<'a, Result<AccessibilitySnapshot, ProviderError>> {
        Box::pin(async {
            Ok(AccessibilitySnapshot {
                snapshot_id: cua_media_protocol::AccessibilitySnapshotId(1),
                state: cua_media_protocol::Value::Null,
            })
        })
    }
}

struct Geometry;

impl WindowGeometryProvider for Geometry {
    fn supports(&self, _: &ProviderTargetId) -> Result<bool, ProviderError> {
        Ok(true)
    }

    fn resize<'a>(
        &'a self,
        _: &'a ProviderTargetId,
        width_points: u32,
        height_points: u32,
    ) -> ProviderFuture<'a, Result<AppliedWindowGeometry, ProviderError>> {
        Box::pin(async move {
            Ok(AppliedWindowGeometry {
                width_points,
                height_points,
            })
        })
    }
}

#[derive(Default)]
struct RecordingInput {
    batches: Mutex<Vec<InteractiveInputBatch>>,
    released: AtomicU64,
    /// How long each dispatch takes (a tool-backed Hyprland lease on an
    /// emulated guest takes seconds).
    dispatch_ms: AtomicU64,
}

struct InputLeaseImpl(Arc<RecordingInput>);

impl InteractiveInputLease for InputLeaseImpl {
    fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputOutcome, ProviderError> {
        std::thread::sleep(Duration::from_millis(
            self.0.dispatch_ms.load(Ordering::SeqCst),
        ));
        self.0.batches.lock().unwrap().push(batch.clone());
        Ok(InteractiveInputOutcome {
            through_sequence: batch.first_sequence + batch.events.len() as u64 - 1,
            event_count: batch.events.len(),
            dispatch_micros: 5,
        })
    }

    fn release_all(&self) {
        self.0.released.fetch_add(1, Ordering::SeqCst);
    }
}

struct Inputs(Arc<RecordingInput>);

impl InteractiveInputProvider for Inputs {
    fn open(
        &self,
        _: &ProviderTargetId,
        policy: SessionPolicy,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
        if policy == SessionPolicy::ViewOnly {
            return Ok(None);
        }
        Ok(Some(Arc::new(InputLeaseImpl(self.0.clone()))))
    }
}

#[derive(Default)]
struct FakeAudio {
    sinks: Mutex<Vec<Arc<dyn AudioFrameSink>>>,
}

struct FakeAudioLease;

impl AudioTrackLease for FakeAudioLease {
    fn stop(&self) {}
}

struct AudioProviderImpl(Arc<FakeAudio>);

impl AudioProvider for AudioProviderImpl {
    fn sources(&self) -> Vec<AudioSourceInfo> {
        vec![AudioSourceInfo {
            source_id: "desktop".into(),
            kind: AudioSourceKind::Desktop,
            name: "Desktop audio".into(),
            pid: None,
            available: true,
            limitation: None,
            desktop_fallback: false,
        }]
    }

    fn codecs(&self) -> Vec<AudioCodecName> {
        vec![AudioCodecName::PcmS16le]
    }

    fn start(
        &self,
        _: &AudioTrackConfig,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Arc<dyn AudioTrackLease>, ProviderError> {
        self.0.sinks.lock().unwrap().push(sink);
        Ok(Arc::new(FakeAudioLease))
    }
}

struct Fixture {
    runtime: Arc<MediaRuntime>,
    capture: Arc<FakeCapture>,
    input: Arc<RecordingInput>,
    audio: Arc<FakeAudio>,
}

fn fixture() -> Fixture {
    let capture = Arc::new(FakeCapture::default());
    let input = Arc::new(RecordingInput::default());
    let audio = Arc::new(FakeAudio::default());
    let runtime = MediaRuntime::new(
        MediaProviders {
            targets: Arc::new(Targets),
            displays: Arc::new(Displays),
            captures: Arc::new(CaptureProviderImpl(capture.clone())),
            actions: Arc::new(Actions),
            accessibility: Arc::new(Accessibility),
            geometry: Arc::new(Geometry),
            inputs: Arc::new(Inputs(input.clone())),
            audio: Some(Arc::new(AudioProviderImpl(audio.clone()))),
        },
        Arc::new(InputLeases::default()),
    );
    Fixture {
        runtime,
        capture,
        input,
        audio,
    }
}

fn params(target: MediaTargetSpec) -> OpenMediaParams {
    OpenMediaParams {
        target,
        codecs: vec![VideoCodec::H264],
        max_fps: 30,
        max_dimension: 0,
        bitrate_kbps: None,
        policy: SessionPolicy::BackgroundOnly,
        geometry_control: WindowGeometryControl::ObserveOnly,
        principal: MediaPrincipal {
            id: "alice".into(),
            name: "Alice".into(),
            color: "#ff0000".into(),
            agent: false,
        },
        disable_video: false,
        audio: None,
        quic: false,
        attach_deadline: Instant::now() + Duration::from_secs(60),
    }
}

fn window() -> MediaTargetSpec {
    MediaTargetSpec::Window {
        handle: TargetHandle("w1".into()),
        epoch: TargetEpoch(1),
    }
}

fn display() -> MediaTargetSpec {
    MediaTargetSpec::Display {
        display_id: "primary".into(),
    }
}

/// Open on a blocking thread (open waits for the first frame) while the
/// fake produces one.
async fn open(fixture: &Fixture, params: OpenMediaParams) -> OpenedMedia {
    let runtime = fixture.runtime.clone();
    let capture = fixture.capture.clone();
    let opening = tokio::task::spawn_blocking(move || runtime.open(params));
    tokio::time::sleep(Duration::from_millis(50)).await;
    capture.emit(true, 640, 480, 1);
    opening.await.unwrap().unwrap()
}

/// Hard bounds for draining a viewer: a runaway producer fails the test
/// instead of growing memory.
const MAX_DRAIN_ITEMS: usize = 10_000;
const MAX_DRAIN_BYTES: usize = 64 * 1024 * 1024;

fn drain(viewer: &Viewer) -> Vec<Outbound> {
    let mut items = Vec::new();
    let mut bytes = 0usize;
    for _ in 0..MAX_DRAIN_ITEMS {
        let Some(item) = viewer.try_next_outbound() else {
            return items;
        };
        bytes += match &item {
            Outbound::Video(packet) => packet.payload.len(),
            Outbound::Audio(packet) => packet.len(),
            _ => 64,
        };
        assert!(
            bytes <= MAX_DRAIN_BYTES,
            "drain exceeded {MAX_DRAIN_BYTES} bytes"
        );
        items.push(item);
    }
    panic!("drain exceeded {MAX_DRAIN_ITEMS} items: the viewer never ran dry");
}

fn videos(items: &[Outbound]) -> Vec<Arc<VideoPacket>> {
    items
        .iter()
        .filter_map(|item| match item {
            Outbound::Video(packet) => Some(packet.clone()),
            _ => None,
        })
        .collect()
}

/// At least `count` control replies, waiting (bounded) for the input
/// worker to deliver them.
async fn controls_eventually(viewer: &Viewer, count: usize) -> Vec<ServerMessage> {
    let mut replies = Vec::new();
    for _ in 0..200 {
        replies.extend(controls(&drain(viewer)));
        if replies.len() >= count {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    replies
}

fn controls(items: &[Outbound]) -> Vec<ServerMessage> {
    items
        .iter()
        .filter_map(|item| match item {
            Outbound::Control(message) => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn display_session_greets_server_first_and_starts_on_a_keyframe() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    assert_eq!(
        opened.opened.target,
        MediaTargetRef::Display {
            display_id: "0".into()
        }
    );
    assert_eq!(opened.opened.geometry.width_px, 640);
    fixture.capture.emit(false, 640, 480, 1);
    let viewer = opened.session.attach().unwrap();
    let items = drain(&viewer);
    let control = controls(&items);
    assert!(matches!(control[0], ServerMessage::Hello(ref hello) if hello.selected_version == 2));
    assert!(
        matches!(control[1], ServerMessage::SessionOpened(ref opened) if opened.session_id.0 == opened.session_id.0)
    );
    // Control first, then video; the first video packet is a keyframe.
    let first_video = items
        .iter()
        .position(|item| matches!(item, Outbound::Video(_)))
        .unwrap();
    assert!(items[..first_video]
        .iter()
        .all(|item| matches!(item, Outbound::Control(_))));
    let video = videos(&items);
    assert!(video[0].keyframe);
    assert_eq!(video.len(), 2, "short GOP is replayed on attach");
    assert!(video[1].sequence() > video[0].sequence());
    // Sequences start at an arbitrary (non-zero, not one) value.
    assert!(video[0].sequence() > 1);
}

#[tokio::test]
async fn capture_pauses_with_no_socket_and_resumes_on_a_fresh_idr() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    fixture.capture.emit(true, 640, 480, 1);
    let viewer = opened.session.attach().unwrap();
    assert_eq!(
        fixture.capture.paused.load(Ordering::SeqCst),
        0,
        "a first attach does not resume"
    );
    drop(viewer);
    assert_eq!(
        fixture.capture.paused.load(Ordering::SeqCst),
        1,
        "the last socket left: paused"
    );
    let before = fixture.capture.keyframe_requests.load(Ordering::SeqCst);
    let again = opened.session.attach().unwrap();
    assert_eq!(
        fixture.capture.paused.load(Ordering::SeqCst),
        2,
        "resumed on attach"
    );
    assert!(
        fixture.capture.keyframe_requests.load(Ordering::SeqCst) > before,
        "a stale GOP is not replayed after a pause; a fresh IDR is asked for"
    );
    assert!(
        videos(&drain(&again)).is_empty(),
        "nothing stale before the new keyframe"
    );
}

#[tokio::test]
async fn idle_session_refreshes_with_a_periodic_idr_and_active_one_does_not() {
    let fixture = fixture();
    fixture
        .runtime
        .set_idle_keyframe_interval(Some(Duration::from_millis(400)));
    let opened = open(&fixture, params(window())).await;
    let _viewer = opened.session.attach().unwrap();
    // Active: frames every 100 ms, no idle refresh.
    let base = fixture.capture.keyframe_requests.load(Ordering::SeqCst);
    for _ in 0..8 {
        fixture.capture.emit(false, 640, 480, 1);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(opened.session.video.lock().unwrap().idle_keyframes, 0);
    // Idle: one refresh per interval, while nothing is sent.
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    let refreshes = opened.session.video.lock().unwrap().idle_keyframes;
    assert!(
        (2..=3).contains(&refreshes),
        "{refreshes} refreshes in ~1.05 s at 400 ms"
    );
    assert!(fixture.capture.keyframe_requests.load(Ordering::SeqCst) >= base + refreshes);
}

#[tokio::test]
async fn idle_refresh_can_be_disabled() {
    let fixture = fixture();
    fixture.runtime.set_idle_keyframe_interval(None);
    let opened = open(&fixture, params(window())).await;
    let _viewer = opened.session.attach().unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(opened.session.video.lock().unwrap().idle_keyframes, 0);
}

#[tokio::test]
async fn late_viewer_without_cached_gop_gets_a_forced_idr_and_no_deltas_before_it() {
    let fixture = fixture();
    let opened = open(&fixture, params(window())).await;
    for _ in 0..200 {
        fixture.capture.emit(false, 640, 480, 1);
    }
    let before = fixture.capture.keyframe_requests.load(Ordering::SeqCst);
    let viewer = opened.session.attach().unwrap();
    assert!(fixture.capture.keyframe_requests.load(Ordering::SeqCst) > before);
    fixture.capture.emit(false, 640, 480, 1);
    assert!(
        videos(&drain(&viewer)).is_empty(),
        "deltas before the IDR are discarded"
    );
    fixture.capture.emit(true, 640, 480, 1);
    let video = videos(&drain(&viewer));
    assert_eq!(video.len(), 1);
    assert!(video[0].keyframe);
}

#[tokio::test]
async fn slow_viewer_drops_to_next_keyframe_without_affecting_a_fast_viewer() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let slow = opened.session.attach().unwrap();
    let fast = opened.session.attach().unwrap();
    drain(&slow);
    drain(&fast);
    // A 4 MiB budget: push big deltas without draining the slow viewer.
    let big = Arc::<[u8]>::from({
        let mut bytes = DELTA.to_vec();
        bytes.resize(512 * 1024, 7);
        bytes
    });
    let sink = fixture
        .capture
        .sinks
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    let mut fast_received = 0;
    for _ in 0..12 {
        sink.on_event(CaptureEvent::Frame(OwnedFrame {
            bytes: big.clone(),
            format: PixelFormat::H264AnnexB,
            width_px: 640,
            height_px: 480,
            bytes_per_row: None,
            capture_timestamp_us: clock::now_us(),
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: false,
        }));
        fast_received += videos(&drain(&fast)).len();
    }
    assert_eq!(fast_received, 12);
    let stats = slow.stats();
    assert!(stats.frames_dropped > 0);
    assert!(videos(&drain(&slow)).len() < 12);
    fixture.capture.emit(false, 640, 480, 1);
    assert!(
        videos(&drain(&slow)).is_empty(),
        "slow viewer waits for a keyframe"
    );
    fixture.capture.emit(true, 640, 480, 1);
    let recovered = videos(&drain(&slow));
    assert!(recovered[0].keyframe);
}

#[tokio::test]
async fn resize_advances_the_geometry_epoch_before_the_new_frame() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let viewer = opened.session.attach().unwrap();
    drain(&viewer);
    let epoch = opened.opened.geometry_epoch.0;
    fixture.capture.emit(true, 800, 600, 2);
    let items = drain(&viewer);
    let lifecycle = items
        .iter()
        .position(|item| {
            matches!(
                item,
                Outbound::Control(ServerMessage::Lifecycle {
                    event: WindowLifecycleEvent::GeometryChanged { .. },
                    ..
                })
            )
        })
        .unwrap();
    let frame = items
        .iter()
        .position(|item| matches!(item, Outbound::Video(_)))
        .unwrap();
    assert!(lifecycle < frame);
    let video = videos(&items);
    assert_eq!(video[0].descriptor.geometry_epoch.0, epoch + 1);
    assert!(video[0].keyframe);
    assert!(video[0].descriptor.codec_epoch.0 > opened.opened.codec_epoch.0);
    assert_eq!(opened.session.session_opened().geometry.width_px, 800);
}

fn key_batch(first_sequence: u64) -> ClientMessage {
    ClientMessage::InteractiveInput(InteractiveInputBatch {
        session_id: WindowSessionId(String::new()),
        first_sequence,
        events: vec![
            InteractiveInputEvent::Key {
                key: "a".into(),
                state: InputKeyState::Down,
                modifiers: vec![],
                repeat: false,
            },
            InteractiveInputEvent::Key {
                key: "a".into(),
                state: InputKeyState::Up,
                modifiers: vec![],
                repeat: false,
            },
        ],
    })
}

#[tokio::test]
async fn input_adopts_the_first_sequence_and_reports_gaps_with_the_expected_value() {
    let fixture = fixture();
    let opened = open(&fixture, params(window())).await;
    let mut viewer = opened.session.attach().unwrap();
    drain(&viewer);
    // Acknowledgements come from the input worker; wait for each so the
    // order below is the order of the batches (another test's delivery can
    // hold the process-wide input lock past the reply budget).
    let mut control = Vec::new();
    for first in [1000, 1002] {
        viewer.handle(key_batch(first)).await;
        control.extend(controls_eventually(&viewer, 1).await);
    }
    viewer.handle(key_batch(1010)).await;
    control.extend(controls(&drain(&viewer)));
    assert!(matches!(
        control[0],
        ServerMessage::InteractiveInputAcknowledgement(ref ack) if ack.delivered && ack.through_sequence == 1001
    ));
    assert!(matches!(
        control[1],
        ServerMessage::InteractiveInputAcknowledgement(ref ack) if ack.delivered && ack.through_sequence == 1003
    ));
    assert!(matches!(
        control[2],
        ServerMessage::Error {
            code: ErrorCode::InputSequenceGap,
            expected_sequence: Some(1004),
            ..
        }
    ));
    assert_eq!(fixture.input.batches.lock().unwrap().len(), 2);
    // A second socket adopts its own base.
    let mut other = opened.session.attach().unwrap();
    drain(&other);
    other.handle(key_batch(7)).await;
    assert!(matches!(
        controls_eventually(&other, 1).await[0],
        ServerMessage::InteractiveInputAcknowledgement(ref ack) if ack.delivered
    ));
}

/// A slow native delivery must not hold the socket: its frames, pings and
/// other replies keep flowing, and batches are still delivered and
/// acknowledged in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_input_delivery_does_not_block_the_socket_and_stays_ordered() {
    let fixture = fixture();
    fixture.input.dispatch_ms.store(1500, Ordering::SeqCst);
    let opened = open(&fixture, params(window())).await;
    let mut viewer = opened.session.attach().unwrap();
    drain(&viewer);
    let started = Instant::now();
    viewer.handle(key_batch(1)).await;
    viewer.handle(key_batch(3)).await;
    viewer
        .handle(ClientMessage::RequestKeyframe {
            session_id: WindowSessionId(String::new()),
        })
        .await;
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "handling waited for the delivery: {:?}",
        started.elapsed()
    );
    let mut replies = controls(&drain(&viewer));
    assert!(matches!(
        replies.first(),
        Some(ServerMessage::KeyframeRequested { .. })
    ));
    for _ in 0..100 {
        if replies
            .iter()
            .filter(|reply| matches!(reply, ServerMessage::InteractiveInputAcknowledgement(_)))
            .count()
            == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        replies.extend(controls(&drain(&viewer)));
    }
    let acks: Vec<u64> = replies
        .iter()
        .filter_map(|reply| match reply {
            ServerMessage::InteractiveInputAcknowledgement(ack) if ack.delivered => {
                Some(ack.through_sequence)
            }
            _ => None,
        })
        .collect();
    assert_eq!(acks, vec![2, 4]);
    let delivered: Vec<u64> = fixture
        .input
        .batches
        .lock()
        .unwrap()
        .iter()
        .map(|batch| batch.first_sequence)
        .collect();
    assert_eq!(delivered, vec![1, 3]);
}

#[tokio::test]
async fn a_second_principal_is_refused_while_the_window_lease_is_held() {
    let fixture = fixture();
    let alice = open(&fixture, params(window())).await;
    let mut bob_params = params(window());
    bob_params.principal.id = "bob".into();
    bob_params.principal.name = "Bob".into();
    let bob = open(&fixture, bob_params).await;
    let mut alice_viewer = alice.session.attach().unwrap();
    let mut bob_viewer = bob.session.attach().unwrap();
    drain(&alice_viewer);
    drain(&bob_viewer);
    alice_viewer.handle(key_batch(1)).await;
    bob_viewer.handle(key_batch(1)).await;
    let ack = controls(&drain(&bob_viewer));
    match &ack[0] {
        ServerMessage::InteractiveInputAcknowledgement(ack) => {
            assert!(!ack.delivered);
            let error = ack.error.as_ref().expect("undelivered acks carry an error");
            assert!(error.message.contains("Alice"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn view_only_sessions_refuse_input_with_an_error() {
    let fixture = fixture();
    let mut view_only = params(window());
    view_only.policy = SessionPolicy::ViewOnly;
    let opened = open(&fixture, view_only).await;
    let mut viewer = opened.session.attach().unwrap();
    drain(&viewer);
    viewer.handle(key_batch(1)).await;
    match &controls(&drain(&viewer))[0] {
        ServerMessage::InteractiveInputAcknowledgement(ack) => {
            assert!(!ack.delivered && ack.error.is_some())
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn v1_messages_get_unsupported_operation() {
    let fixture = fixture();
    let opened = open(&fixture, params(window())).await;
    let mut viewer = opened.session.attach().unwrap();
    drain(&viewer);
    viewer.handle(ClientMessage::Unsupported).await;
    viewer.handle(ClientMessage::Ping { nonce: 9 }).await;
    let control = controls(&drain(&viewer));
    assert!(matches!(
        control[0],
        ServerMessage::Error {
            code: ErrorCode::UnsupportedOperation,
            ..
        }
    ));
    assert!(matches!(control[1], ServerMessage::Pong { nonce: 9 }));
}

#[tokio::test]
async fn socket_limit_and_close_codes() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let viewers: Vec<Viewer> = (0..8).map(|_| opened.session.attach().unwrap()).collect();
    assert_eq!(opened.session.attach().unwrap_err(), 4429);
    let id = opened.session.id().to_owned();
    assert!(fixture.runtime.close(&id, 4404));
    for viewer in &viewers {
        assert!(drain(viewer)
            .iter()
            .any(|item| matches!(item, Outbound::Close(4404))));
    }
    assert_eq!(opened.session.attach().unwrap_err(), 4404);
    assert!(fixture.runtime.session(&id).is_none());
}

#[tokio::test]
async fn target_closed_sends_lifecycle_then_4410() {
    let fixture = fixture();
    let opened = open(&fixture, params(window())).await;
    let viewer = opened.session.attach().unwrap();
    drain(&viewer);
    fixture
        .capture
        .event(CaptureEvent::TitleChanged("renamed".into()));
    fixture.capture.event(CaptureEvent::Closed);
    let items = drain(&viewer);
    let control = controls(&items);
    assert!(control.iter().any(|message| matches!(
        message,
        ServerMessage::Lifecycle {
            event: WindowLifecycleEvent::TitleChanged { .. },
            ..
        }
    )));
    assert!(control.iter().any(|message| matches!(
        message,
        ServerMessage::Lifecycle {
            event: WindowLifecycleEvent::Closed,
            ..
        }
    )));
    assert!(matches!(items.last(), Some(Outbound::Close(4410))));
}

#[tokio::test]
async fn acking_clients_are_held_to_an_ack_window() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let mut viewer = opened.session.attach().unwrap();
    let first = videos(&drain(&viewer));
    viewer
        .handle(ClientMessage::FrameAck(FrameAck {
            session_id: WindowSessionId(String::new()),
            sequence: first[0].sequence(),
            decode_us: Some(100),
            decode_queue: None,
        }))
        .await;
    let mut sent = 0;
    for _ in 0..200 {
        fixture.capture.emit(false, 640, 480, 1);
        sent += videos(&drain(&viewer)).len();
    }
    assert!((8..200).contains(&sent), "sent {sent}");
    assert!(viewer.stats().frames_dropped > 0);
}

#[tokio::test]
async fn audio_tracks_follow_session_opened_and_take_priority_over_video() {
    let fixture = fixture();
    let mut audio_params = params(display());
    audio_params.audio = Some(AudioRequest {
        downlink: true,
        source_ids: vec![],
        config: AudioTrackConfig {
            source_id: String::new(),
            codec: AudioCodecName::PcmS16le,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_ms: 20,
            bitrate_kbps: 0,
            fec: false,
            dtx: false,
            expected_loss_percent: 0,
        },
        uplink: None,
    });
    let opened = open(&fixture, audio_params).await;
    let audio = opened.audio.as_ref().unwrap();
    assert_eq!(audio.tracks.len(), 1);
    assert_eq!(audio.tracks[0].track_id, 1);
    let mut viewer = opened.session.attach().unwrap();
    let greeting = controls(&drain(&viewer));
    assert!(
        matches!(greeting[2], ServerMessage::AudioConfig(ref config) if config.track_id == 1 && config.sample_rate_hz == 48_000)
    );
    let sink = fixture.audio.sinks.lock().unwrap()[0].clone();
    fixture.capture.emit(true, 640, 480, 1);
    let mut pts = clock::now_us();
    for _ in 0..3 {
        sink.on_frame(AudioFrame {
            pts_us: pts,
            frame_samples: 960,
            payload: vec![0; 960 * 4],
            dtx: false,
        });
        pts += 20_000;
    }
    let items = drain(&viewer);
    let first_audio = items
        .iter()
        .position(|item| matches!(item, Outbound::Audio(_)))
        .unwrap();
    let first_video = items
        .iter()
        .position(|item| matches!(item, Outbound::Video(_)))
        .unwrap();
    assert!(
        first_audio < first_video,
        "audio goes out before queued video"
    );
    let packets: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            Outbound::Audio(bytes) => Some(decode_audio_packet(bytes).unwrap().0),
            _ => None,
        })
        .collect();
    assert_eq!(packets.len(), 3);
    assert_eq!(packets[1].sequence, packets[0].sequence.wrapping_add(1));
    assert_eq!(packets[1].pts_us - packets[0].pts_us, 20_000);
    // Overflow past 200 ms drops the oldest and flags a discontinuity.
    for _ in 0..15 {
        sink.on_frame(AudioFrame {
            pts_us: pts,
            frame_samples: 960,
            payload: vec![0; 16],
            dtx: false,
        });
        pts += 20_000;
    }
    let overflowed: Vec<_> = drain(&viewer)
        .iter()
        .filter_map(|item| match item {
            Outbound::Audio(bytes) => Some(decode_audio_packet(bytes).unwrap().0),
            _ => None,
        })
        .collect();
    assert_eq!(overflowed.len(), 10);
    assert!(overflowed[0].discontinuity);
    assert!(!overflowed[1].discontinuity);
    // Pause via preferences.
    opened.session.set_audio_preferences(AudioPreferenceUpdate {
        enabled: Some(false),
        ..AudioPreferenceUpdate::default()
    });
    assert!(controls(&drain(&viewer)).iter().any(|message| matches!(message, ServerMessage::AudioTrackState(state) if state.state == AudioTrackStateKind::Paused)));
    // Uplink packets without a grant are refused once.
    viewer.handle_binary(&cua_media_transport::audio::encode_audio_packet(
        &cua_media_transport::audio::AudioPacketHeader {
            discontinuity: false,
            dtx: false,
            track_id: 9,
            sequence: 1,
            pts_us: 0,
            frame_samples: 960,
            config_epoch: 1,
        },
        &[],
    ));
    viewer.handle_binary(b"RAU2");
    let denied: Vec<_> = controls(&drain(&viewer))
        .into_iter()
        .filter(|message| {
            matches!(
                message,
                ServerMessage::Error {
                    code: ErrorCode::AudioUplinkDenied,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(denied.len(), 1);
}

#[tokio::test]
async fn stats_report_health_fields() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let mut viewer = opened.session.attach().unwrap();
    drain(&viewer);
    viewer
        .handle(ClientMessage::GetStats {
            session_id: WindowSessionId(opened.session.id().into()),
        })
        .await;
    let control = controls(&drain(&viewer));
    match &control[0] {
        ServerMessage::Stats(stats) => {
            assert_eq!(stats.frames_since_attach, 1);
            assert!(stats.last_frame_age_ms.is_some());
            assert_eq!(stats.encoder.as_deref(), Some("fake"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn bidirectional_geometry_is_exclusive_per_window() {
    let fixture = fixture();
    let mut first = params(window());
    first.geometry_control = WindowGeometryControl::Bidirectional;
    let opened = open(&fixture, first.clone()).await;
    let second = fixture.runtime.open(first.clone());
    assert!(matches!(second, Err(MediaError::FailedPrecondition(_))));
    fixture.runtime.close(opened.session.id(), 4404);
    let mut display_bidirectional = params(display());
    display_bidirectional.geometry_control = WindowGeometryControl::Bidirectional;
    assert!(matches!(
        fixture.runtime.open(display_bidirectional),
        Err(MediaError::InvalidArgument(_))
    ));
}

#[tokio::test]
async fn close_is_terminal_and_emitted_exactly_once() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let viewer = opened.session.attach().unwrap();
    drain(&viewer);
    fixture.runtime.close(opened.session.id(), 4404);
    // Producers after close are ignored.
    fixture.capture.emit(true, 640, 480, 1);
    let mut closes = 0;
    let mut polls = 0;
    while let Some(item) = viewer.try_next_outbound() {
        polls += 1;
        assert!(polls < 100, "viewer kept yielding after close");
        if matches!(item, Outbound::Close(_)) {
            closes += 1;
        }
    }
    assert_eq!(closes, 1);
    assert!(viewer.try_next_outbound().is_none());
    assert!(
        drain(&viewer).is_empty(),
        "a drain after close terminates empty"
    );
}

#[tokio::test]
async fn control_overflow_closes_once_instead_of_growing() {
    let fixture = fixture();
    let opened = open(&fixture, params(display())).await;
    let mut viewer = opened.session.attach().unwrap();
    for nonce in 0..1_000 {
        viewer.handle(ClientMessage::Ping { nonce }).await;
    }
    let items = drain(&viewer);
    assert!(
        items.len() <= 300,
        "control queue is bounded: {}",
        items.len()
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| matches!(item, Outbound::Close(4500)))
            .count(),
        1
    );
    assert!(viewer.try_next_outbound().is_none());
}
