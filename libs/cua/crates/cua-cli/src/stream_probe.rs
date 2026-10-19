//! `cua sb stream-probe` (hidden): a headless stream viewer for image gates
//! and measurements.
//!
//! It opens a media session the way the Spaces viewers do (the primary
//! display with `allow_activation`, or one window with `background_only`,
//! reopened with `allow_activation` when the Space refuses background input
//! to it), then reports, as one JSON object:
//!
//! - time to open the session and to the first frame, the codec and size;
//! - the frame rate over `--seconds`;
//! - media-socket ping round trips (`ping` / `pong`);
//! - for each `--action`, sent as `interactive_input` on the media socket:
//!   the acknowledgement (delivered or refused, and the time to it), the
//!   first frame after it and, in the Cua Spaces `cua` (which decodes),
//!   the time until the picture visibly changed.
//!
//! A visible change is a frame that differs from the last frame before the
//! input in more than a few pixels, ignoring pixels that already changed
//! while idle just before (a blinking caret, a clock).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Args;
use cua_sdk::{CuaError, MediaEvent, MediaOpenOptions, MediaSession, SpacesdClient};
use serde_json::{Value, json};

use crate::util::internal;

/// `cua sb stream-probe`.
#[derive(Args, Debug, Clone)]
pub struct StreamProbeArgs {
    /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
    pub name: String,
    /// Stream this window (an id from `cua do window ls`) instead of the
    /// primary display.
    #[arg(long, conflicts_with = "window_app")]
    pub window: Option<String>,
    /// Stream the first window whose app or title contains this (as
    /// `cua do window ls APP` lists them), focused windows first.
    #[arg(long)]
    pub window_app: Option<String>,
    /// Input policy: `allow_activation` (the desktop default),
    /// `background_only` (the window default; reopened with
    /// `allow_activation` when the Space refuses background input, as the
    /// viewers do) or `view_only`.
    #[arg(long)]
    pub policy: Option<String>,
    /// Frame-rate cap requested from the Space.
    #[arg(long, default_value_t = 30)]
    pub max_fps: u32,
    /// Long-edge cap in pixels (0: native).
    #[arg(long, default_value_t = 0)]
    pub max_dimension: u32,
    /// Seconds to count frames for the frame rate.
    #[arg(long, default_value_t = 5.0)]
    pub seconds: f64,
    /// Ping round trips on the media socket.
    #[arg(long, default_value_t = 10)]
    pub pings: u32,
    /// Input to send through the session, in order (repeatable):
    /// `click:X,Y`, `type:TEXT`, `key:NAME[+command|+shift|+control|+option]`,
    /// `scroll:X,Y,DY` or `wait:MS`. X and Y are normalized to the stream
    /// (0 to 1); DY is in wheel clicks, positive scrolls down.
    #[arg(long = "action", value_name = "ACTION")]
    pub actions: Vec<String>,
    /// Seconds to wait for each action's acknowledgement and change.
    #[arg(long, default_value_t = 30.0)]
    pub action_timeout: f64,
    /// Save the last frame here as PNG (needs the Cua Spaces `cua`).
    #[arg(long)]
    pub save_frame: Option<PathBuf>,
    /// Report the agent participants on the Space's presence before and
    /// after the actions (`presence`): a viewer's input must add none, only
    /// the viewer's own cursor.
    #[arg(long)]
    pub presence_check: bool,
}

/// One input action.
/// One `--action` of the probe.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Click(f64, f64),
    Type(String),
    Key(String, Vec<&'static str>),
    Scroll(f64, f64, f64),
    Wait(u64),
}

#[doc(hidden)]
pub fn parse_action(spec: &str) -> Result<Action, CuaError> {
    let bad = || CuaError::InvalidArgument(format!("bad --action {spec:?}"));
    let (kind, rest) = spec.split_once(':').ok_or_else(bad)?;
    let nums = |n: usize| -> Result<Vec<f64>, CuaError> {
        let v: Vec<f64> = rest
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        if v.len() != n || v.iter().any(|x| !x.is_finite()) {
            return Err(bad());
        }
        Ok(v)
    };
    let unit = |x: f64| (0.0..=1.0).contains(&x);
    Ok(match kind {
        "click" => {
            let v = nums(2)?;
            if !unit(v[0]) || !unit(v[1]) {
                return Err(bad());
            }
            Action::Click(v[0], v[1])
        }
        "scroll" => {
            let v = nums(3)?;
            if !unit(v[0]) || !unit(v[1]) {
                return Err(bad());
            }
            Action::Scroll(v[0], v[1], v[2])
        }
        "type" if !rest.is_empty() => Action::Type(rest.to_string()),
        "key" => {
            let mut parts = rest.split('+');
            let key = parts.next().filter(|k| !k.is_empty()).ok_or_else(bad)?;
            let mut mods = Vec::new();
            for m in parts {
                mods.push(match m {
                    "command" | "cmd" | "super" | "meta" => "command",
                    "shift" => "shift",
                    "control" | "ctrl" => "control",
                    "option" | "alt" => "option",
                    _ => return Err(bad()),
                });
            }
            Action::Key(key.to_string(), mods)
        }
        "wait" => Action::Wait(rest.parse().map_err(|_| bad())?),
        _ => return Err(bad()),
    })
}

/// The `interactive_input` events of an action (none for `wait`).
#[doc(hidden)]
pub fn events_of(action: &Action) -> Vec<Value> {
    let pointer = |phase: &str, button: Option<&str>, x: f64, y: f64| {
        json!({"kind": "pointer", "phase": phase, "button": button,
               "x_normalized": x, "y_normalized": y, "modifiers": []})
    };
    match action {
        Action::Click(x, y) => vec![
            pointer("move", None, *x, *y),
            pointer("down", Some("left"), *x, *y),
            pointer("up", Some("left"), *x, *y),
        ],
        Action::Type(text) => vec![json!({"kind": "text_commit", "text": text})],
        Action::Key(key, mods) => ["down", "up"]
            .iter()
            .map(|state| {
                json!({"kind": "key", "key": key, "state": state,
                       "modifiers": mods, "repeat": false})
            })
            .collect(),
        Action::Scroll(x, y, dy) => vec![json!({
            "kind": "scroll", "x_normalized": x, "y_normalized": y,
            "delta_x": 0.0, "delta_y": dy, "phase": "none",
            "momentum_phase": "none", "precise": false
        })],
        Action::Wait(_) => Vec::new(),
    }
}

/// A frame as the probe keeps it: when it arrived and, when decoded, its
/// BGRA pixels.
#[derive(Clone)]
struct Frame {
    at: Instant,
    width: u32,
    height: u32,
    pixels: Option<Arc<Vec<u8>>>,
}

#[derive(Default)]
struct Shared {
    frames: Vec<(Instant, u32, u32)>,
    last: Option<Frame>,
    events: Vec<(Instant, Value)>,
    pongs: HashMap<u64, Instant>,
    /// Pixels that changed between frames while `noise` is being recorded.
    noise: Option<Noise>,
    /// The visible-change watch of the action in flight.
    watch: Option<Watch>,
    decode_errors: u64,
    /// The socket's close, when it closed.
    closed: Option<Value>,
}

struct Noise {
    width: u32,
    height: u32,
    mask: Vec<bool>,
    previous: Arc<Vec<u8>>,
}

struct Watch {
    since: Instant,
    reference: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    mask: Option<Vec<bool>>,
    changed_at: Option<Instant>,
    changed_pixels: usize,
}

/// Pixels whose largest channel difference exceeds this count as changed.
const PIXEL_DELTA: i32 = 40;
/// A frame with more changed pixels than this (outside the noise mask) is a
/// visible change.
const MIN_CHANGED_PIXELS: usize = 12;

fn changed(a: &[u8], b: &[u8], mask: Option<&[bool]>, mut each: impl FnMut(usize)) -> usize {
    let mut count = 0;
    for (i, (pa, pb)) in a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .enumerate()
    {
        if mask.is_some_and(|m| m.get(i).copied().unwrap_or(false)) {
            continue;
        }
        let d = (0..3)
            .map(|c| (i32::from(pa[c]) - i32::from(pb[c])).abs())
            .max()
            .unwrap_or(0);
        if d > PIXEL_DELTA {
            count += 1;
            each(i);
        }
    }
    count
}

impl Shared {
    fn frame(&mut self, frame: Frame) {
        self.frames.push((frame.at, frame.width, frame.height));
        if let Some(pixels) = &frame.pixels {
            if let Some(n) = &mut self.noise {
                if n.width == frame.width && n.height == frame.height {
                    let mask = &mut n.mask;
                    changed(&n.previous, pixels, None, |i| mask[i] = true);
                }
                n.previous = pixels.clone();
                n.width = frame.width;
                n.height = frame.height;
                if n.mask.len() != (frame.width * frame.height) as usize {
                    n.mask = vec![false; (frame.width * frame.height) as usize];
                }
            }
            if let Some(w) = &mut self.watch
                && w.changed_at.is_none()
                && frame.at >= w.since
            {
                let resized = w.width != frame.width || w.height != frame.height;
                let n = if resized {
                    usize::MAX
                } else {
                    changed(&w.reference, pixels, w.mask.as_deref(), |_| {})
                };
                if n > MIN_CHANGED_PIXELS {
                    w.changed_at = Some(frame.at);
                    w.changed_pixels = n;
                }
            }
        }
        self.last = Some(frame);
    }

    fn event(&mut self, event: MediaEvent) {
        let now = Instant::now();
        let v: Value = serde_json::from_str(&event.json).unwrap_or(Value::Null);
        if event.kind == "pong"
            && let Some(nonce) = v["payload"]["nonce"].as_u64()
        {
            self.pongs.insert(nonce, now);
        }
        if event.kind == "decode_error" {
            self.decode_errors += 1;
        }
        if event.kind == "closed" {
            self.closed = Some(v.clone());
        }
        self.events.push((now, v));
    }
}

struct Sink(Arc<Mutex<Shared>>);

impl cua_sdk::FrameSink for Sink {
    fn on_frame(&self, frame: cua_sdk::VideoFrame) {
        let f = Frame {
            at: Instant::now(),
            width: frame.width,
            height: frame.height,
            pixels: None,
        };
        self.0.lock().unwrap().frame(f);
    }
    fn on_event(&self, event: MediaEvent) {
        self.0.lock().unwrap().event(event);
    }
}

impl crate::extension::DecodedFrames for Sink {
    fn frame(&self, width: u32, height: u32, bgra: Vec<u8>) {
        let f = Frame {
            at: Instant::now(),
            width,
            height,
            pixels: Some(Arc::new(bgra)),
        };
        self.0.lock().unwrap().frame(f);
    }
    fn event(&self, event: MediaEvent) {
        self.0.lock().unwrap().event(event);
    }
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn policy_enum(policy: &str) -> Result<&'static str, CuaError> {
    Ok(match policy {
        "allow_activation" => "SESSION_POLICY_ALLOW_ACTIVATION",
        "background_only" => "SESSION_POLICY_BACKGROUND_ONLY",
        "view_only" => "SESSION_POLICY_VIEW_ONLY",
        other => {
            return Err(CuaError::InvalidArgument(format!(
                "--policy must be allow_activation, background_only or view_only (got {other:?})"
            )));
        }
    })
}

struct Open {
    session: Arc<MediaSession>,
    shared: Arc<Mutex<Shared>>,
    open_ms: f64,
    started: Instant,
}

async fn open(
    env: &Arc<SpacesdClient>,
    a: &StreamProbeArgs,
    policy: &str,
) -> Result<Open, CuaError> {
    let options = MediaOpenOptions {
        display: None,
        window_handle: a.window.clone(),
        max_fps: a.max_fps,
        max_dimension: a.max_dimension,
        audio: false,
        disable_video: false,
        request_json: Some(json!({"policy": policy_enum(policy)?}).to_string()),
    };
    let shared = Arc::new(Mutex::new(Shared::default()));
    let sink = Arc::new(Sink(shared.clone()));
    let started = Instant::now();
    // The Cua Spaces `cua` decodes (the decoder ships with Cua Spaces);
    // this build alone reports acknowledgements and frame timing.
    let session = match crate::extension::get() {
        Some(ext) => ext.open_media_decoded(env.clone(), options, sink).await?,
        None => env.open_media(options, sink).await?,
    };
    Ok(Open {
        session,
        shared,
        open_ms: ms(started.elapsed()),
        started,
    })
}

async fn until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The acknowledgement (or error) covering input sequence `through`, sent at
/// or after `since`.
fn ack_for(shared: &Shared, since: Instant, through: u64) -> Option<(Instant, Value)> {
    shared.events.iter().find_map(|(at, v)| {
        let kind = v["type"].as_str().unwrap_or("");
        let p = &v["payload"];
        let covers = kind == "interactive_input_acknowledgement"
            && p["through_sequence"].as_u64().is_some_and(|s| s >= through);
        (*at >= since && (covers || kind == "error")).then(|| (*at, v.clone()))
    })
}

/// How many control messages of each type arrived.
fn kinds(events: &[(Instant, Value)]) -> Value {
    let mut counts = serde_json::Map::new();
    for (_, v) in events {
        let kind = v["type"].as_str().unwrap_or("closed").to_string();
        let n = counts.get(&kind).and_then(Value::as_u64).unwrap_or(0);
        counts.insert(kind, (n + 1).into());
    }
    Value::Object(counts)
}

/// The principal ids of the agents present on the Space's presence now,
/// from the roster a short-lived join receives.
async fn presence_agents(env: &SpacesdClient) -> Result<Vec<String>, CuaError> {
    let request = json!({"principal": {
        "id": "cua-stream-probe",
        "displayName": "cua stream-probe",
        "kind": "PRINCIPAL_KIND_HUMAN",
    }});
    let messages = env
        .call_json_stream(
            "/cua.env.v1.PresenceService/Join".into(),
            request.to_string(),
            1,
            10_000,
        )
        .await?;
    let joined: Value = serde_json::from_str(
        messages
            .first()
            .ok_or_else(|| CuaError::Timeout("no presence roster within 10 s".into()))?,
    )
    .map_err(internal)?;
    Ok(agents_in(&joined))
}

/// Agent principal ids in a `JoinResponse` (proto3 JSON) `joined` roster.
fn agents_in(joined: &Value) -> Vec<String> {
    let mut agents: Vec<String> = joined["joined"]["roster"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| &member["participant"]["principal"])
        .filter(|principal| principal["kind"] == "PRINCIPAL_KIND_AGENT" || principal["kind"] == 2)
        .filter_map(|principal| principal["id"].as_str().map(str::to_owned))
        .collect();
    agents.sort();
    agents
}

/// Runs the probe and returns its report.
pub async fn run(env: Arc<SpacesdClient>, a: &StreamProbeArgs) -> Result<Value, CuaError> {
    let resolved;
    let a = if let Some(app) = &a.window_app {
        let mut windows = crate::computer::Computer::new(env.clone())
            .windows(app)
            .await?;
        windows.sort_by_key(|w| !w.focused);
        let w = windows
            .first()
            .ok_or_else(|| CuaError::NotFound(format!("no window matches {app:?}")))?;
        resolved = StreamProbeArgs {
            window: Some(crate::computer::window_id(w)),
            window_app: None,
            ..a.clone()
        };
        &resolved
    } else {
        a
    };
    let actions = a
        .actions
        .iter()
        .map(|s| parse_action(s))
        .collect::<Result<Vec<_>, _>>()?;
    let decoded = crate::extension::get().is_some();
    if a.save_frame.is_some() && !decoded {
        return Err(CuaError::Unsupported(
            "--save-frame needs the decoder, which ships with Cua Spaces (source-available, FSL-1.1-MIT); run the Cua Spaces `cua`".into(),
        ));
    }
    let mut policy = a.policy.clone().unwrap_or_else(|| {
        if a.window.is_some() {
            "background_only".into()
        } else {
            "allow_activation".into()
        }
    });
    let agents_before = if a.presence_check {
        Some(presence_agents(&env).await?)
    } else {
        None
    };
    let mut o = open(&env, a, &policy).await?;
    let first = until(Duration::from_secs(60), || {
        o.shared.lock().unwrap().frames.first().copied()
    })
    .await
    .ok_or_else(|| CuaError::Timeout("no video frame within 60 s".into()))?;
    let first_frame_ms = ms(first.0 - o.started);

    // Frame rate.
    let from = Instant::now();
    tokio::time::sleep(Duration::from_secs_f64(a.seconds.max(0.5))).await;
    let (frames_counted, size) = {
        let s = o.shared.lock().unwrap();
        let n = s.frames.iter().filter(|f| f.0 >= from).count();
        let size = s.frames.last().map(|f| (f.1, f.2)).unwrap_or((0, 0));
        (n, size)
    };
    let fps = frames_counted as f64 / from.elapsed().as_secs_f64();

    // Media-socket round trips.
    let mut rtts = Vec::new();
    for nonce in 1..=u64::from(a.pings) {
        let sent = Instant::now();
        o.session
            .send_control(json!({"type": "ping", "payload": {"nonce": nonce}}).to_string())?;
        let shared = o.shared.clone();
        if let Some(at) = until(Duration::from_secs(10), || {
            shared.lock().unwrap().pongs.get(&nonce).copied()
        })
        .await
        {
            rtts.push(ms(at - sent));
        }
    }
    let mut sorted = rtts.clone();
    sorted.sort_by(f64::total_cmp);
    let ping = json!({
        "sent": a.pings,
        "answered": rtts.len(),
        "min_ms": sorted.first(),
        "median_ms": sorted.get(sorted.len() / 2),
        "max_ms": sorted.last(),
    });

    // Input.
    let timeout = Duration::from_secs_f64(a.action_timeout.max(1.0));
    let mut next_sequence = 1u64;
    let mut reopened = false;
    let mut results = Vec::new();
    for (spec, action) in a.actions.iter().zip(&actions) {
        if let Action::Wait(ms_) = action {
            tokio::time::sleep(Duration::from_millis(*ms_)).await;
            results.push(json!({"action": spec}));
            continue;
        }
        let mut attempt = 0;
        loop {
            attempt += 1;
            // What the picture looked like before, and what changes by itself.
            if decoded {
                {
                    let mut s = o.shared.lock().unwrap();
                    s.noise = s.last.clone().and_then(|prev| {
                        prev.pixels.map(|px| Noise {
                            width: prev.width,
                            height: prev.height,
                            mask: vec![false; (prev.width * prev.height) as usize],
                            previous: px,
                        })
                    });
                }
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
            let events = events_of(action);
            let through = next_sequence + events.len() as u64 - 1;
            let batch = json!({"type": "interactive_input", "payload": {
                "session_id": "", "first_sequence": next_sequence, "events": events}});
            next_sequence = through + 1;
            let sent = {
                let mut s = o.shared.lock().unwrap();
                let mask = s.noise.take().map(|n| n.mask);
                s.watch = s.last.as_ref().and_then(|f| {
                    f.pixels.clone().map(|px| Watch {
                        since: Instant::now(),
                        reference: px,
                        width: f.width,
                        height: f.height,
                        mask,
                        changed_at: None,
                        changed_pixels: 0,
                    })
                });
                Instant::now()
            };
            o.session.send_control(batch.to_string())?;
            let shared = o.shared.clone();
            let ack = until(timeout, || ack_for(&shared.lock().unwrap(), sent, through)).await;
            let refused_activation = ack.as_ref().is_some_and(|(_, v)| {
                v["payload"]["delivered"] == false
                    && v["payload"]["error"]["code"] == "would_require_activation"
            });
            if refused_activation && a.window.is_some() && !reopened && attempt == 1 {
                // As the viewers do: reopen the window activating and resend.
                reopened = true;
                policy = "allow_activation".into();
                let _ = o.session.close().await;
                o = open(&env, a, &policy).await?;
                let shared = o.shared.clone();
                until(Duration::from_secs(60), || {
                    shared.lock().unwrap().frames.first().copied()
                })
                .await
                .ok_or_else(|| CuaError::Timeout("no frame after reopening".into()))?;
                next_sequence = 1;
                continue;
            }
            let mut r = json!({"action": spec});
            match &ack {
                Some((at, v)) => {
                    let p = &v["payload"];
                    r["ack_ms"] = ms(*at - sent).into();
                    r["delivered"] = p["delivered"].as_bool().unwrap_or(false).into();
                    if !p["error"].is_null() {
                        r["error"] = p["error"].clone();
                    }
                    if v["type"] == "error" {
                        r["error"] = p.clone();
                    }
                    if let Some(us) = p["host_dispatch_us"].as_u64() {
                        r["host_dispatch_ms"] = (us as f64 / 1000.0).into();
                    }
                }
                None => {
                    r["delivered"] = false.into();
                    r["error"] = json!({"message": "no acknowledgement"});
                }
            }
            let shared = o.shared.clone();
            let ack_at = ack.as_ref().map(|a| a.0).unwrap_or(sent);
            if let Some(at) = until(Duration::from_secs(5), || {
                let s = shared.lock().unwrap();
                s.frames.iter().find(|f| f.0 > ack_at).map(|f| f.0)
            })
            .await
            {
                r["first_frame_after_ack_ms"] = ms(at - sent).into();
            }
            if decoded {
                let shared = o.shared.clone();
                match until(timeout, || {
                    let s = shared.lock().unwrap();
                    let w = s.watch.as_ref()?;
                    w.changed_at.map(|at| (at, w.changed_pixels))
                })
                .await
                {
                    Some((at, n)) => {
                        r["visible_ms"] = ms(at - sent).into();
                        r["changed_pixels"] = n.min(u32::MAX as usize).into();
                    }
                    None => r["visible_ms"] = Value::Null,
                }
                o.shared.lock().unwrap().watch = None;
            }
            results.push(r);
            break;
        }
    }

    let saved = if let Some(path) = &a.save_frame {
        let last = o.shared.lock().unwrap().last.clone();
        match last.and_then(|f| f.pixels.map(|p| (f.width, f.height, p))) {
            Some((w, h, px)) => {
                let rgba: Vec<u8> = px
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[2], p[1], p[0], 255])
                    .collect();
                let img = image::RgbaImage::from_raw(w, h, rgba)
                    .ok_or_else(|| internal("frame size mismatch"))?;
                img.save(path).map_err(internal)?;
                Some(path.display().to_string())
            }
            None => None,
        }
    } else {
        None
    };

    let stats = o.session.stats();
    let (errors, closed, decode_errors, event_kinds) = {
        let s = o.shared.lock().unwrap();
        let errors: Vec<Value> = s
            .events
            .iter()
            .filter(|(_, v)| v["type"] == "error" || v["type"] == "decode_error")
            .map(|(_, v)| v.clone())
            .collect();
        (errors, s.closed.clone(), s.decode_errors, kinds(&s.events))
    };
    let presence = match agents_before {
        Some(before) => {
            let after = presence_agents(&env).await?;
            let new: Vec<&String> = after.iter().filter(|id| !before.contains(id)).collect();
            json!({"agents_before": before, "agents_after": after, "new_agents": new})
        }
        None => Value::Null,
    };
    let codec = o.session.codec();
    let _ = o.session.close().await;
    Ok(json!({
        "target": a.window.as_ref().map_or("display:primary".to_string(), |w| format!("window:{w}")),
        "policy": policy,
        "reopened_activating": reopened,
        "codec": codec,
        "decoded": decoded,
        "open_ms": o.open_ms,
        "first_frame_ms": first_frame_ms,
        "width": size.0,
        "height": size.1,
        "fps": (fps * 10.0).round() / 10.0,
        "fps_window_s": a.seconds.max(0.5),
        "frames": stats.frames,
        "frames_dropped": stats.frames_dropped,
        "ping": ping,
        "actions": results,
        "errors": errors,
        "decode_errors": decode_errors,
        "events": event_kinds,
        "closed_early": closed,
        "saved_frame": saved,
        "presence": presence,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_parse_and_become_protocol_events() {
        assert_eq!(
            parse_action("click:0.5,0.25").unwrap(),
            Action::Click(0.5, 0.25)
        );
        assert!(parse_action("click:1.5,0.2").is_err());
        assert_eq!(
            parse_action("key:enter+command+shift").unwrap(),
            Action::Key("enter".into(), vec!["command", "shift"])
        );
        assert!(parse_action("key:a+hyper").is_err());
        assert_eq!(
            parse_action("type:ls -la").unwrap(),
            Action::Type("ls -la".into())
        );
        assert_eq!(
            parse_action("scroll:0.5,0.5,-3").unwrap(),
            Action::Scroll(0.5, 0.5, -3.0)
        );
        assert_eq!(parse_action("wait:250").unwrap(), Action::Wait(250));
        assert!(parse_action("type:").is_err());
        assert!(parse_action("tap:1").is_err());

        // Every non-wait action sends input (that the Space accepts each
        // batch is checked against the protocol in cua-spaces-cli's
        // stream_probe_wire test).
        for spec in [
            "click:0.1,0.9",
            "type:hi",
            "key:enter+command",
            "scroll:0.5,0.5,3",
        ] {
            assert!(
                !events_of(&parse_action(spec).unwrap()).is_empty(),
                "{spec}"
            );
        }
    }

    #[test]
    fn presence_agents_are_read_from_the_roster() {
        let joined = json!({"joined": {"participant": {"participantId": "p-me"}, "roster": [
            {"participant": {"participantId": "p-1", "principal": {"id": "user:dana", "kind": "PRINCIPAL_KIND_HUMAN"}}},
            {"participant": {"participantId": "a-2", "principal": {"id": "cua-driver:run-2", "kind": "PRINCIPAL_KIND_AGENT"}}},
            {"participant": {"participantId": "a-1", "principal": {"id": "agent-1", "kind": 2}}},
        ]}});
        assert_eq!(agents_in(&joined), vec!["agent-1", "cua-driver:run-2"]);
        assert!(agents_in(&json!({"joined": {}})).is_empty());
    }

    #[test]
    fn visible_change_ignores_idle_noise() {
        let a = vec![0u8; 4 * 100];
        let mut b = a.clone();
        for p in b.as_chunks_mut::<4>().0.iter_mut().take(20) {
            p[1] = 200;
        }
        assert_eq!(changed(&a, &b, None, |_| {}), 20);
        let mut mask = vec![false; 100];
        mask[..15].iter_mut().for_each(|m| *m = true);
        assert_eq!(changed(&a, &b, Some(&mask), |_| {}), 5);
    }
}
