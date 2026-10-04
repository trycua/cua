// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One attached media socket: its queues, flow control and control messages.
//!
//! Send order on every socket: control messages, then audio, then video.
//!
//! Video flow control (H.264):
//! - Each viewer has its own bounded queue (about two seconds of stream at the
//!   current bitrate, at least 4 MiB). A keyframe supersedes everything queued
//!   before it. When the queue overflows, or its oldest frame is older than a
//!   second, the viewer drops every queued frame and waits for the next
//!   keyframe; the session asks the encoder for one (rate limited).
//! - Clients that send `frame_ack` get an ack window: at most about two
//!   seconds of frames (at the measured frame rate) may be unacknowledged.
//!   Past that the viewer stops sending dependent frames and resumes on a
//!   keyframe. A client that stops acking for 4 s is stalled and is probed
//!   with one keyframe every 2 s.
//! - Packed/PNG frames are independent: the queue keeps only the newest one.
//!
//! Audio keeps at most 200 ms per track; the oldest packet is dropped past
//! that and the next packet sent carries the `discontinuity` flag.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{self, ClientMessage, ErrorCode, FrameAck, ServerMessage, Stats};
use cua_media_protocol::{
    AccessibilitySnapshotId, ActionBasis, ActionError, ActionErrorCode, ActionFrameCorrelation,
    ActionResult, FrameSequence, InteractiveInputAcknowledgement, InteractiveInputBatch,
    StreamPreferences, StreamStats, VideoCodec, WindowGeometryControl, WindowGeometryRequest,
    WindowGeometryResult, WindowSessionId, WindowState,
};
use cua_media_transport::audio::{
    decode_audio_packet, sequence_after, AUDIO_FLAG_DISCONTINUITY, AUDIO_PACKET_HEADER_BYTES,
};
use cua_spacesd_provider_api::{ActionInvocation, ProviderError, ProviderErrorCode};
use tokio::sync::Notify;

use super::audio::AudioFrame;
use super::leases::{LeaseConflict, LeaseGrant};
use super::session::{MediaSession, VideoPacket};

const MAX_CONTROL_MESSAGES: usize = 256;
const MAX_PENDING_ACTIONS: usize = 64;
const MIN_QUEUE_BUDGET_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUEUE_DELAY: Duration = Duration::from_secs(1);
const ACK_WINDOW_SECONDS: f64 = 2.0;
const MIN_ACK_WINDOW_FRAMES: usize = 8;
const STALL_AFTER: Duration = Duration::from_secs(4);
const STALL_PROBE_EVERY: Duration = Duration::from_secs(2);
const ACTION_FRAME_WAIT: Duration = Duration::from_millis(250);
const MAX_QUEUED_AUDIO_US: u64 = v2::MAX_QUEUED_AUDIO_MS * 1_000;

/// One item to write to the socket.
#[derive(Debug, Clone)]
pub enum Outbound {
    Control(ServerMessage),
    /// A complete binary audio packet (`RAU2`).
    Audio(Arc<[u8]>),
    Video(Arc<VideoPacket>),
    /// Close the socket with this code.
    Close(u16),
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CongestionSignal {
    pub(crate) queue_delay_ms: f64,
    pub(crate) ack_excess_ms: f64,
    pub(crate) dropped_frames: u64,
}

struct QueuedAudio {
    track_id: u16,
    packet: Arc<[u8]>,
    duration_us: u64,
}

struct PendingAction {
    action_id: String,
    boundary: u64,
    first_frame: Option<u64>,
    completed: bool,
    deadline: Option<Instant>,
}

#[derive(Default)]
struct AckState {
    enabled: bool,
    last_acked: Option<u64>,
    last_ack_at: Option<Instant>,
    in_flight: VecDeque<(u64, Instant)>,
    min_rtt_ms: Option<f64>,
    rtt_ms: Option<f64>,
    excess_ms: f64,
    stalled_since: Option<Instant>,
    last_probe: Option<Instant>,
    measured_fps: f64,
    acks_in_window: u32,
    window_started: Option<Instant>,
}

struct Queues {
    control: VecDeque<ServerMessage>,
    control_overflow: bool,
    audio: VecDeque<QueuedAudio>,
    audio_discontinuity: HashMap<u16, bool>,
    video: VecDeque<Arc<VideoPacket>>,
    video_bytes: usize,
    awaiting_keyframe: bool,
    close: Option<u16>,
    /// Set once `Close` has been handed out. A terminated viewer yields
    /// nothing more, so a polling loop always ends.
    terminated: bool,
    bitrate_kbps: u32,
    frames_since_attach: u64,
    keyframes_sent: u64,
    frames_dropped: u64,
    dropped_since_sample: u64,
    bytes_sent: u64,
    last_sent_sequence: Option<u64>,
    ack: AckState,
    pending_actions: Vec<PendingAction>,
    action_frame_timeouts: u64,
    actions_dispatched: u64,
}

impl Queues {
    /// Queue a control message; past the cap the socket is closed (4500)
    /// instead of growing memory.
    fn push_bounded(&mut self, message: ServerMessage) {
        if self.close.is_some() || self.terminated {
            return;
        }
        if self.control.len() >= MAX_CONTROL_MESSAGES {
            self.control_overflow = true;
        } else {
            self.control.push_back(message);
        }
    }
}

/// State shared between the session (producer) and the socket (consumer).
pub struct ViewerShared {
    pub(crate) id: u64,
    session_id: WindowSessionId,
    codec: VideoCodec,
    queues: Mutex<Queues>,
    notify: Notify,
}

impl ViewerShared {
    pub(crate) fn new(id: u64, session_id: &str, codec: VideoCodec, bitrate_kbps: u32) -> Self {
        Self {
            id,
            session_id: WindowSessionId(session_id.to_owned()),
            codec,
            queues: Mutex::new(Queues {
                control: VecDeque::new(),
                control_overflow: false,
                audio: VecDeque::new(),
                audio_discontinuity: HashMap::new(),
                video: VecDeque::new(),
                video_bytes: 0,
                awaiting_keyframe: false,
                close: None,
                terminated: false,
                bitrate_kbps,
                frames_since_attach: 0,
                keyframes_sent: 0,
                frames_dropped: 0,
                dropped_since_sample: 0,
                bytes_sent: 0,
                last_sent_sequence: None,
                ack: AckState::default(),
                pending_actions: Vec::new(),
                action_frame_timeouts: 0,
                actions_dispatched: 0,
            }),
            notify: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queues> {
        self.queues
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn push_control(&self, message: ServerMessage) {
        {
            let mut queues = self.lock();
            queues.push_bounded(message);
        }
        self.notify.notify_one();
    }

    pub(crate) fn close(&self, code: u16) {
        let mut queues = self.lock();
        if !queues.terminated {
            queues.close.get_or_insert(code);
        }
        drop(queues);
        self.notify.notify_one();
    }

    pub(crate) fn set_awaiting_keyframe(&self) {
        self.lock().awaiting_keyframe = true;
    }

    /// Queue a frame. Returns true when this viewer needs a keyframe from
    /// the encoder (it just dropped its queue).
    pub(crate) fn push_video(&self, packet: &Arc<VideoPacket>, replay: bool) -> bool {
        let mut needs_keyframe = false;
        {
            let mut queues = self.lock();
            if queues.close.is_some() || queues.terminated {
                return false;
            }
            // Action-to-frame ordering.
            if !replay {
                let mut correlations = Vec::new();
                for action in &mut queues.pending_actions {
                    if action.first_frame.is_none() && packet.sequence() >= action.boundary {
                        action.first_frame = Some(packet.sequence());
                        if action.completed {
                            correlations.push(action.action_id.clone());
                        }
                    }
                }
                queues
                    .pending_actions
                    .retain(|action| !(action.completed && action.first_frame.is_some()));
                for action_id in correlations {
                    queues.push_bounded(ServerMessage::ActionFrameCorrelation(
                        ActionFrameCorrelation {
                            action_id,
                            session_id: packet.descriptor.session_id.clone(),
                            first_frame_sequence_after: Some(FrameSequence(packet.sequence())),
                        },
                    ));
                }
            }
            if self.codec != VideoCodec::H264 {
                // Independent frames: newest wins.
                queues.frames_dropped += queues.video.len() as u64;
                queues.video.clear();
                queues.video_bytes = 0;
                queues.video_bytes += packet.payload.len();
                queues.video.push_back(packet.clone());
            } else if packet.keyframe {
                // A keyframe supersedes everything queued before it.
                let superseded = queues.video.len() as u64;
                queues.frames_dropped += superseded;
                queues.video.clear();
                queues.video_bytes = packet.payload.len();
                queues.video.push_back(packet.clone());
                queues.awaiting_keyframe = false;
                queues.ack.stalled_since = None;
            } else if queues.awaiting_keyframe {
                // Dependent frame without its keyframe: undecodable here.
            } else {
                queues.video_bytes += packet.payload.len();
                queues.video.push_back(packet.clone());
                let budget = queue_budget(queues.bitrate_kbps);
                let too_old = queues
                    .video
                    .front()
                    .is_some_and(|front| front.captured_at.elapsed() > MAX_QUEUE_DELAY);
                if queues.video_bytes > budget || too_old {
                    let dropped = queues.video.len() as u64;
                    queues.frames_dropped += dropped;
                    queues.dropped_since_sample += dropped;
                    queues.video.clear();
                    queues.video_bytes = 0;
                    queues.awaiting_keyframe = true;
                    needs_keyframe = true;
                }
            }
        }
        self.notify.notify_one();
        needs_keyframe
    }

    pub(crate) fn push_audio(&self, track_id: u16, packet: Arc<[u8]>, duration_us: u64) {
        {
            let mut queues = self.lock();
            if queues.close.is_some() || queues.terminated {
                return;
            }
            queues.audio.push_back(QueuedAudio {
                track_id,
                packet,
                duration_us,
            });
            loop {
                let queued: u64 = queues
                    .audio
                    .iter()
                    .filter(|audio| audio.track_id == track_id)
                    .map(|audio| audio.duration_us)
                    .sum();
                if queued <= MAX_QUEUED_AUDIO_US {
                    break;
                }
                let Some(position) = queues
                    .audio
                    .iter()
                    .position(|audio| audio.track_id == track_id)
                else {
                    break;
                };
                queues.audio.remove(position);
                queues.audio_discontinuity.insert(track_id, true);
            }
        }
        self.notify.notify_one();
    }

    /// Next item to send, if any, applying the ack window.
    fn try_next(&self) -> Option<Outbound> {
        let mut queues = self.lock();
        if queues.terminated {
            return None;
        }
        if std::mem::take(&mut queues.control_overflow) {
            queues.close.get_or_insert(v2::close_code::INTERNAL);
        }
        if let Some(message) = queues.control.pop_front() {
            return Some(Outbound::Control(message));
        }
        if let Some(code) = queues.close.take() {
            // Terminal: drop everything still queued and hand out Close once.
            queues.terminated = true;
            queues.audio.clear();
            queues.video.clear();
            queues.video_bytes = 0;
            queues.pending_actions.clear();
            queues.ack.in_flight.clear();
            return Some(Outbound::Close(code));
        }
        if let Some(audio) = queues.audio.pop_front() {
            let discontinuity = queues
                .audio_discontinuity
                .remove(&audio.track_id)
                .unwrap_or(false);
            let packet = if discontinuity && audio.packet.len() >= AUDIO_PACKET_HEADER_BYTES {
                let mut bytes = audio.packet.to_vec();
                bytes[5] |= AUDIO_FLAG_DISCONTINUITY;
                Arc::from(bytes)
            } else {
                audio.packet
            };
            return Some(Outbound::Audio(packet));
        }
        let front_is_key = queues.video.front().is_some_and(|packet| packet.keyframe);
        if queues.ack.enabled && !front_is_key && self.codec == VideoCodec::H264 {
            let allowance = ack_allowance(&queues.ack);
            if queues.ack.in_flight.len() >= allowance || queues.ack.stalled_since.is_some() {
                // Hold dependent frames; they are dropped when a keyframe or
                // an overflow arrives.
                if queues.ack.in_flight.len() >= allowance && !queues.video.is_empty() {
                    let dropped = queues.video.len() as u64;
                    queues.frames_dropped += dropped;
                    queues.dropped_since_sample += dropped;
                    queues.video.clear();
                    queues.video_bytes = 0;
                    queues.awaiting_keyframe = true;
                }
                return None;
            }
        }
        let packet = queues.video.pop_front()?;
        queues.video_bytes = queues.video_bytes.saturating_sub(packet.payload.len());
        queues.frames_since_attach += 1;
        queues.bytes_sent += packet.payload.len() as u64;
        queues.last_sent_sequence = Some(packet.sequence());
        if packet.keyframe {
            queues.keyframes_sent += 1;
        }
        if queues.ack.enabled {
            queues
                .ack
                .in_flight
                .push_back((packet.sequence(), Instant::now()));
            while queues.ack.in_flight.len() > 1024 {
                queues.ack.in_flight.pop_front();
            }
        }
        Some(Outbound::Video(packet))
    }

    /// Periodic maintenance. Returns true when a keyframe is needed.
    pub(crate) fn tick(&self) -> bool {
        let mut needs_keyframe = false;
        let mut notify = false;
        {
            let mut queues = self.lock();
            let now = Instant::now();
            // Action correlation deadlines.
            let mut timeouts = Vec::new();
            for action in &mut queues.pending_actions {
                if action.completed
                    && action.first_frame.is_none()
                    && action.deadline.is_some_and(|deadline| now >= deadline)
                {
                    timeouts.push(action.action_id.clone());
                }
            }
            if !timeouts.is_empty() {
                queues
                    .pending_actions
                    .retain(|action| !timeouts.contains(&action.action_id));
                queues.action_frame_timeouts += timeouts.len() as u64;
                for action_id in timeouts {
                    queues.push_bounded(ServerMessage::ActionFrameCorrelation(
                        ActionFrameCorrelation {
                            action_id,
                            session_id: self.session_id.clone(),
                            first_frame_sequence_after: None,
                        },
                    ));
                }
                notify = true;
            }
            // Stall detection for acking clients.
            if queues.ack.enabled {
                let oldest_unacked = queues.ack.in_flight.front().map(|(_, at)| *at);
                if let Some(sent_at) = oldest_unacked {
                    if now.saturating_duration_since(sent_at) > STALL_AFTER
                        && queues.ack.stalled_since.is_none()
                    {
                        queues.ack.stalled_since = Some(now);
                    }
                }
                if let Some(since) = queues.ack.stalled_since {
                    let probe_due = queues
                        .ack
                        .last_probe
                        .is_none_or(|last| now.duration_since(last) >= STALL_PROBE_EVERY);
                    if now.duration_since(since) >= STALL_PROBE_EVERY && probe_due {
                        queues.ack.last_probe = Some(now);
                        queues.ack.in_flight.clear();
                        queues.awaiting_keyframe = true;
                        needs_keyframe = true;
                    }
                }
            }
        }
        if notify {
            self.notify.notify_one();
        }
        needs_keyframe
    }

    pub(crate) fn take_congestion(&self) -> CongestionSignal {
        let mut queues = self.lock();
        let queue_delay_ms = queues
            .video
            .front()
            .map(|packet| packet.captured_at.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        CongestionSignal {
            queue_delay_ms,
            ack_excess_ms: queues.ack.excess_ms,
            dropped_frames: std::mem::take(&mut queues.dropped_since_sample),
        }
    }

    fn on_ack(&self, ack: &FrameAck) {
        let mut queues = self.lock();
        let now = Instant::now();
        queues.ack.enabled = true;
        if queues
            .ack
            .last_acked
            .is_some_and(|last| ack.sequence <= last)
        {
            return;
        }
        let mut rtt = None;
        while let Some((sequence, sent_at)) = queues.ack.in_flight.front().copied() {
            if sequence > ack.sequence {
                break;
            }
            queues.ack.in_flight.pop_front();
            if sequence == ack.sequence {
                rtt = Some(
                    now.duration_since(sent_at).as_secs_f64() * 1000.0
                        - f64::from(ack.decode_us.unwrap_or(0)) / 1000.0,
                );
            }
        }
        if let Some(rtt) = rtt.map(|rtt| rtt.max(0.0)) {
            let min = queues.ack.min_rtt_ms.map_or(rtt, |min| min.min(rtt));
            queues.ack.min_rtt_ms = Some(min);
            let smoothed = queues
                .ack
                .rtt_ms
                .map_or(rtt, |previous| previous * 0.8 + rtt * 0.2);
            queues.ack.rtt_ms = Some(smoothed);
            queues.ack.excess_ms = (smoothed - min).max(0.0);
        }
        queues.ack.last_acked = Some(ack.sequence);
        queues.ack.last_ack_at = Some(now);
        queues.ack.stalled_since = None;
        // Measured decode rate, updated only while healthy so a stall cannot
        // shrink its own allowance.
        let started = *queues.ack.window_started.get_or_insert(now);
        queues.ack.acks_in_window += 1;
        let elapsed = now.duration_since(started).as_secs_f64();
        if elapsed >= 1.0 {
            if queues.ack.excess_ms < 150.0 {
                queues.ack.measured_fps = f64::from(queues.ack.acks_in_window) / elapsed;
            }
            queues.ack.acks_in_window = 0;
            queues.ack.window_started = Some(now);
        }
        drop(queues);
        self.notify.notify_one();
    }
}

fn queue_budget(bitrate_kbps: u32) -> usize {
    // About two seconds of stream.
    ((bitrate_kbps as usize) * 1000 / 8 * 2).max(MIN_QUEUE_BUDGET_BYTES)
}

fn ack_allowance(ack: &AckState) -> usize {
    let fps = if ack.measured_fps > 0.0 {
        ack.measured_fps
    } else {
        30.0
    };
    let mut seconds = ACK_WINDOW_SECONDS;
    if let Some(rtt) = ack.rtt_ms {
        if rtt > 50.0 {
            seconds -= (rtt.min(1000.0)) / 1000.0;
        }
    }
    ((fps * seconds.max(0.5)) as usize).max(MIN_ACK_WINDOW_FRAMES)
}

/// A socket attached to a media session. Drop it to detach.
pub struct Viewer {
    session: Arc<MediaSession>,
    shared: Arc<ViewerShared>,
    input_base: Option<u64>,
    uplink_denied_reported: bool,
    last_geometry_revision: u64,
    latest_snapshot: Option<AccessibilitySnapshotId>,
    input_events_dispatched: Arc<AtomicU64>,
    /// Batches for the input worker, in arrival order (see `input_worker`).
    input_jobs: Option<tokio::sync::mpsc::UnboundedSender<InputJob>>,
}

/// How long `handle` waits for a batch's delivery before leaving its
/// acknowledgement to the input worker.
const INPUT_REPLY_BUDGET: Duration = Duration::from_millis(250);

/// One accepted batch for the input worker.
struct InputJob {
    lease: Arc<dyn cua_spacesd_provider_api::InteractiveInputLease>,
    batch: InteractiveInputBatch,
    through: u64,
    announced: crate::input_activity::Announced,
    done: tokio::sync::oneshot::Sender<()>,
}

/// Deliver one batch through its lease and build its acknowledgement.
async fn run_input_job(
    session_id: &WindowSessionId,
    lease: Arc<dyn cua_spacesd_provider_api::InteractiveInputLease>,
    batch: InteractiveInputBatch,
    through: u64,
    announced: crate::input_activity::Announced,
    dispatched: &AtomicU64,
) -> ServerMessage {
    let started = Instant::now();
    let events = batch.events.len() as u64;
    let nack = |code: ActionErrorCode, message: String| {
        ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
            session_id: session_id.clone(),
            through_sequence: through,
            delivered: false,
            error: Some(ActionError {
                code,
                message,
                current_geometry_epoch: None,
            }),
            host_dispatch_us: None,
        })
    };
    let result = tokio::task::spawn_blocking(move || {
        let _guard = announced.acquire_blocking();
        lease.dispatch(&batch)
    })
    .await;
    match result {
        Ok(Ok(outcome)) => {
            dispatched.fetch_add(events, Ordering::Relaxed);
            ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                session_id: session_id.clone(),
                through_sequence: through,
                delivered: true,
                error: None,
                host_dispatch_us: Some(
                    outcome
                        .dispatch_micros
                        .max(started.elapsed().as_micros() as u64),
                ),
            })
        }
        Ok(Err(error)) => nack(provider_code(&error), error.message),
        Err(error) => nack(ActionErrorCode::DeliveryFailed, error.to_string()),
    }
}

impl std::fmt::Debug for Viewer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Viewer")
            .field("session", &self.session.id())
            .field("id", &self.shared.id)
            .finish()
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.session.detach(self.shared.id);
    }
}

impl Viewer {
    pub(crate) fn new(session: Arc<MediaSession>, shared: Arc<ViewerShared>) -> Self {
        Self {
            session,
            shared,
            input_base: None,
            uplink_denied_reported: false,
            last_geometry_revision: 0,
            latest_snapshot: None,
            input_events_dispatched: Arc::new(AtomicU64::new(0)),
            input_jobs: None,
        }
    }

    pub fn session(&self) -> &Arc<MediaSession> {
        &self.session
    }

    pub fn id(&self) -> u64 {
        self.shared.id
    }

    /// Wait for the next item to send.
    pub async fn next_outbound(&self) -> Outbound {
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(item) = self.shared.try_next() {
                return item;
            }
            notified.await;
        }
    }

    /// Non-blocking variant for tests and pollers.
    pub fn try_next_outbound(&self) -> Option<Outbound> {
        self.shared.try_next()
    }

    fn reply(&self, message: ServerMessage) {
        self.shared.push_control(message);
    }

    fn session_id(&self) -> WindowSessionId {
        WindowSessionId(self.session.id().to_owned())
    }

    fn check_session(&self, session_id: &WindowSessionId) -> bool {
        if session_id.0 == self.session.id() || session_id.0.is_empty() {
            return true;
        }
        self.reply(ServerMessage::error(
            ErrorCode::UnknownSession,
            "message names a different media session",
        ));
        false
    }

    /// Handle one binary message from the client (uplink audio).
    pub fn handle_binary(&mut self, bytes: &[u8]) {
        let Some(uplink) = self.session.uplink.lock().unwrap().clone() else {
            self.deny_uplink();
            return;
        };
        let Ok((header, payload)) = decode_audio_packet(bytes) else {
            return;
        };
        if header.track_id != uplink.grant.track_id {
            self.deny_uplink();
            return;
        }
        uplink.packets.fetch_add(1, Ordering::Relaxed);
        {
            let mut last = uplink.last_sequence.lock().unwrap();
            if let Some(previous) = *last {
                if !sequence_after(header.sequence, previous) {
                    uplink.late.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let gap = header.sequence.wrapping_sub(previous).saturating_sub(1);
                uplink.lost.fetch_add(u64::from(gap), Ordering::Relaxed);
            }
            *last = Some(header.sequence);
        }
        if uplink.muted.load(Ordering::Relaxed) {
            return;
        }
        uplink.sink.on_frame(AudioFrame {
            pts_us: header.pts_us,
            frame_samples: header.frame_samples,
            payload: payload.to_vec(),
            dtx: header.dtx,
        });
    }

    fn deny_uplink(&mut self) {
        if !self.uplink_denied_reported {
            self.uplink_denied_reported = true;
            self.reply(ServerMessage::error(
                ErrorCode::AudioUplinkDenied,
                "no uplink audio track was granted for this session",
            ));
        }
    }

    /// Handle one control message from the client. Replies are queued and
    /// come out of [`Viewer::next_outbound`].
    pub async fn handle(&mut self, message: ClientMessage) {
        match message {
            ClientMessage::Ticket { .. } => self.reply(ServerMessage::error(
                ErrorCode::InvalidMessage,
                "the socket is already attached",
            )),
            ClientMessage::InteractiveInput(batch) => {
                let ack = self.dispatch_input(batch).await;
                if let Some(ack) = ack {
                    self.reply(ack);
                }
            }
            ClientMessage::Action(action) => self.dispatch_action(action),
            ClientMessage::RequestKeyframe { session_id } => {
                if self.check_session(&session_id) {
                    self.shared.set_awaiting_keyframe();
                    self.session.request_keyframe_now();
                    self.reply(ServerMessage::KeyframeRequested { session_id });
                }
            }
            ClientMessage::SetStreamPreferences(preferences) => {
                if !self.check_session(&preferences.session_id) {
                    return;
                }
                let session = self.session.clone();
                let result = tokio::task::spawn_blocking(move || {
                    session.set_preferences(
                        preferences.max_fps,
                        preferences.max_dimension,
                        preferences.target_bitrate_kbps.unwrap_or(0),
                    )
                })
                .await;
                match result {
                    Ok(Ok((max_fps, max_dimension, bitrate))) => {
                        self.shared.lock().bitrate_kbps = bitrate;
                        self.reply(ServerMessage::StreamPreferencesApplied(StreamPreferences {
                            session_id: self.session_id(),
                            max_fps,
                            max_dimension,
                            target_bitrate_kbps: Some(bitrate),
                        }));
                    }
                    Ok(Err(error)) => self.reply(ServerMessage::error(
                        ErrorCode::CaptureFailed,
                        error.to_string(),
                    )),
                    Err(error) => self.reply(ServerMessage::error(
                        ErrorCode::Internal,
                        error.to_string(),
                    )),
                }
            }
            ClientMessage::SetWindowGeometry(request) => self.set_geometry(request).await,
            ClientMessage::GetWindowState { session_id } => {
                if self.check_session(&session_id) {
                    self.window_state().await;
                }
            }
            ClientMessage::GetStats { session_id } => {
                if self.check_session(&session_id) {
                    self.reply(ServerMessage::Stats(Box::new(self.stats())));
                }
            }
            ClientMessage::AudioUplinkState { track_id, muted } => {
                let uplink = self.session.uplink.lock().unwrap().clone();
                match uplink {
                    Some(uplink) if uplink.grant.track_id == track_id => {
                        uplink.muted.store(muted, Ordering::Relaxed);
                        uplink.sink.set_muted(muted);
                    }
                    _ => self.deny_uplink(),
                }
            }
            ClientMessage::FrameAck(ack) => {
                if ack.session_id.0 == self.session.id() || ack.session_id.0.is_empty() {
                    self.shared.on_ack(&ack);
                }
            }
            ClientMessage::Ping { nonce } => self.reply(ServerMessage::Pong { nonce }),
            ClientMessage::Unsupported => self.reply(ServerMessage::error(
                ErrorCode::UnsupportedOperation,
                "this message is not part of rcdp wire v2 (session setup, presence and clipboard moved to gRPC)",
            )),
        }
    }

    async fn dispatch_input(&mut self, batch: InteractiveInputBatch) -> Option<ServerMessage> {
        let session_id = self.session_id();
        let nack = |through: u64, code: ActionErrorCode, message: String| {
            ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                session_id: session_id.clone(),
                through_sequence: through,
                delivered: false,
                error: Some(ActionError {
                    code,
                    message,
                    current_geometry_epoch: None,
                }),
                host_dispatch_us: None,
            })
        };
        if !self.check_session(&batch.session_id) {
            return None;
        }
        let through = match batch.validate() {
            Ok(through) => through,
            Err(message) => {
                return Some(ServerMessage::error(ErrorCode::InvalidMessage, message));
            }
        };
        // Adopt the first batch's start as the base; afterwards contiguous.
        if let Some(expected) = self.input_base {
            if batch.first_sequence != expected {
                return Some(ServerMessage::Error {
                    code: ErrorCode::InputSequenceGap,
                    message: format!(
                        "input batch starts at {} but {} was expected",
                        batch.first_sequence, expected
                    ),
                    expected_sequence: Some(expected),
                });
            }
        }
        self.input_base = Some(through + 1);
        let principal = self.session.principal().clone();
        if let Some(leases) = self.session.leases() {
            match leases.acquire(self.session.target_key(), &principal.id, &principal.name) {
                Ok(LeaseGrant::Held) => {}
                Ok(LeaseGrant::Transferred { .. }) => {
                    if let Some(input) = self.session.input.lock().unwrap().as_ref() {
                        input.release_all();
                    }
                }
                Err(LeaseConflict {
                    holder_name,
                    retry_after,
                    ..
                }) => {
                    return Some(nack(
                        through,
                        ActionErrorCode::RateLimited,
                        format!(
                            "input to this target is leased to {holder_name}; retry in {} ms",
                            retry_after.as_millis()
                        ),
                    ));
                }
            }
        }
        let lease = match self.session.input_lease() {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                return Some(nack(
                    through,
                    ActionErrorCode::ViewOnly,
                    "interactive input is not available for this session".into(),
                ))
            }
            Err(error) => return Some(nack(through, provider_code(&error), error.message)),
        };
        // Presence: this viewer's principal now drives the pointer; the
        // cursor-shape probe waits for (and aborts on) this injection.
        let announced = crate::input_activity::global().announce(&principal.id);
        let (done, finished) = tokio::sync::oneshot::channel();
        let job = InputJob {
            lease,
            batch,
            through,
            announced,
            done,
        };
        if let Err(tokio::sync::mpsc::error::SendError(job)) = self.input_worker().send(job) {
            return Some(nack(
                job.through,
                ActionErrorCode::DeliveryFailed,
                "the input worker stopped".into(),
            ));
        }
        // The worker acknowledges the batch itself. A quick delivery (every
        // native session) is acknowledged before this returns; a slow one
        // (tool-backed Hyprland input) is acknowledged when it finishes,
        // without holding up this socket's frames, pings and replies.
        let _ = tokio::time::timeout(INPUT_REPLY_BUDGET, finished).await;
        None
    }

    /// The ordered input worker of this socket, started on first use.
    fn input_worker(&mut self) -> &tokio::sync::mpsc::UnboundedSender<InputJob> {
        let session_id = self.session_id();
        let shared = self.shared.clone();
        let dispatched = self.input_events_dispatched.clone();
        self.input_jobs.get_or_insert_with(|| {
            let (jobs, mut queue) = tokio::sync::mpsc::unbounded_channel::<InputJob>();
            tokio::spawn(async move {
                while let Some(job) = queue.recv().await {
                    let ack = run_input_job(
                        &session_id,
                        job.lease,
                        job.batch,
                        job.through,
                        job.announced,
                        &dispatched,
                    )
                    .await;
                    shared.push_control(ack);
                    let _ = job.done.send(());
                }
            });
            jobs
        })
    }

    fn dispatch_action(&mut self, action: cua_media_protocol::ActionRequest) {
        if !self.check_session(&action.session_id) {
            return;
        }
        let session_id = self.session_id();
        let fail = |code: ActionErrorCode, message: &str, epoch: Option<u64>| {
            ServerMessage::ActionResult(ActionResult {
                action_id: action.action_id.clone(),
                delivered: false,
                error: Some(ActionError {
                    code,
                    message: message.into(),
                    current_geometry_epoch: epoch.map(cua_media_protocol::GeometryEpoch),
                }),
                first_frame_sequence_after: None,
            })
        };
        if matches!(
            self.session.target_ref(),
            v2::MediaTargetRef::Display { .. }
        ) {
            self.reply(fail(
                ActionErrorCode::Unsupported,
                "actions address a window; use ComputerService for display input",
                None,
            ));
            return;
        }
        if contains_native_key(&action.arguments) {
            self.reply(fail(
                ActionErrorCode::NativeTargetRejected,
                "native target identifiers are not accepted",
                None,
            ));
            return;
        }
        let (epoch, geometry) = self.session.geometry();
        match &action.basis {
            ActionBasis::Pixel { geometry_epoch, .. } if geometry_epoch.0 != epoch => {
                self.reply(fail(
                    ActionErrorCode::StaleGeometry,
                    "pixel basis does not match the current geometry epoch",
                    Some(epoch),
                ));
                return;
            }
            ActionBasis::Accessibility { snapshot_id }
                if self.latest_snapshot != Some(*snapshot_id) =>
            {
                self.reply(fail(
                    ActionErrorCode::StaleAccessibilitySnapshot,
                    "accessibility snapshot is stale",
                    Some(epoch),
                ));
                return;
            }
            _ => {}
        }
        let Some(providers) = self.session.providers() else {
            return;
        };
        let boundary = self.session.video.lock().unwrap().next_sequence;
        {
            let mut queues = self.shared.lock();
            queues.actions_dispatched += 1;
            if queues.pending_actions.len() >= MAX_PENDING_ACTIONS {
                queues.pending_actions.remove(0);
            }
            queues.pending_actions.push(PendingAction {
                action_id: action.action_id.clone(),
                boundary,
                first_frame: None,
                completed: false,
                deadline: None,
            });
        }
        let invocation = ActionInvocation {
            action_id: action.action_id.clone(),
            action: action.tool.clone(),
            arguments: action.arguments.clone(),
            basis: action.basis.clone(),
            coordinate_space: Some(geometry),
        };
        let target = self.session.target.id.clone();
        let policy = self.session.policy();
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let outcome = providers.actions.perform(&target, invocation, policy).await;
            let mut queues = shared.lock();
            let action_id = action.action_id.clone();
            let first_frame = queues
                .pending_actions
                .iter()
                .find(|pending| pending.action_id == action_id)
                .and_then(|pending| pending.first_frame);
            let result = match outcome {
                Ok(outcome) => ActionResult {
                    action_id: action_id.clone(),
                    delivered: outcome.delivered,
                    error: None,
                    first_frame_sequence_after: first_frame.map(FrameSequence),
                },
                Err(error) => ActionResult {
                    action_id: action_id.clone(),
                    delivered: false,
                    error: Some(ActionError {
                        code: provider_code(&error),
                        message: error.message,
                        current_geometry_epoch: None,
                    }),
                    first_frame_sequence_after: None,
                },
            };
            let delivered = result.delivered;
            queues.push_bounded(ServerMessage::ActionResult(result));
            if first_frame.is_some() || !delivered {
                queues
                    .pending_actions
                    .retain(|pending| pending.action_id != action_id);
            } else if let Some(pending) = queues
                .pending_actions
                .iter_mut()
                .find(|pending| pending.action_id == action_id)
            {
                pending.completed = true;
                pending.deadline = Some(Instant::now() + ACTION_FRAME_WAIT);
            }
            let _ = session_id;
            drop(queues);
            shared.notify.notify_one();
        });
    }

    async fn window_state(&mut self) {
        if matches!(
            self.session.target_ref(),
            v2::MediaTargetRef::Display { .. }
        ) {
            self.reply(ServerMessage::error(
                ErrorCode::UnsupportedOperation,
                "window state applies to window targets; use AccessibilityService",
            ));
            return;
        }
        let Some(providers) = self.session.providers() else {
            return;
        };
        match providers
            .accessibility
            .snapshot(&self.session.target.id)
            .await
        {
            Ok(snapshot) => {
                self.latest_snapshot = Some(snapshot.snapshot_id);
                self.reply(ServerMessage::WindowState(WindowState {
                    session_id: self.session_id(),
                    snapshot_id: snapshot.snapshot_id,
                    state: snapshot.state,
                }));
            }
            Err(error) => self.reply(ServerMessage::error(
                ErrorCode::WindowStateFailed,
                error.message,
            )),
        }
    }

    async fn set_geometry(&mut self, request: WindowGeometryRequest) {
        if !self.check_session(&request.session_id) {
            return;
        }
        let result = |applied: bool, width: u32, height: u32, error: Option<String>| {
            ServerMessage::WindowGeometryResult(WindowGeometryResult {
                session_id: request.session_id.clone(),
                revision: request.revision,
                applied,
                width_points: width,
                height_points: height,
                error,
            })
        };
        if self.session.target_ref().is_display() {
            self.reply(ServerMessage::error(
                ErrorCode::UnsupportedOperation,
                "display targets cannot be resized",
            ));
            return;
        }
        if self.session.geometry_control != WindowGeometryControl::Bidirectional {
            self.reply(result(
                false,
                0,
                0,
                Some("this session observes geometry only".into()),
            ));
            return;
        }
        let previous = self
            .session
            .geometry_revision
            .fetch_max(request.revision, Ordering::AcqRel);
        if request.revision <= previous.max(self.last_geometry_revision) {
            self.reply(result(false, 0, 0, Some("stale geometry revision".into())));
            return;
        }
        self.last_geometry_revision = request.revision;
        let Some(providers) = self.session.providers() else {
            return;
        };
        match providers
            .geometry
            .resize(
                &self.session.target.id,
                request.width_points,
                request.height_points,
            )
            .await
        {
            Ok(applied) => self.reply(result(
                true,
                applied.width_points,
                applied.height_points,
                None,
            )),
            Err(error) => self.reply(result(false, 0, 0, Some(error.message))),
        }
    }

    /// Current stats for this socket.
    pub fn stats(&self) -> Stats {
        let queues = self.shared.lock();
        let video = self.session.video.lock().unwrap();
        let rate = self.session.rate_decision();
        Stats {
            stream: StreamStats {
                session_id: self.session_id(),
                frames_emitted: video.frames_emitted,
                frames_replaced: queues.frames_dropped,
                keyframe_requests: video.keyframe_requests,
                pending_frames: u8::try_from(queues.video.len()).unwrap_or(u8::MAX),
                bytes_emitted: queues.bytes_sent,
                actions_dispatched: queues.actions_dispatched,
                action_frame_timeouts: queues.action_frame_timeouts,
                preference_updates: video.preference_updates,
                input_events_dispatched: self.input_events_dispatched.load(Ordering::Relaxed),
            },
            frames_since_attach: queues.frames_since_attach,
            last_frame_age_ms: video
                .last_frame_at
                .map(|at| at.elapsed().as_millis() as u64),
            target_idle: video.suspended.is_none()
                && video
                    .last_frame_at
                    .is_some_and(|at| at.elapsed() > Duration::from_millis(500)),
            keyframes_sent: queues.keyframes_sent,
            frames_dropped: queues.frames_dropped,
            queued_video_bytes: queues.video_bytes as u64,
            ack_rtt_ms: queues.ack.rtt_ms,
            encoder_bitrate_kbps: (self.session.codec() == VideoCodec::H264)
                .then_some(rate.bitrate_kbps),
            effective_max_fps: rate.max_fps,
            encoder: self.session.encoder_name(),
            audio_tracks: self.session.audio_stats(),
        }
    }
}

fn provider_code(error: &ProviderError) -> ActionErrorCode {
    match error.code {
        ProviderErrorCode::StaleTarget => ActionErrorCode::StaleTarget,
        ProviderErrorCode::PermissionDenied | ProviderErrorCode::ConsentRequired => {
            ActionErrorCode::PermissionDenied
        }
        ProviderErrorCode::TargetUnavailable => ActionErrorCode::WindowUnavailable,
        ProviderErrorCode::Unsupported => ActionErrorCode::Unsupported,
        ProviderErrorCode::ViewOnly => ActionErrorCode::ViewOnly,
        ProviderErrorCode::WouldRequireActivation => ActionErrorCode::WouldRequireActivation,
        ProviderErrorCode::DeliveryFailed
        | ProviderErrorCode::CaptureFailed
        | ProviderErrorCode::Internal => ActionErrorCode::DeliveryFailed,
    }
}

fn contains_native_key(value: &cua_media_protocol::Value) -> bool {
    match value {
        cua_media_protocol::Value::Object(map) => map.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "pid" | "window_id" | "native_window_id" | "hwnd" | "portal_token"
            ) || contains_native_key(value)
        }),
        cua_media_protocol::Value::Array(values) => values.iter().any(contains_native_key),
        _ => false,
    }
}
