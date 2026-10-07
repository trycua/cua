//! Streaming example on the cua SDK (Rust host of the `cua-sdk` crate).
//!
//! Runs the shared scenario in `examples/streaming/SCENARIO.md`: connect,
//! start the grid fixture, list targets, stream the desktop and the grid
//! window (decoded BGRA + PCM), click a grid cell over the media socket,
//! verify it, and print a `SUMMARY` line. With `CUA_BENCH_JSONL` set it runs
//! the benchmark lane instead (one target, per-frame JSONL).
//!
//! Headless only: rendering lives in `cua-viewer` (libs/cua/crates/cua-viewer),
//! which uses the same media plane.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cua_sdk::{
    Cua, CuaConfig, DecodedFrameSink, DecodedVideoFrame, SpacesdClient, MediaEvent, MediaOpenOptions,
    MediaSession, PcmAudio, PcmSink,
};
use serde_json::{json, Value};

type Error = Box<dyn std::error::Error + Send + Sync>;

const GRID_TITLE: &str = "CUA Fixture Grid";
const CLICK_CONTENT_PX: (f64, f64) = (200.0, 280.0);
const CELL_RGB: [i32; 3] = [72, 153, 128];
/// Hard bound on WAV size per stream (memory safety): 60 s of 48 kHz stereo.
const MAX_PCM_SAMPLES: usize = 48_000 * 2 * 60;

fn unix_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn cpu_s() -> (f64, f64) {
    // SAFETY: getrusage fills the zeroed struct we own.
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        (tv(u.ru_utime), tv(u.ru_stime))
    }
}

// ------------------------------------------------------------------ pixels

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn rgb_at(frame: &DecodedVideoFrame, x: u32, y: u32) -> Option<[i32; 3]> {
    if x >= frame.width || y >= frame.height {
        return None;
    }
    let i = (y * frame.stride + x * 4) as usize;
    let p = frame.data.get(i..i + 4)?;
    Some([i32::from(p[2]), i32::from(p[1]), i32::from(p[0])])
}

fn cell_bit(frame: &DecodedVideoFrame, ox: u32, oy: u32, index: u32) -> Option<u32> {
    let mut sum = 0i32;
    for dy in 6..10 {
        for dx in 6..10 {
            let rgb = rgb_at(frame, ox + index * 16 + dx, oy + dy)?;
            sum += (rgb[0] + rgb[1] + rgb[2]) / 3;
        }
    }
    Some(u32::from(sum / 16 > 128))
}

/// The bench timecode (SCENARIO.md "Bench timecode") at a known origin.
fn read_timecode_at(frame: &DecodedVideoFrame, ox: u32, oy: u32) -> Option<u32> {
    for (i, want) in [1, 0, 1, 0].iter().enumerate() {
        if cell_bit(frame, ox, oy, i as u32)? != *want {
            return None;
        }
    }
    for (i, want) in [0, 1, 0, 1].iter().enumerate() {
        if cell_bit(frame, ox, oy, 44 + i as u32)? != *want {
            return None;
        }
    }
    let mut value = 0u32;
    for i in 0..32 {
        value = (value << 1) | cell_bit(frame, ox, oy, 4 + i)?;
    }
    let mut check = 0u32;
    for i in 0..8 {
        check = (check << 1) | cell_bit(frame, ox, oy, 36 + i)?;
    }
    let b = value.to_be_bytes();
    (u32::from(b[0] ^ b[1] ^ b[2] ^ b[3]) == check).then_some(value)
}

/// Window frames: the strip is at (0, 0); otherwise scan (bounded).
fn read_timecode(frame: &DecodedVideoFrame, origin: &mut Option<(u32, u32)>) -> Option<u32> {
    if let Some((x, y)) = *origin {
        if let Some(v) = read_timecode_at(frame, x, y) {
            return Some(v);
        }
    }
    if frame.width < 768 || frame.height < 16 {
        return None;
    }
    for y in (0..=frame.height - 16).step_by(2).take(2000) {
        for x in (0..=frame.width - 768).step_by(2) {
            if cell_bit(frame, x, y, 0) == Some(1) && cell_bit(frame, x, y, 1) == Some(0) {
                if let Some(v) = read_timecode_at(frame, x, y) {
                    *origin = Some((x, y));
                    return Some(v);
                }
            }
        }
    }
    None
}

fn unwrap_ms(low: u32, now_ms: i64) -> i64 {
    let base = now_ms & !0xFFFF_FFFF;
    let mut best = base | i64::from(low);
    for c in [best - (1 << 32), best + (1 << 32)] {
        if (c - now_ms).abs() < (best - now_ms).abs() {
            best = c;
        }
    }
    best
}

fn write_wav(path: &Path, samples: &[i16], rate: u32, channels: u16) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * u32::from(channels) * 2).to_le_bytes())?;
    f.write_all(&(channels * 2).to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

// ------------------------------------------------------------------- sinks

#[derive(Default)]
struct State {
    frames: u64,
    first_frame_at: Option<Instant>,
    last: Option<DecodedVideoFrame>,
    last_hash: u64,
    geometry_epoch: u64,
    audio_packets: u64,
    pcm: Vec<i16>,
    rate: u32,
    channels: u16,
    events: Vec<String>,
    input_acks: u64,
    tc_origin: Option<(u32, u32)>,
    jsonl: Option<std::fs::File>,
}

struct Sink(Mutex<State>);

impl Sink {
    fn new(jsonl: Option<std::fs::File>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            jsonl,
            ..State::default()
        })))
    }
}

impl DecodedFrameSink for Sink {
    fn on_decoded_frame(&self, frame: DecodedVideoFrame) {
        let now = unix_ns();
        let mut s = self.0.lock().unwrap();
        s.frames += 1;
        s.first_frame_at.get_or_insert_with(Instant::now);
        if s.jsonl.is_some() {
            let mut origin = s.tc_origin;
            let tc = read_timecode(&frame, &mut origin);
            s.tc_origin = origin;
            let line = json!({"t":"frame","unix_ns":now,"seq":frame.sequence,"bytes":null,"key":null,
                "cap_us":frame.capture_timestamp_us,"w":frame.width,"h":frame.height,
                "tc_ms": tc.map(|v| unwrap_ms(v, now / 1_000_000))});
            if let Some(f) = s.jsonl.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        }
        s.geometry_epoch = frame.geometry_epoch;
        s.last = Some(frame);
    }

    fn on_event(&self, event: MediaEvent) {
        let mut s = self.0.lock().unwrap();
        if event.kind == "interactive_input_acknowledgement" {
            s.input_acks += 1;
        }
        if s.events.len() < 256 {
            s.events.push(event.kind.clone());
        }
        if event.kind == "session_opened" {
            if let Some(e) = serde_json::from_str::<Value>(&event.json)
                .ok()
                .and_then(|v| v.pointer("/payload/geometry_epoch").and_then(Value::as_u64))
            {
                s.geometry_epoch = e;
            }
        }
    }
}

impl PcmSink for Sink {
    fn on_pcm(&self, audio: PcmAudio) {
        let now = unix_ns();
        let mut s = self.0.lock().unwrap();
        s.audio_packets += 1;
        s.rate = audio.sample_rate;
        s.channels = audio.channels;
        if s.pcm.len() + audio.samples.len() <= MAX_PCM_SAMPLES {
            s.pcm.extend_from_slice(&audio.samples);
        }
        if s.jsonl.is_some() {
            let line = json!({"t":"audio","unix_ns":now,"pts_us":audio.pts_us,"bytes":null,
                "samples": audio.samples.len() / usize::from(audio.channels.max(1))});
            if let Some(f) = s.jsonl.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

// --------------------------------------------------------------- scenario

struct Target {
    kind: &'static str,
    id: String,
    title: String,
    width: f64,
    height: f64,
}

async fn list_targets(env: &SpacesdClient) -> Result<Vec<Target>, Error> {
    let raw = env
        .call_json(
            "/cua.env.v1.StreamService/ListTargets".into(),
            json!({"includeWindows": true}).to_string(),
        )
        .await?;
    let v: Value = serde_json::from_str(&raw)?;
    let mut out = Vec::new();
    for t in v
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(d) = t.get("display") {
            let size = d.get("nativeSize").cloned().unwrap_or_default();
            out.push(Target {
                kind: "display",
                id: d
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                title: d
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                width: size.get("width").and_then(Value::as_f64).unwrap_or(0.0),
                height: size.get("height").and_then(Value::as_f64).unwrap_or(0.0),
            });
        } else if let Some(w) = t.get("window") {
            let r = w.get("ref").cloned().unwrap_or_default();
            let b = w.get("bounds").cloned().unwrap_or_default();
            let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            out.push(Target {
                kind: "window",
                id: r
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                title: w
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                width: num(&b, "width"),
                height: num(&b, "height"),
            });
        }
    }
    Ok(out)
}

fn options(target: &Target, audio: bool) -> MediaOpenOptions {
    let window = target.kind == "window";
    // Input needs a policy other than view-only; the escape hatch sets it.
    let request =
        json!({"policy": "SESSION_POLICY_ALLOW_ACTIVATION", "codecs": ["MEDIA_CODEC_H264"]});
    MediaOpenOptions {
        display: (!window).then(|| target.id.clone()),
        window_handle: window.then(|| target.id.clone()),
        max_fps: 30,
        max_dimension: 0,
        audio,
        disable_video: false,
        request_json: Some(request.to_string()),
    }
}

struct StreamReport {
    summary: Value,
    session: Arc<MediaSession>,
    sink: Arc<Sink>,
}

async fn stream(
    env: &SpacesdClient,
    target: &Target,
    seconds: f64,
    out: &Path,
    name: &str,
    jsonl: Option<std::fs::File>,
) -> Result<StreamReport, Error> {
    let sink = Sink::new(jsonl);
    let opened = Instant::now();
    let session = env
        .open_media_decoded_with_audio(options(target, true), sink.clone(), sink.clone())
        .await?;
    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
    let s = sink.0.lock().unwrap();
    let wav = out.join(format!("{name}.wav"));
    if s.rate > 0 {
        write_wav(&wav, &s.pcm, s.rate, s.channels)?;
    }
    let last_hash = s
        .last
        .as_ref()
        .map(|f| fnv1a(&f.data))
        .unwrap_or(s.last_hash);
    let summary = json!({
        "frames": s.frames,
        "keyframes": null,
        "bytes": null,
        "first_frame_ms": s.first_frame_at.map(|t| (t.duration_since(opened).as_secs_f64() * 1e4).round() / 10.0),
        "fps": ((s.frames as f64 / seconds) * 10.0).round() / 10.0,
        "audio_packets": s.audio_packets,
        "last_hash": format!("{last_hash:016x}"),
        "hash_of": "bgra",
        "size": s.last.as_ref().map(|f| [f.width, f.height]),
        "wav": if s.rate > 0 { Value::from(wav.display().to_string()) } else { Value::Null },
    });
    drop(s);
    Ok(StreamReport {
        summary,
        session,
        sink,
    })
}

async fn grid_log(env: &SpacesdClient) -> Vec<Value> {
    match env
        .sh(
            "cat /tmp/cua-fixtures/grid.jsonl 2>/dev/null || true".into(),
            Some(10_000),
        )
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Click grid cell (2, 3) through the media socket (interactive pointer
/// input), then check the fixture log and the decoded pixel.
async fn click(env: &SpacesdClient, report: &StreamReport, window: &Target) -> Value {
    let before = grid_log(env).await.len();
    let (fw, fh) = {
        let s = report.sink.0.lock().unwrap();
        s.last
            .as_ref()
            .map(|f| (f64::from(f.width), f64::from(f.height)))
            .unwrap_or((window.width, window.height))
    };
    let scale = if window.width > 0.0 {
        fw / window.width
    } else {
        1.0
    };
    let (x, y) = (CLICK_CONTENT_PX.0 * scale, CLICK_CONTENT_PX.1 * scale);
    let pointer = |phase: &str| {
        json!({"kind":"pointer","phase":phase,"button":"left",
        "x_normalized": x / fw, "y_normalized": y / fh, "modifiers": []})
    };
    let batch = json!({"type":"interactive_input","payload":{
        "session_id": report.session.session_id(), "first_sequence": 1,
        "events": [pointer("move"), pointer("down"), pointer("up")]}});
    let sent = report.session.send_control(batch.to_string()).is_ok();
    let mut logged = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let lines = grid_log(env).await;
        if lines.iter().skip(before).any(|l| {
            l.get("type").and_then(Value::as_str) == Some("button_press")
                && l.get("cell") == Some(&json!([2, 3]))
        }) {
            logged = true;
            break;
        }
    }
    let pixel = {
        let s = report.sink.0.lock().unwrap();
        s.last.as_ref().and_then(|f| rgb_at(f, x as u32, y as u32))
    };
    let pixel_ok = pixel.is_some_and(|p| p.iter().zip(CELL_RGB).all(|(a, b)| (a - b).abs() <= 24));
    json!({"sent": sent, "via": "interactive_input", "logged": logged, "pixel_ok": pixel_ok,
        "frame_point": [x.round(), y.round()], "pixel": pixel})
}

async fn scenario(env: &SpacesdClient) -> Result<bool, Error> {
    let seconds: f64 = std::env::var("CUA_STREAM_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5.0);
    let out = PathBuf::from(std::env::var("CUA_OUT_DIR").unwrap_or_else(|_| "./out".into()));
    std::fs::create_dir_all(&out)?;
    println!("health {}", env.health().await?);
    let started = env
        .sh("cua-fixtures start grid".into(), Some(30_000))
        .await?;
    println!("{}", String::from_utf8_lossy(&started.stdout).trim());

    let mut targets = Vec::new();
    for _ in 0..20 {
        targets = list_targets(env).await?;
        if targets.iter().any(|t| t.title == GRID_TITLE) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    for t in &targets {
        println!(
            "target {:7} {:24} {:32} {}x{}",
            t.kind, t.id, t.title, t.width, t.height
        );
    }
    let display = targets
        .iter()
        .find(|t| t.kind == "display")
        .ok_or("no display target")?;
    let grid = targets
        .iter()
        .find(|t| t.title == GRID_TITLE)
        .ok_or("grid window not listed")?;

    let desktop = stream(env, display, seconds, &out, "desktop", None).await?;
    desktop.session.close().await?;
    let window = stream(env, grid, seconds, &out, "window", None).await?;
    let click = click(env, &window, grid).await;
    window.session.close().await?;
    let ok = desktop.summary["frames"].as_u64().unwrap_or(0) > 0
        && window.summary["frames"].as_u64().unwrap_or(0) > 0
        && click["logged"] == json!(true);
    let summary = json!({"example": "rust", "desktop": desktop.summary, "window": window.summary, "click": click});
    println!("SUMMARY {summary}");
    Ok(ok)
}

async fn bench(env: &SpacesdClient, path: &str) -> Result<bool, Error> {
    let seconds: f64 = std::env::var("CUA_BENCH_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let spec = std::env::var("CUA_BENCH_TARGET").unwrap_or_else(|_| "display:primary".into());
    let audio = std::env::var("CUA_BENCH_AUDIO")
        .map(|v| v != "0")
        .unwrap_or(true);
    let targets = list_targets(env).await?;
    let target = match spec.split_once(':') {
        Some(("window", title)) => targets
            .into_iter()
            .find(|t| t.kind == "window" && t.title == title),
        Some(("display", id)) => targets
            .into_iter()
            .find(|t| t.kind == "display" && (id == "primary" || t.id == id)),
        _ => None,
    }
    .ok_or_else(|| format!("bench target {spec} not found"))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", json!({"t": "open", "unix_ns": unix_ns()}))?;
    let sink = Sink::new(Some(file));
    let mut opts = options(&target, audio);
    opts.audio = audio;
    let session = env
        .open_media_decoded_with_audio(opts, sink.clone(), sink.clone())
        .await?;
    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
    session.close().await?;
    let (user, sys) = cpu_s();
    let mut s = sink.0.lock().unwrap();
    let frames = s.frames;
    if let Some(f) = s.jsonl.as_mut() {
        writeln!(
            f,
            "{}",
            json!({"t": "end", "unix_ns": unix_ns(), "cpu_user_s": user, "cpu_sys_s": sys})
        )?;
    }
    eprintln!("bench: {frames} frames");
    Ok(frames > 0)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let url = std::env::var("CUA_ENV_URL").unwrap_or_else(|_| "http://127.0.0.1:33211".into());
    let Ok(token) = std::env::var("CUA_ENV_TOKEN") else {
        eprintln!("CUA_ENV_TOKEN is required (see examples/streaming/SCENARIO.md)");
        return std::process::ExitCode::from(2);
    };
    let result = async {
        let cua = Cua::embedded(CuaConfig {
            fleet_from_env: false,
            ..CuaConfig::default()
        })?;
        let env = cua.spacesd(url, Some(token)).await?;
        let run = async {
            match std::env::var("CUA_BENCH_JSONL") {
                Ok(path) => bench(&env, &path).await,
                Err(_) => scenario(&env).await,
            }
        };
        tokio::time::timeout(Duration::from_secs(300), run)
            .await
            .map_err(|_| Error::from("scenario timed out"))?
    }
    .await;
    match result {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(1),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(1)
        }
    }
}
