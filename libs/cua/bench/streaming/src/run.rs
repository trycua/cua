// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One benchmark run with the native Rust client: open a media session on
//! the scenario's target, stream for a bounded time, decode, and measure.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{self, ClientMessage, ServerMessage};
use cua_media_protocol::{ActionBasis, ActionRequest, FrameSequence, GeometryEpoch, WireHeader};
use cua_proto::env::v1 as pb;

use crate::audio::AudioSide;
use crate::link::{self, Impair, Link, OpenParams, Raw, Transport};
use crate::report::{put_dist, round3};
use crate::space::{unix_ns, Space};
use crate::video::{self, Decoder, Outcome, Timecode};

/// Hard bound on media-plane items handled per run (memory safety).
const MAX_ITEMS: u64 = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Idle window: bytes/s floor, input→photon on an otherwise still frame.
    Static,
    /// Idle desktop target (whole display).
    DesktopStatic,
    /// Timecode strip + spinner: glass-to-glass, fps, input→photon.
    Timecode,
    Scroll,
    Video,
    /// The fixture window moves continuously; desktop target.
    Drag,
    /// A/V fixture: flash + beep each second; A/V skew and audio latency.
    Avsync,
    /// Timecode with QUIC video datagram loss injected at the client.
    Loss,
    /// Timecode with the WebSocket reader stalled 1 s every 5 s.
    Stall,
}

impl Scenario {
    pub fn all() -> &'static [Scenario] {
        use Scenario::*;
        &[
            Static,
            DesktopStatic,
            Timecode,
            Scroll,
            Video,
            Drag,
            Avsync,
            Loss,
            Stall,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Static => "static",
            Scenario::DesktopStatic => "desktop-static",
            Scenario::Timecode => "timecode",
            Scenario::Scroll => "scroll",
            Scenario::Video => "video",
            Scenario::Drag => "drag",
            Scenario::Avsync => "avsync",
            Scenario::Loss => "loss",
            Scenario::Stall => "stall",
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        Self::all()
            .iter()
            .copied()
            .find(|s| s.name() == name)
            .ok_or_else(|| format!("unknown scenario {name}"))
    }

    pub fn fixture_mode(self) -> &'static str {
        match self {
            Scenario::Static | Scenario::DesktopStatic | Scenario::Avsync => "static",
            Scenario::Timecode | Scenario::Loss | Scenario::Stall => "timecode",
            Scenario::Scroll => "scroll",
            Scenario::Video => "video",
            Scenario::Drag => "drag",
        }
    }

    /// `None` = primary display, otherwise a window title.
    pub fn target_title(self) -> Option<&'static str> {
        match self {
            Scenario::DesktopStatic | Scenario::Drag => None,
            Scenario::Avsync => Some("CUA Fixture AV Sync"),
            _ => Some("CUA Bench Timecode"),
        }
    }

    pub fn probes_input(self) -> bool {
        matches!(self, Scenario::Static | Scenario::Timecode)
    }

    /// Whether this scenario applies to a transport.
    pub fn applies(self, transport: Transport) -> bool {
        match self {
            Scenario::Loss => transport == Transport::Quic,
            Scenario::Stall => transport == Transport::Ws,
            _ => true,
        }
    }
}

pub struct RunConfig {
    pub scenario: Scenario,
    pub transport: Transport,
    pub decoder: String,
    pub seconds: u64,
    pub max_fps: u32,
    pub loss_percent: f64,
    /// Input policy for the probe clicks: XTest (activate) or XSendEvent (background).
    pub activate: bool,
}

pub struct Outcome2 {
    pub metrics: BTreeMap<String, f64>,
    pub encoder: String,
    pub target: String,
}

/// Find the scenario's target, retrying while the fixture window appears.
pub async fn find_target(
    grpc: &mut link::Grpc,
    title: Option<&str>,
) -> link::Result<(pb::media_target::Target, String)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let response = grpc
            .list_targets(pb::ListTargetsRequest {
                include_windows: true,
                window_filter: None,
            })
            .await?
            .into_inner();
        for target in &response.targets {
            match (&target.target, title) {
                (Some(pb::stream_target::Target::Display(display)), None)
                    if display.primary || response.targets.len() == 1 =>
                {
                    return Ok((
                        pb::media_target::Target::DisplayId(display.id.clone()),
                        format!("display:{}", display.id),
                    ));
                }
                (Some(pb::stream_target::Target::Window(window)), Some(title))
                    if window.title == title && target.available =>
                {
                    if let Some(reference) = &window.r#ref {
                        return Ok((
                            pb::media_target::Target::Window(reference.clone()),
                            format!("window:{title}"),
                        ));
                    }
                }
                _ => {}
            }
        }
        if title.is_none() {
            if let Some(pb::stream_target::Target::Display(display)) =
                response.targets.first().and_then(|t| t.target.clone())
            {
                return Ok((
                    pb::media_target::Target::DisplayId(display.id.clone()),
                    format!("display:{}", display.id),
                ));
            }
        }
        if Instant::now() > deadline {
            return Err(format!("target {title:?} not listed").into());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn rusage_s(who: i32) -> f64 {
    // SAFETY: getrusage writes into the zeroed struct we own.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(who, &mut usage);
        let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        tv(usage.ru_utime) + tv(usage.ru_stime)
    }
}

pub fn self_cpu_s() -> f64 {
    rusage_s(libc::RUSAGE_SELF)
}

pub fn children_cpu_s() -> f64 {
    rusage_s(libc::RUSAGE_CHILDREN)
}

struct Probe {
    sent: Instant,
    action_id: String,
    via_action: bool,
    expect: bool,
    result_at: Option<Instant>,
    correlated_seq: Option<u64>,
    correlation_at: Option<Instant>,
}

pub async fn run_native(space: &Space, config: &RunConfig) -> link::Result<Outcome2> {
    let scenario = config.scenario;
    // Fixture for this scenario.
    space.fixture(scenario.fixture_mode()).await?;
    if scenario == Scenario::Avsync {
        space.avsync(true).await?;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let (offset_ns, offset_rtt_ns) = {
        let addr = space.time_addr;
        tokio::task::spawn_blocking(move || crate::space::clock_offset(addr, 60)).await??
    };

    let mut grpc = link::grpc(&space.url, &space.token).await?;
    let (target, target_name) = find_target(&mut grpc, scenario.target_title()).await?;
    let mut decoder = Decoder::new(video::backend_named(&config.decoder)?)?;

    let driver_cpu0 = space.driver_cpu_s().await;
    let container_cpu0 = space.container_cpu_s();
    let cpu0 = self_cpu_s();
    let open_started = Instant::now();
    let open_unix = unix_ns();
    let mut link = Link::open(
        grpc,
        OpenParams {
            target,
            transport: config.transport,
            max_fps: config.max_fps,
            audio: true,
            quic_addr: Some(space.quic_addr),
            ws_base: space.url.clone(),
            impair: Impair {
                quic_video_drop: if scenario == Scenario::Loss {
                    config.loss_percent / 100.0
                } else {
                    0.0
                },
                seed: 0x5eed,
            },
            policy: if config.activate {
                pb::SessionPolicy::AllowActivation
            } else {
                pb::SessionPolicy::BackgroundOnly
            },
        },
    )
    .await?;
    let open_ms = open_started.elapsed().as_secs_f64() * 1e3;
    // Timecode cells are read at the fixture's CELL size (scale 1), as since
    // the bench landed; the geometry's scale is not applied.
    let mut timecode = Timecode::new(1.0);
    let mut audio = AudioSide::new(1000.0);

    let mut m = BTreeMap::new();
    let mut first_frame: Option<f64> = None;
    let mut frame_times: Vec<Instant> = Vec::new();
    let mut g2g: Vec<f64> = Vec::new();
    let mut frames_received = 0u64;
    let mut frames_decoded = 0u64;
    let mut keyframes = 0u64;
    let mut video_bytes = 0u64;
    let mut incomplete = 0u64;
    let mut keyframe_requests = 0u64;
    let mut last_keyframe_request: Option<Instant> = None;
    let mut loss_started: Option<Instant> = None;
    let mut recovery: Vec<f64> = Vec::new();
    let mut geometry_epoch = 0u64;
    let mut last_sequence = 0u64;
    let mut photon_state: Option<bool> = None;
    let mut probe: Option<Probe> = None;
    let mut next_probe = Instant::now() + Duration::from_millis(1500);
    let mut input_photon: Vec<f64> = Vec::new();
    let mut action_result_ms: Vec<f64> = Vec::new();
    let mut correlation_ms: Vec<f64> = Vec::new();
    let mut correlated_to_photon_frames: Vec<f64> = Vec::new();
    let mut probe_misses = 0u64;
    let mut action_misses = 0u64;
    let mut action_photon: Vec<f64> = Vec::new();
    let mut input_ack_ms: Vec<f64> = Vec::new();
    let mut input_sequence: u64 = 1000;
    let mut frame_size: (u32, u32) = (0, 0);
    let mut probe_seq = 0u64;
    let mut flashes: Vec<(u64, i64)> = Vec::new(); // (capture_us, decoded unix_ns)
    let mut bright = false;
    let mut last_stats: Option<Box<v2::Stats>> = None;
    let mut errors: Vec<String> = Vec::new();
    let mut last_hash = 0u64;
    let mut stall_until: Option<Instant> = None;
    let mut next_stall = Instant::now() + Duration::from_secs(4);
    let mut stall_resumed: Option<Instant> = None;
    let mut stalls = 0u64;
    let mut miss_streak = 0u64;

    let deadline = Instant::now() + Duration::from_secs(config.seconds);
    let mut items = 0u64;
    while Instant::now() < deadline && items < MAX_ITEMS {
        // Stall injection: stop reading the socket (WS backpressure).
        if scenario == Scenario::Stall {
            if let Some(until) = stall_until {
                if Instant::now() < until {
                    tokio::time::sleep(until - Instant::now()).await;
                }
                stall_until = None;
                stall_resumed = Some(Instant::now());
                next_stall = Instant::now() + Duration::from_secs(4);
            } else if Instant::now() >= next_stall
                && Instant::now() + Duration::from_millis(2500) < deadline
            {
                stall_until = Some(Instant::now() + Duration::from_secs(1));
                stalls += 1;
                continue;
            }
        }
        // Input probe.
        if scenario.probes_input() && probe.is_none() && Instant::now() >= next_probe {
            if let (Some(state), Some((x, y))) = (photon_state, timecode.photon_center()) {
                probe_seq += 1;
                // Alternate the two input paths: media-plane `action` (window
                // targets, ActionFrameCorrelation) and `interactive_input`.
                let via_action = probe_seq.is_multiple_of(2);
                let action_id = format!("bench-{probe_seq}");
                let session_id = cua_media_protocol::WindowSessionId(link.session_id.clone());
                let message = if via_action {
                    ClientMessage::Action(ActionRequest {
                        action_id: action_id.clone(),
                        session_id,
                        tool: "click".into(),
                        arguments: serde_json::json!({"x": x.round(), "y": y.round()}),
                        basis: ActionBasis::Pixel {
                            geometry_epoch: GeometryEpoch(geometry_epoch),
                            frame_sequence: FrameSequence(last_sequence),
                        },
                    })
                } else {
                    let (w, h) = frame_size;
                    let pointer = |phase| cua_media_protocol::InteractiveInputEvent::Pointer {
                        phase,
                        button: Some(cua_media_protocol::InputPointerButton::Left),
                        x_normalized: x / f64::from(w.max(1)),
                        y_normalized: y / f64::from(h.max(1)),
                        modifiers: vec![],
                    };
                    let batch = cua_media_protocol::InteractiveInputBatch {
                        session_id,
                        first_sequence: input_sequence,
                        events: vec![
                            pointer(cua_media_protocol::InputPointerPhase::Move),
                            pointer(cua_media_protocol::InputPointerPhase::Down),
                            pointer(cua_media_protocol::InputPointerPhase::Up),
                        ],
                    };
                    input_sequence += 3;
                    ClientMessage::InteractiveInput(batch)
                };
                if std::env::var_os("CUA_BENCH_DEBUG").is_some() {
                    eprintln!(
                        "  probe {}",
                        cua_media_protocol::v2::encode_client_text(&message)
                    );
                }
                link.send(&message).await?;
                probe = Some(Probe {
                    sent: Instant::now(),
                    action_id,
                    via_action,
                    expect: !state,
                    result_at: None,
                    correlated_seq: None,
                    correlation_at: None,
                });
            }
            next_probe = Instant::now() + Duration::from_millis(1500);
        }
        if let Some(p) = &probe {
            if p.sent.elapsed() > Duration::from_secs(2) {
                if p.via_action {
                    action_misses += 1;
                } else {
                    probe_misses += 1;
                }
                probe = None;
                // Resync the photon state from the next frames.
                photon_state = None;
            }
        }

        let Some(raw) = link.next(Duration::from_millis(50)).await? else {
            continue;
        };
        items += 1;
        match raw {
            Raw::Closed(code) => {
                errors.push(format!("media socket closed ({code:?})"));
                break;
            }
            Raw::Loss { incomplete: n, .. } => {
                incomplete += n;
                decoder.lost();
                loss_started.get_or_insert_with(Instant::now);
            }
            Raw::Text(text) => match link::decode_text(&text) {
                Some(ServerMessage::SessionOpened(opened)) => {
                    geometry_epoch = opened.geometry_epoch.0
                }
                Some(ServerMessage::AudioConfig(config)) => audio.configure(&config),
                Some(ServerMessage::Lifecycle {
                    event:
                        cua_media_protocol::WindowLifecycleEvent::GeometryChanged {
                            geometry_epoch: e,
                            ..
                        },
                    ..
                }) => {
                    geometry_epoch = e.0;
                }
                Some(ServerMessage::ActionResult(result)) => {
                    if std::env::var_os("CUA_BENCH_DEBUG").is_some() {
                        eprintln!("  action_result {result:?}");
                    }
                    if let Some(p) = probe.as_mut().filter(|p| p.action_id == result.action_id) {
                        p.result_at = Some(Instant::now());
                        if !result.delivered {
                            errors.push(format!(
                                "action not delivered: {:?}",
                                result.error.map(|e| e.message)
                            ));
                        }
                        if let Some(seq) = result.first_frame_sequence_after {
                            p.correlated_seq = Some(seq.0);
                            p.correlation_at = Some(Instant::now());
                        }
                    }
                }
                Some(ServerMessage::ActionFrameCorrelation(c)) => {
                    if let Some(p) = probe.as_mut().filter(|p| p.action_id == c.action_id) {
                        p.correlated_seq = c.first_frame_sequence_after.map(|s| s.0);
                        p.correlation_at = Some(Instant::now());
                    }
                }
                Some(ServerMessage::InteractiveInputAcknowledgement(ack)) => {
                    if let Some(p) = probe
                        .as_mut()
                        .filter(|p| !p.via_action && p.result_at.is_none())
                    {
                        p.result_at = Some(Instant::now());
                        input_ack_ms.push(p.sent.elapsed().as_secs_f64() * 1e3);
                    }
                    if !ack.delivered {
                        errors.push(format!(
                            "input not delivered: {:?}",
                            ack.error.map(|e| e.message)
                        ));
                    }
                }
                Some(ServerMessage::Stats(stats)) => last_stats = Some(stats),
                Some(ServerMessage::Error { code, message, .. }) => {
                    errors.push(format!("{code:?}: {message}"))
                }
                _ => {}
            },
            Raw::Binary(bytes) if cua_media_transport::audio::is_audio_packet(&bytes) => {
                if let Ok((header, payload)) =
                    cua_media_transport::audio::decode_audio_packet(&bytes)
                {
                    audio.packet(&header, payload, unix_ns());
                }
            }
            Raw::Binary(bytes) => {
                let Ok((WireHeader::Video(desc), payload)) =
                    cua_media_transport::decode_packet(&bytes)
                else {
                    continue;
                };
                frames_received += 1;
                video_bytes += payload.len() as u64;
                if desc.keyframe {
                    keyframes += 1;
                }
                last_sequence = desc.sequence.0;
                match decoder.decode(
                    desc.sequence.0,
                    desc.codec_epoch.0,
                    desc.keyframe,
                    &payload,
                    desc.capture_timestamp_us,
                ) {
                    Outcome::Frame(frame) => {
                        let now = Instant::now();
                        let now_ns = unix_ns();
                        frames_decoded += 1;
                        frame_size = (frame.width, frame.height);
                        first_frame
                            .get_or_insert_with(|| open_started.elapsed().as_secs_f64() * 1e3);
                        frame_times.push(now);
                        if let Some(started) = loss_started.take() {
                            recovery.push(started.elapsed().as_secs_f64() * 1e3);
                        }
                        let allow_scan =
                            timecode.origin().is_none() || miss_streak.is_multiple_of(10);
                        let tc = if scenario == Scenario::Avsync {
                            None
                        } else {
                            timecode.read(&frame, allow_scan)
                        };
                        if let Some(low) = tc {
                            miss_streak = 0;
                            let guest_host_ms = (now_ns + offset_ns) / 1_000_000;
                            let drawn = video::unwrap_ms(low, guest_host_ms);
                            let latency = (guest_host_ms - drawn) as f64;
                            // A still frame's strip is as old as the last redraw.
                            let animated =
                                !matches!(scenario, Scenario::Static | Scenario::DesktopStatic);
                            if animated && (-100.0..10_000.0).contains(&latency) {
                                g2g.push(latency);
                            }
                            if let Some(resumed) = stall_resumed {
                                // Recovered once a fresh frame (< 200 ms old) shows up.
                                if latency < 200.0 {
                                    recovery.push(resumed.elapsed().as_secs_f64() * 1e3);
                                    stall_resumed = None;
                                }
                            }
                        } else if scenario != Scenario::Avsync {
                            miss_streak += 1;
                        }
                        if let Some(state) = timecode.photon(&frame) {
                            if let Some(p) = &probe {
                                if state == p.expect {
                                    let elapsed = p.sent.elapsed().as_secs_f64() * 1e3;
                                    if p.via_action {
                                        action_photon.push(elapsed);
                                    } else {
                                        input_photon.push(elapsed);
                                    }
                                    if let Some(at) = p.result_at.filter(|_| p.via_action) {
                                        action_result_ms
                                            .push(at.duration_since(p.sent).as_secs_f64() * 1e3);
                                    }
                                    if let (Some(at), Some(seq)) =
                                        (p.correlation_at, p.correlated_seq)
                                    {
                                        correlation_ms
                                            .push(at.duration_since(p.sent).as_secs_f64() * 1e3);
                                        correlated_to_photon_frames
                                            .push(desc.sequence.0 as f64 - seq as f64);
                                    }
                                    probe = None;
                                }
                            }
                            photon_state = Some(state);
                        }
                        if scenario == Scenario::Avsync {
                            let luma = video::mean_luma(&frame);
                            if luma > 128.0 && !bright {
                                flashes.push((desc.capture_timestamp_us, now_ns));
                            }
                            bright = luma > 128.0;
                        }
                        last_hash = video::hash(&frame);
                    }
                    Outcome::Pending => {}
                    Outcome::NeedKeyframe => {
                        loss_started.get_or_insert_with(Instant::now);
                    }
                }
                // Keep asking (1/s) while references are missing: a lost
                // keyframe must not freeze the stream.
                if decoder.waiting() && frames_decoded > 0 {
                    loss_started.get_or_insert_with(Instant::now);
                    if last_keyframe_request.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                        last_keyframe_request = Some(Instant::now());
                        keyframe_requests += 1;
                        link.send(&ClientMessage::RequestKeyframe {
                            session_id: cua_media_protocol::WindowSessionId(
                                link.session_id.clone(),
                            ),
                        })
                        .await?;
                    }
                }
            }
        }
    }
    let elapsed = open_started.elapsed().as_secs_f64();
    let cpu1 = self_cpu_s();
    let driver_cpu1 = space.driver_cpu_s().await;
    let container_cpu1 = space.container_cpu_s();

    // Server-side stats, bounded wait.
    let _ = link
        .send(&ClientMessage::GetStats {
            session_id: cua_media_protocol::WindowSessionId(link.session_id.clone()),
        })
        .await;
    let stats_deadline = Instant::now() + Duration::from_millis(1500);
    let mut drained = 0;
    while Instant::now() < stats_deadline && drained < 10_000 {
        drained += 1;
        match link.next(Duration::from_millis(100)).await {
            Ok(Some(Raw::Text(text))) => {
                if let Some(ServerMessage::Stats(stats)) = link::decode_text(&text) {
                    last_stats = Some(stats);
                    break;
                }
            }
            Ok(Some(Raw::Closed(_))) | Err(_) => break,
            _ => {}
        }
    }
    let quic_rtt = link.quic_rtt();
    let datagrams_dropped = link.datagrams_dropped;
    let datagrams = link.datagrams;
    let wire_bytes = link.wire_bytes;
    link.close().await;
    if scenario == Scenario::Avsync {
        let _ = space.avsync(false).await;
    }

    // ---- metrics
    m.insert("open_ms".into(), round3(open_ms));
    if let Some(v) = first_frame {
        m.insert("ttff_ms".into(), round3(v));
    }
    m.insert("frames_received".into(), frames_received as f64);
    m.insert("frames_decoded".into(), frames_decoded as f64);
    m.insert("keyframes".into(), keyframes as f64);
    m.insert("fps".into(), round3(frames_decoded as f64 / elapsed));
    let intervals: Vec<f64> = frame_times
        .windows(2)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1e3)
        .collect();
    put_dist(&mut m, "interval_ms", &intervals);
    put_dist(&mut m, "g2g_ms", &g2g);
    m.insert(
        "video_bytes_per_s".into(),
        round3(video_bytes as f64 / elapsed),
    );
    m.insert(
        "wire_bytes_per_s".into(),
        round3(wire_bytes as f64 / elapsed),
    );
    m.insert(
        "audio_bytes_per_s".into(),
        round3(audio.bytes as f64 / elapsed),
    );
    m.insert("audio_packets".into(), audio.packets as f64);
    m.insert(
        "client_cpu_pct".into(),
        round3((cpu1 - cpu0) / elapsed * 100.0),
    );
    if let (Some(a), Some(b)) = (driver_cpu0, driver_cpu1) {
        m.insert("server_cpu_pct".into(), round3((b - a) / elapsed * 100.0));
    }
    if let (Some(a), Some(b)) = (container_cpu0, container_cpu1) {
        m.insert(
            "container_cpu_pct".into(),
            round3((b - a) / elapsed * 100.0),
        );
    }
    m.insert("clock_offset_ms".into(), round3(offset_ns as f64 / 1e6));
    m.insert("clock_rtt_ms".into(), round3(offset_rtt_ns as f64 / 1e6));
    put_dist(
        &mut m,
        "decode_us",
        &decoder
            .decode_us
            .iter()
            .map(|v| f64::from(*v))
            .collect::<Vec<_>>(),
    );
    m.insert("decode_errors".into(), decoder.errors as f64);
    m.insert("timecode_reads".into(), timecode.reads as f64);
    m.insert("timecode_misses".into(), timecode.misses as f64);
    m.insert("last_frame_hash_low16".into(), (last_hash & 0xffff) as f64);
    if scenario.probes_input() {
        put_dist(&mut m, "input_photon_ms", &input_photon);
        put_dist(&mut m, "action_photon_ms", &action_photon);
        put_dist(&mut m, "input_ack_ms", &input_ack_ms);
        m.insert("action_probe_misses".into(), action_misses as f64);
        put_dist(&mut m, "action_result_ms", &action_result_ms);
        put_dist(&mut m, "action_correlation_ms", &correlation_ms);
        put_dist(
            &mut m,
            "correlated_to_photon_frames",
            &correlated_to_photon_frames,
        );
        m.insert("input_probes".into(), probe_seq as f64);
        m.insert("input_probe_misses".into(), probe_misses as f64);
    }
    if scenario == Scenario::Loss || scenario == Scenario::Stall || !recovery.is_empty() {
        put_dist(&mut m, "recovery_ms", &recovery);
        m.insert("incomplete_frames".into(), incomplete as f64);
        m.insert("keyframe_requests".into(), keyframe_requests as f64);
    }
    if scenario == Scenario::Loss {
        m.insert("datagrams".into(), datagrams as f64);
        m.insert("datagrams_dropped".into(), datagrams_dropped as f64);
        m.insert("loss_percent".into(), config.loss_percent);
    }
    if scenario == Scenario::Stall {
        m.insert("stalls".into(), stalls as f64);
    }
    if let Some(rtt) = quic_rtt {
        m.insert("quic_rtt_ms".into(), round3(rtt.as_secs_f64() * 1e3));
    }
    if scenario == Scenario::Avsync {
        // Audio latency: beep onset arrival vs the wall-clock boundary it was
        // scheduled on (guest clock mapped through the offset).
        let mut audio_latency = Vec::new();
        for onset in &audio.onsets {
            let guest_ns = onset.arrival_unix_ns + offset_ns;
            let boundary_ns = guest_ns.div_euclid(1_000_000_000) * 1_000_000_000;
            audio_latency.push((guest_ns - boundary_ns) as f64 / 1e6);
        }
        put_dist(&mut m, "audio_latency_ms", &audio_latency);
        let mut flash_latency = Vec::new();
        let mut skew = Vec::new();
        for (capture_us, decoded_ns) in &flashes {
            let guest_ns = decoded_ns + offset_ns;
            let boundary_ns = guest_ns.div_euclid(1_000_000_000) * 1_000_000_000;
            flash_latency.push((guest_ns - boundary_ns) as f64 / 1e6);
            if let Some(onset) = audio
                .onsets
                .iter()
                .min_by_key(|o| (o.pts_us as i64 - *capture_us as i64).abs())
                .filter(|o| (o.pts_us as i64 - *capture_us as i64).abs() < 500_000)
            {
                skew.push((*capture_us as i64 - onset.pts_us as i64) as f64 / 1e3);
            }
        }
        put_dist(&mut m, "flash_latency_ms", &flash_latency);
        put_dist(&mut m, "av_skew_ms", &skew);
        let abs: Vec<f64> = skew.iter().map(|v| v.abs()).collect();
        put_dist(&mut m, "av_skew_abs_ms", &abs);
        m.insert("audio_onsets".into(), audio.onsets.len() as f64);
        m.insert("flashes".into(), flashes.len() as f64);
        m.insert("audio_concealed".into(), audio.concealed as f64);
    }
    let mut encoder = String::new();
    if let Some(stats) = last_stats {
        encoder = stats.encoder.clone().unwrap_or_default();
        m.insert("server_frames_dropped".into(), stats.frames_dropped as f64);
        m.insert("server_keyframes_sent".into(), stats.keyframes_sent as f64);
        m.insert(
            "server_frames_replaced".into(),
            stats.stream.frames_replaced as f64,
        );
        m.insert(
            "server_keyframe_requests".into(),
            stats.stream.keyframe_requests as f64,
        );
        if let Some(kbps) = stats.encoder_bitrate_kbps {
            m.insert("server_bitrate_kbps".into(), f64::from(kbps));
        }
        m.insert(
            "server_effective_max_fps".into(),
            f64::from(stats.effective_max_fps),
        );
    }
    let _ = open_unix;
    if frames_decoded == 0 {
        errors.push("no frames decoded".into());
    }
    if !errors.is_empty() {
        m.insert("errors".into(), errors.len() as f64);
        eprintln!(
            "  [{}] notes: {}",
            scenario.name(),
            errors
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    if frames_decoded == 0 {
        return Err(errors.join("; ").into());
    }
    Ok(Outcome2 {
        metrics: m,
        encoder,
        target: target_name,
    })
}
