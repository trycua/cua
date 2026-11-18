// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! PulseAudio / PipeWire (pipewire-pulse) capture and uplink on Linux.
//!
//! Audio I/O uses the blocking `pa_simple` API from `libpulse-simple.so.0`,
//! loaded at runtime (no build-time dependency; a host without a sound
//! server reports a clean error). Topology changes (null sinks, virtual
//! sources, moving an app's streams) go through `pactl`, which both
//! PulseAudio and pipewire-pulse ship.
//!
//! - **Desktop mix:** records `@DEFAULT_MONITOR@`, the monitor of the
//!   default sink. In the Spaces image the default sink is a null sink
//!   ([`ensure_desktop_sink`]).
//! - **Per app:** moves the app's sink inputs into a dedicated null sink
//!   `cua_app_<pid>`, loops that sink back to the default sink (so the
//!   desktop mix still contains the app) and records its monitor. If that
//!   fails the capture falls back to the desktop mix and says so.
//! - **Uplink:** a null sink `<name>_sink` plus a remapped source `<name>`
//!   that applications see as a microphone; client audio is written into
//!   the sink ([`PulseUplink`]).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{
    AudioBackend, AudioCapture, AudioCaptureRequest, AudioCaptureState, AudioFormat, AudioFrame,
    AudioFrameSink, AudioSink, AudioSourceInfo, AudioSourceKind, AudioStream, DESKTOP_SOURCE_ID,
};
use crate::backends::dynlib::{resolve, Library, LibraryLoader, SystemLoader};
use crate::error::{CodecError, Result};

const PA_STREAM_PLAYBACK: c_int = 1;
const PA_STREAM_RECORD: c_int = 2;
const PA_SAMPLE_S16LE: c_int = 3;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

type NewFn = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const SampleSpec,
    *const c_void,
    *const BufferAttr,
    *mut c_int,
) -> *mut c_void;
type ReadFn = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_int) -> c_int;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut c_void);
type LatencyFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> u64;
type StrErrorFn = unsafe extern "C" fn(c_int) -> *const c_char;

struct PaApi {
    _simple: Box<dyn Library>,
    _core: Box<dyn Library>,
    new: NewFn,
    read: ReadFn,
    write: WriteFn,
    free: FreeFn,
    latency: Option<LatencyFn>,
    strerror: StrErrorFn,
}

impl PaApi {
    fn load(loader: &dyn LibraryLoader) -> Result<Arc<Self>> {
        let simple = loader
            .open(&["libpulse-simple.so.0", "libpulse-simple.so"])
            .map_err(|e| CodecError::Audio(format!("libpulse-simple not found: {e}")))?;
        let core = loader
            .open(&["libpulse.so.0", "libpulse.so"])
            .map_err(|e| CodecError::Audio(format!("libpulse not found: {e}")))?;
        unsafe {
            Ok(Arc::new(Self {
                new: resolve(simple.as_ref(), "pa_simple_new").map_err(CodecError::Audio)?,
                read: resolve(simple.as_ref(), "pa_simple_read").map_err(CodecError::Audio)?,
                write: resolve(simple.as_ref(), "pa_simple_write").map_err(CodecError::Audio)?,
                free: resolve(simple.as_ref(), "pa_simple_free").map_err(CodecError::Audio)?,
                latency: resolve(simple.as_ref(), "pa_simple_get_latency").ok(),
                strerror: resolve(core.as_ref(), "pa_strerror").map_err(CodecError::Audio)?,
                _simple: simple,
                _core: core,
            }))
        }
    }

    fn error(&self, what: &str, code: c_int) -> CodecError {
        let msg = unsafe {
            let p = (self.strerror)(code);
            if p.is_null() {
                format!("error {code}")
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        CodecError::Audio(format!("{what}: {msg}"))
    }
}

/// An open `pa_simple` stream.
struct Simple {
    api: Arc<PaApi>,
    handle: *mut c_void,
}

// SAFETY: a pa_simple handle is used by one thread at a time (&mut self).
unsafe impl Send for Simple {}

impl Simple {
    fn open(
        api: Arc<PaApi>,
        record: bool,
        device: &str,
        stream: &str,
        format: AudioFormat,
        buffer_ms: u32,
    ) -> Result<Self> {
        let spec = SampleSpec {
            format: PA_SAMPLE_S16LE,
            rate: format.sample_rate,
            channels: format.channels as u8,
        };
        let chunk = (format.samples_for_us(u64::from(buffer_ms.max(1)) * 1000)
            * usize::from(format.channels)
            * 2) as u32;
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: if record { u32::MAX } else { chunk * 4 },
            prebuf: if record { u32::MAX } else { chunk },
            minreq: u32::MAX,
            fragsize: if record { chunk } else { u32::MAX },
        };
        let name = CString::new("cua-spacesd").expect("static");
        let stream = CString::new(stream)
            .map_err(|_| CodecError::InvalidArgument("nul in stream name".into()))?;
        let dev = CString::new(device)
            .map_err(|_| CodecError::InvalidArgument("nul in device".into()))?;
        let mut error = 0;
        let handle = unsafe {
            (api.new)(
                std::ptr::null(),
                name.as_ptr(),
                if record {
                    PA_STREAM_RECORD
                } else {
                    PA_STREAM_PLAYBACK
                },
                dev.as_ptr(),
                stream.as_ptr(),
                &spec,
                std::ptr::null(),
                &attr,
                &mut error,
            )
        };
        if handle.is_null() {
            return Err(api.error(&format!("pa_simple_new({device})"), error));
        }
        Ok(Self { api, handle })
    }

    fn read(&mut self, buf: &mut [i16]) -> Result<()> {
        let mut error = 0;
        let rc = unsafe {
            (self.api.read)(
                self.handle,
                buf.as_mut_ptr().cast(),
                buf.len() * 2,
                &mut error,
            )
        };
        if rc < 0 {
            return Err(self.api.error("pa_simple_read", error));
        }
        Ok(())
    }

    /// Stream latency in microseconds as the server reports it (for a record
    /// stream: how long ago the next unread sample was captured).
    fn latency_us(&mut self) -> Option<u64> {
        let latency = self.api.latency?;
        let mut error = 0;
        let value = unsafe { latency(self.handle, &mut error) };
        (value != u64::MAX && error == 0).then_some(value)
    }

    fn write(&mut self, buf: &[i16]) -> Result<()> {
        let mut error = 0;
        let rc = unsafe {
            (self.api.write)(self.handle, buf.as_ptr().cast(), buf.len() * 2, &mut error)
        };
        if rc < 0 {
            return Err(self.api.error("pa_simple_write", error));
        }
        Ok(())
    }
}

impl Drop for Simple {
    fn drop(&mut self) {
        unsafe { (self.api.free)(self.handle) };
    }
}

/// Runs `pactl` with a hard timeout.
pub fn pactl(args: &[&str]) -> Result<String> {
    let mut child = Command::new("pactl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CodecError::Audio(format!("pactl: {e}")))?;
    // Drain pipes on threads so large output cannot block pactl.
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut v);
        v
    });
    let err_reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr, &mut v);
        v
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > Duration::from_secs(5) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CodecError::Audio(format!("pactl {args:?} timed out")));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(CodecError::Audio(format!("pactl: {e}"))),
        }
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if !status.success() {
        return Err(CodecError::Audio(format!(
            "pactl {args:?} failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn load_module(args: &[&str]) -> Result<u32> {
    let mut all = vec!["load-module"];
    all.extend_from_slice(args);
    pactl(&all)?
        .trim()
        .parse()
        .map_err(|_| CodecError::Audio("load-module returned no index".into()))
}

fn unload_module(index: u32) {
    let _ = pactl(&["unload-module", &index.to_string()]);
}

/// Makes sure a default sink exists (the Spaces image has no sound card):
/// creates a `cua_desktop` null sink and makes it the default if there is
/// no sink at all. Returns the default sink name.
pub fn ensure_desktop_sink() -> Result<String> {
    let sinks = pactl(&["list", "short", "sinks"])?;
    if sinks.trim().is_empty() {
        load_module(&[
            "module-null-sink",
            "sink_name=cua_desktop",
            "sink_properties=device.description=cua-desktop",
        ])?;
        pactl(&["set-default-sink", "cua_desktop"])?;
    }
    Ok(pactl(&["get-default-sink"])
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "cua_desktop".into()))
}

/// A playing application stream (`pactl list sink-inputs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkInput {
    /// Sink input index.
    pub index: u32,
    /// `application.process.id`.
    pub pid: Option<u32>,
    /// `application.name`.
    pub app_name: Option<String>,
    /// `application.process.binary`.
    pub binary: Option<String>,
}

/// Parses `pactl -f json list sink-inputs`.
pub fn parse_sink_inputs_json(json: &str) -> Vec<SinkInput> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let index = item.get("index")?.as_u64()? as u32;
            let props = item.get("properties");
            let prop = |k: &str| {
                props
                    .and_then(|p| p.get(k))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            };
            Some(SinkInput {
                index,
                pid: prop("application.process.id").and_then(|p| p.parse().ok()),
                app_name: prop("application.name"),
                binary: prop("application.process.binary"),
            })
        })
        .collect()
}

fn sink_inputs() -> Vec<SinkInput> {
    pactl(&["-f", "json", "list", "sink-inputs"])
        .map(|s| parse_sink_inputs_json(&s))
        .unwrap_or_default()
}

/// Smoothed record-stream latency for timestamping. Reports are noisy (the
/// first reads carry the stream's start-up buffer), so the estimate is the
/// median of recent plausible samples, which moves slowly enough to keep
/// packet timestamps contiguous.
#[derive(Debug, Default)]
struct SourceLatency {
    samples: std::collections::VecDeque<u64>,
}

impl SourceLatency {
    /// Latencies above this are start-up transients, not steady state.
    const MAX_PLAUSIBLE_US: u64 = 250_000;
    const WINDOW: usize = 32;

    fn update(&mut self, reported: Option<u64>) -> u64 {
        if let Some(value) = reported.filter(|value| *value <= Self::MAX_PLAUSIBLE_US) {
            self.samples.push_back(value);
            while self.samples.len() > Self::WINDOW {
                self.samples.pop_front();
            }
        }
        let mut sorted: Vec<u64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied().unwrap_or(0)
    }
}

/// Removes read-delivery jitter from record timestamps. The source clock
/// is continuous, so chunk k's true end time is `T0 + k * chunk`; a read can
/// only return late (scheduler wakeups, the server handing over several
/// chunks at once), never early. Over a short window the smallest
/// `read_at - samples_so_far` is the best estimate of `T0`, and the excess of
/// the current read over it is jitter to subtract. The window is short so a
/// source clock that drifts against the host clock is followed. Under
/// gVisor (runsc) reads arrive in bursts of up to ~60 ms, which stamped
/// audio that much late and skewed A/V against video.
#[derive(Debug)]
struct ReadJitter {
    window_us: u64,
    /// (read_at_us, read_at_us - produced_us) for recent reads.
    offsets: std::collections::VecDeque<(u64, i64)>,
    produced_us: u64,
}

impl ReadJitter {
    const WINDOW_US: u64 = 200_000;

    fn new() -> Self {
        Self {
            window_us: Self::WINDOW_US,
            offsets: std::collections::VecDeque::new(),
            produced_us: 0,
        }
    }

    /// Record a chunk of `chunk_us` read at `read_at_us`; returns how late
    /// (microseconds) this read is against the window's earliest delivery.
    fn update(&mut self, read_at_us: u64, chunk_us: u64) -> u64 {
        self.produced_us += chunk_us;
        let offset = read_at_us as i64 - self.produced_us as i64;
        while self
            .offsets
            .front()
            .is_some_and(|(at, _)| read_at_us.saturating_sub(*at) > self.window_us)
        {
            self.offsets.pop_front();
        }
        // Bounded: one entry per chunk inside the window.
        if self.offsets.len() >= 4096 {
            self.offsets.pop_front();
        }
        self.offsets.push_back((read_at_us, offset));
        let min = self.offsets.iter().map(|(_, o)| *o).min().unwrap_or(offset);
        (offset - min).max(0) as u64
    }
}

/// Media-clock microseconds (see [`crate::media_clock_us`]).
fn now_us() -> u64 {
    crate::types::media_clock_us()
}

/// PulseAudio / PipeWire capture backend.
pub struct PulseCapture {
    api: Arc<PaApi>,
}

impl PulseCapture {
    /// Loads libpulse-simple (fails cleanly when absent).
    pub fn new() -> Result<Self> {
        Self::with_loader(&SystemLoader)
    }

    /// With an explicit loader (tests).
    pub fn with_loader(loader: &dyn LibraryLoader) -> Result<Self> {
        Ok(Self {
            api: PaApi::load(loader)?,
        })
    }
}

struct PulseStream {
    source: AudioSourceInfo,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    modules: Vec<u32>,
}

impl AudioStream for PulseStream {
    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }
    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for m in self.modules.drain(..).rev() {
            unload_module(m);
        }
    }
}

impl Drop for PulseStream {
    fn drop(&mut self) {
        self.stop();
    }
}

impl AudioCapture for PulseCapture {
    fn backend(&self) -> AudioBackend {
        AudioBackend::Pulse
    }

    fn sources(&self) -> Result<Vec<AudioSourceInfo>> {
        let mut out = vec![AudioSourceInfo::desktop("Desktop audio")];
        let mut seen = Vec::new();
        for input in sink_inputs() {
            let Some(pid) = input.pid else { continue };
            if seen.contains(&pid) {
                continue;
            }
            seen.push(pid);
            out.push(AudioSourceInfo {
                source_id: format!("app:{pid}"),
                kind: AudioSourceKind::Application,
                name: input
                    .app_name
                    .clone()
                    .unwrap_or_else(|| format!("pid {pid}")),
                pid: Some(pid),
                app_id: input.binary.clone(),
                available: true,
                limitation: None,
                desktop_fallback: false,
            });
        }
        Ok(out)
    }

    fn start(
        &self,
        request: &AudioCaptureRequest,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Box<dyn AudioStream>> {
        let (device, source, modules) = if request.source_id == DESKTOP_SOURCE_ID {
            (
                "@DEFAULT_MONITOR@".to_owned(),
                AudioSourceInfo::desktop("Desktop audio"),
                Vec::new(),
            )
        } else {
            match setup_per_app(&request.source_id) {
                Ok((device, source, modules)) => (device, source, modules),
                Err(e) => {
                    let mut src = AudioSourceInfo::desktop("Desktop audio");
                    src.source_id = request.source_id.clone();
                    src.kind = AudioSourceKind::Application;
                    src.desktop_fallback = true;
                    src.limitation = Some(format!(
                        "per-app capture failed ({e}); capturing the desktop mix"
                    ));
                    sink.on_state(AudioCaptureState::Fallback(e.to_string()));
                    ("@DEFAULT_MONITOR@".to_owned(), src, Vec::new())
                }
            }
        };
        spawn_record(&self.api, &device, source, modules, request, sink)
    }
}

/// Records an arbitrary PulseAudio source by name (for example a virtual
/// microphone created by [`PulseUplink`]).
pub fn record_device(
    capture: &PulseCapture,
    device: &str,
    request: &AudioCaptureRequest,
    sink: Arc<dyn AudioFrameSink>,
) -> Result<Box<dyn AudioStream>> {
    let mut source = AudioSourceInfo::desktop(device);
    source.source_id = device.to_owned();
    spawn_record(&capture.api, device, source, Vec::new(), request, sink)
}

fn spawn_record(
    api: &Arc<PaApi>,
    device: &str,
    source: AudioSourceInfo,
    modules: Vec<u32>,
    request: &AudioCaptureRequest,
    sink: Arc<dyn AudioFrameSink>,
) -> Result<Box<dyn AudioStream>> {
    let simple = match Simple::open(
        api.clone(),
        true,
        device,
        "cua capture",
        request.format,
        request.buffer_ms,
    ) {
        Ok(s) => s,
        Err(e) => {
            for m in modules.into_iter().rev() {
                unload_module(m);
            }
            return Err(e);
        }
    };
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    let format = request.format;
    let chunk_samples = format.samples_for_us(u64::from(request.buffer_ms.max(1)) * 1000);
    let thread = std::thread::Builder::new()
        .name("cua-pulse-capture".into())
        .spawn(move || {
            let mut simple = simple;
            let mut buf = vec![0i16; chunk_samples * usize::from(format.channels)];
            let chunk_us = format.duration_us(chunk_samples);
            let mut latency = SourceLatency::default();
            let mut jitter = ReadJitter::new();
            sink.on_state(AudioCaptureState::Active);
            while flag.load(Ordering::Acquire) {
                if let Err(e) = simple.read(&mut buf) {
                    sink.on_state(AudioCaptureState::Suspended(e.to_string()));
                    break;
                }
                let read_at = now_us();
                let source_latency = latency.update(simple.latency_us());
                let late = jitter.update(read_at, chunk_us);
                sink.on_audio(AudioFrame {
                    // Capture time of the first sample: the chunk ended
                    // `source_latency` before it was delivered, and was
                    // delivered `late` after the source produced it.
                    pts_us: read_at.saturating_sub(chunk_us + source_latency + late),
                    format,
                    samples: buf.clone(),
                });
            }
        })
        .map_err(|e| CodecError::Audio(format!("spawning capture thread: {e}")))?;
    Ok(Box::new(PulseStream {
        source,
        running,
        thread: Some(thread),
        modules,
    }))
}

fn setup_per_app(source_id: &str) -> Result<(String, AudioSourceInfo, Vec<u32>)> {
    let pid: u32 = source_id
        .strip_prefix("app:")
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| CodecError::InvalidArgument(format!("unknown audio source {source_id}")))?;
    let inputs: Vec<SinkInput> = sink_inputs()
        .into_iter()
        .filter(|i| i.pid == Some(pid))
        .collect();
    if inputs.is_empty() {
        return Err(CodecError::Audio(format!(
            "pid {pid} has no playing streams"
        )));
    }
    let default_sink = pactl(&["get-default-sink"])?.trim().to_owned();
    let sink_name = format!("cua_app_{pid}");
    let mut modules = Vec::new();
    let result = (|| {
        modules.push(load_module(&[
            "module-null-sink",
            &format!("sink_name={sink_name}"),
            &format!("sink_properties=device.description=cua-app-{pid}"),
        ])?);
        modules.push(load_module(&[
            "module-loopback",
            &format!("source={sink_name}.monitor"),
            &format!("sink={default_sink}"),
            "latency_msec=1",
        ])?);
        for input in &inputs {
            pactl(&["move-sink-input", &input.index.to_string(), &sink_name])?;
        }
        Ok::<_, CodecError>(())
    })();
    if let Err(e) = result {
        for m in modules.into_iter().rev() {
            unload_module(m);
        }
        return Err(e);
    }
    let first = &inputs[0];
    Ok((
        format!("{sink_name}.monitor"),
        AudioSourceInfo {
            source_id: source_id.to_owned(),
            kind: AudioSourceKind::Application,
            name: first
                .app_name
                .clone()
                .unwrap_or_else(|| format!("pid {pid}")),
            pid: Some(pid),
            app_id: first.binary.clone(),
            available: true,
            limitation: Some(
                "streams the app opens after capture starts stay on the default sink".into(),
            ),
            desktop_fallback: false,
        },
        modules,
    ))
}

/// Uplink: a virtual microphone fed by client audio.
pub struct PulseUplink {
    simple: Simple,
    format: AudioFormat,
    name: String,
    modules: Vec<u32>,
    previous_default: Option<String>,
}

impl PulseUplink {
    /// Creates the virtual source `name` (default "cua-uplink") and opens a
    /// playback stream into it.
    pub fn open(name: &str, format: AudioFormat, set_default_input: bool) -> Result<Self> {
        let api = PaApi::load(&SystemLoader)?;
        let name = if name.is_empty() { "cua-uplink" } else { name };
        let sink = format!("{}_sink", name.replace('-', "_"));
        let mut modules = vec![load_module(&[
            "module-null-sink",
            &format!("sink_name={sink}"),
            &format!("rate={}", format.sample_rate),
            &format!("channels={}", format.channels),
            &format!("sink_properties=device.description={name}-sink"),
        ])?];
        match load_module(&[
            "module-remap-source",
            &format!("master={sink}.monitor"),
            &format!("source_name={name}"),
            &format!("source_properties=device.description={name}"),
        ]) {
            Ok(m) => modules.push(m),
            Err(e) => {
                unload_module(modules[0]);
                return Err(e);
            }
        }
        let previous_default = if set_default_input {
            let prev = pactl(&["get-default-source"])
                .ok()
                .map(|s| s.trim().to_owned());
            pactl(&["set-default-source", name])?;
            prev
        } else {
            None
        };
        let simple = match Simple::open(api, false, &sink, "cua uplink", format, 20) {
            Ok(s) => s,
            Err(e) => {
                for m in modules.into_iter().rev() {
                    unload_module(m);
                }
                return Err(e);
            }
        };
        Ok(Self {
            simple,
            format,
            name: name.to_owned(),
            modules,
            previous_default,
        })
    }
}

impl AudioSink for PulseUplink {
    fn format(&self) -> AudioFormat {
        self.format
    }
    fn write(&mut self, samples: &[i16]) -> Result<()> {
        self.simple.write(samples)
    }
    fn source_name(&self) -> &str {
        &self.name
    }
}

impl Drop for PulseUplink {
    fn drop(&mut self) {
        if let Some(prev) = self.previous_default.take() {
            let _ = pactl(&["set-default-source", &prev]);
        }
        for m in self.modules.drain(..).rev() {
            unload_module(m);
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn source_latency_ignores_startup_transients_and_moves_slowly() {
        let mut latency = super::SourceLatency::default();
        assert_eq!(
            latency.update(Some(695_000)),
            0,
            "start-up buffer is ignored"
        );
        assert_eq!(latency.update(None), 0);
        for _ in 0..10 {
            latency.update(Some(20_000));
        }
        assert_eq!(
            latency.update(Some(90_000)),
            20_000,
            "one outlier does not move the median"
        );
        for _ in 0..40 {
            latency.update(Some(40_000));
        }
        assert_eq!(latency.update(Some(40_000)), 40_000);
    }

    use super::*;

    #[test]
    fn read_jitter_removes_bursty_delivery_and_follows_drift() {
        let mut j = ReadJitter::new();
        // Steady 10 ms reads: no correction.
        let mut t = 1_000_000u64;
        for _ in 0..20 {
            t += 10_000;
            assert_eq!(j.update(t, 10_000), 0);
        }
        // A 40 ms stall, then the backlog arrives at once: each late read
        // is corrected back to its production time.
        t += 50_000;
        assert_eq!(j.update(t, 10_000), 40_000);
        assert_eq!(j.update(t, 10_000), 30_000);
        assert_eq!(j.update(t, 10_000), 20_000);
        assert_eq!(j.update(t, 10_000), 10_000);
        assert_eq!(j.update(t, 10_000), 0);
        // A source clock 2% slow against the host: after the window the
        // estimate follows it instead of growing without bound.
        for _ in 0..200 {
            t += 10_200;
            let late = j.update(t, 10_000);
            assert!(late <= 200_000 * 2 / 100 + 1, "late {late}");
        }
    }

    #[test]
    fn sink_input_json_parsing() {
        let json = r#"[{"index":7,"properties":{"application.name":"paplay","application.process.id":"42","application.process.binary":"paplay"}},{"index":8,"properties":{}}]"#;
        let list = parse_sink_inputs_json(json);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].pid, Some(42));
        assert_eq!(list[0].app_name.as_deref(), Some("paplay"));
        assert_eq!(list[1].pid, None);
        assert!(parse_sink_inputs_json("not json").is_empty());
    }

    #[test]
    fn missing_libpulse_is_a_clean_error() {
        let err = PulseCapture::with_loader(&crate::backends::dynlib::FakeLoader::default())
            .err()
            .unwrap();
        assert!(err.to_string().contains("libpulse-simple not found"));
    }
}
