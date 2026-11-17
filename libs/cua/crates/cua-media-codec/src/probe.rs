// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Runtime encoder probing.
//!
//! Every backend is probed with a real operation (a test encode for the
//! backends with a session implementation, a driver/session query for the
//! probe-only hardware backends). Probes can run:
//!
//! - **in process**, guarded by `catch_unwind` (panics are contained, but a
//!   driver that segfaults would take the process down); or
//! - **in a child process** per backend ([`Isolation::Subprocess`]), with a
//!   kill timeout, so a crashing or hanging driver never reaches the daemon.
//!   The child is any executable that calls [`maybe_run_probe_child`] at the
//!   top of `main` (the daemon itself, or the bundled `cua-codec-probe`
//!   binary).
//!
//! Results are cached per process; call [`invalidate_cache`] on GPU or
//! display topology changes.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backends::dynlib::{LibraryLoader, SystemLoader};
use crate::types::{Backend, LatencyClass, VideoCodec};

/// Environment variable naming the backend a probe child should probe.
pub const PROBE_CHILD_ENV: &str = "CUA_CODEC_PROBE_CHILD";

/// Probe outcome for one backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ProbeStatus {
    /// A real test encode succeeded; the backend is selectable.
    Available,
    /// The hardware and runtime are present, but this build has no encode
    /// session for the backend yet. Reported for capabilities, never
    /// selected.
    DetectedOnly,
    /// Not usable here.
    Unavailable {
        /// Why.
        reason: String,
    },
}

/// What one encoder backend can do on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncoderInfo {
    /// Backend.
    pub backend: Backend,
    /// Codecs that passed the probe.
    pub codecs: Vec<VideoCodec>,
    /// Hardware (GPU / fixed-function) encoder.
    pub hardware: bool,
    /// Largest supported width.
    pub max_width: u32,
    /// Largest supported height.
    pub max_height: u32,
    /// Latency ranking used by selection.
    pub latency_class: LatencyClass,
    /// Quirks and degradations (for capabilities).
    pub limitations: Vec<String>,
    /// Probe outcome.
    pub status: ProbeStatus,
    /// Device / driver description.
    pub device: Option<String>,
    /// Time the probe took, ms.
    pub probe_ms: u32,
}

impl EncoderInfo {
    /// An unavailable entry.
    pub fn unavailable(backend: Backend, reason: impl Into<String>, started: Instant) -> Self {
        Self {
            backend,
            codecs: Vec::new(),
            hardware: backend.is_hardware(),
            max_width: 0,
            max_height: 0,
            latency_class: if backend.is_hardware() {
                LatencyClass::Hardware
            } else {
                LatencyClass::SoftwareRealtime
            },
            limitations: Vec::new(),
            status: ProbeStatus::Unavailable {
                reason: reason.into(),
            },
            device: None,
            probe_ms: started.elapsed().as_millis() as u32,
        }
    }

    /// True if selection may pick this backend.
    pub fn is_usable(&self) -> bool {
        self.status == ProbeStatus::Available && !self.codecs.is_empty()
    }
}

/// A decoder available on this host (for native clients and tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoderInfo {
    /// Backend.
    pub backend: Backend,
    /// Decodable codecs.
    pub codecs: Vec<VideoCodec>,
    /// Hardware decoder.
    pub hardware: bool,
}

/// Where probes run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Isolation {
    /// In this process, panics contained with `catch_unwind`.
    InProcess,
    /// One child process per backend. `program` must call
    /// [`maybe_run_probe_child`] early in `main`.
    Subprocess {
        /// Executable to spawn.
        program: PathBuf,
        /// Extra arguments.
        args: Vec<String>,
        /// Kill the child after this long.
        timeout: Duration,
    },
}

impl Isolation {
    /// Subprocess isolation re-executing the current binary.
    pub fn current_exe(timeout: Duration) -> std::io::Result<Self> {
        Ok(Self::Subprocess {
            program: std::env::current_exe()?,
            args: Vec::new(),
            timeout,
        })
    }
}

/// Backends compiled into this build, in probe priority order.
pub fn compiled_backends() -> Vec<Backend> {
    let mut out = Vec::new();
    if cfg!(feature = "nvenc") {
        out.push(Backend::Nvenc);
    }
    if cfg!(feature = "vaapi") {
        out.push(Backend::Vaapi);
    }
    if cfg!(feature = "qsv") {
        out.push(Backend::Qsv);
    }
    if cfg!(feature = "amf") {
        out.push(Backend::Amf);
    }
    if cfg!(target_os = "macos") {
        out.push(Backend::VideoToolbox);
    }
    if cfg!(feature = "mediafoundation") {
        out.push(Backend::MediaFoundation);
    }
    if cfg!(feature = "openh264") {
        out.push(Backend::OpenH264);
    }
    if cfg!(feature = "av1-rav1e") {
        out.push(Backend::Rav1e);
    }
    out
}

/// Probes one backend in this process (panics contained).
pub fn probe_backend(backend: Backend, loader: &dyn LibraryLoader) -> EncoderInfo {
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        probe_backend_unguarded(backend, loader)
    }));
    match result {
        Ok(info) => info,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "unknown panic".into());
            EncoderInfo::unavailable(backend, format!("probe panicked: {msg}"), started)
        }
    }
}

#[allow(unused_variables)]
fn probe_backend_unguarded(backend: Backend, loader: &dyn LibraryLoader) -> EncoderInfo {
    let started = Instant::now();
    let mut info = match backend {
        #[cfg(feature = "nvenc")]
        Backend::Nvenc => crate::backends::nvenc::probe(loader),
        #[cfg(feature = "vaapi")]
        Backend::Vaapi => crate::backends::vaapi::probe(loader),
        #[cfg(feature = "qsv")]
        Backend::Qsv => crate::backends::qsv::probe(loader),
        #[cfg(feature = "amf")]
        Backend::Amf => crate::backends::amf::probe(loader),
        #[cfg(feature = "mediafoundation")]
        Backend::MediaFoundation => crate::backends::mediafoundation::probe(),
        #[cfg(target_os = "macos")]
        Backend::VideoToolbox => probe_videotoolbox(started),
        #[cfg(feature = "openh264")]
        Backend::OpenH264 => match crate::backends::openh264::probe_encode() {
            Ok(()) => EncoderInfo {
                backend,
                codecs: vec![VideoCodec::H264],
                hardware: false,
                max_width: crate::backends::openh264::MAX_WIDTH,
                max_height: crate::backends::openh264::MAX_HEIGHT,
                latency_class: LatencyClass::SoftwareRealtime,
                limitations: vec![
                    "no intra refresh (IDR on demand only)".into(),
                    "H.264 Constrained Baseline only".into(),
                    crate::backends::openh264::library_description().into(),
                ],
                status: ProbeStatus::Available,
                device: Some("CPU".into()),
                probe_ms: 0,
            },
            Err(e) => EncoderInfo::unavailable(backend, e.to_string(), started),
        },
        #[cfg(feature = "av1-rav1e")]
        Backend::Rav1e => match crate::backends::av1::probe_encode() {
            Ok(()) => EncoderInfo {
                backend,
                codecs: vec![VideoCodec::Av1],
                hardware: false,
                max_width: 4096,
                max_height: 2304,
                latency_class: LatencyClass::SoftwareSlow,
                limitations: vec!["software AV1: too slow for 1080p60 on most hosts".into()],
                status: ProbeStatus::Available,
                device: Some("CPU".into()),
                probe_ms: 0,
            },
            Err(e) => EncoderInfo::unavailable(backend, e.to_string(), started),
        },
        other => EncoderInfo::unavailable(other, "not compiled into this build", started),
    };
    info.probe_ms = started.elapsed().as_millis() as u32;
    info
}

#[cfg(target_os = "macos")]
fn probe_videotoolbox(started: Instant) -> EncoderInfo {
    use crate::backends::videotoolbox;
    let mut codecs = Vec::new();
    let mut limitations = vec!["no intra refresh (IDR on demand only)".to_owned()];
    let mut hardware = false;
    for codec in [VideoCodec::H264, VideoCodec::Hevc] {
        match videotoolbox::probe_encode(codec) {
            Ok(hw) => {
                codecs.push(codec);
                hardware |= hw;
                if !hw {
                    limitations.push(format!(
                        "{} uses the software VideoToolbox encoder",
                        codec.as_str()
                    ));
                }
            }
            Err(e) => limitations.push(format!("{}: {e}", codec.as_str())),
        }
    }
    if codecs.is_empty() {
        return EncoderInfo::unavailable(Backend::VideoToolbox, limitations.join("; "), started);
    }
    EncoderInfo {
        backend: Backend::VideoToolbox,
        codecs,
        hardware,
        max_width: 4096,
        max_height: 2304,
        latency_class: if hardware {
            LatencyClass::Hardware
        } else {
            LatencyClass::SoftwareRealtime
        },
        limitations,
        status: ProbeStatus::Available,
        device: Some(format!("VideoToolbox ({})", std::env::consts::ARCH)),
        probe_ms: 0,
    }
}

/// Runs one backend probe in a child process.
pub fn probe_in_child(
    backend: Backend,
    program: &PathBuf,
    args: &[String],
    timeout: Duration,
) -> EncoderInfo {
    let started = Instant::now();
    let name = match serde_json::to_value(backend) {
        Ok(serde_json::Value::String(s)) => s,
        _ => return EncoderInfo::unavailable(backend, "unserialisable backend", started),
    };
    let mut child = match Command::new(program)
        .args(args)
        .env(PROBE_CHILD_ENV, &name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return EncoderInfo::unavailable(backend, format!("spawning probe child: {e}"), started)
        }
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let Some(status) = status else {
        // Do not join the reader: a grandchild may still hold the pipe.
        return EncoderInfo::unavailable(
            backend,
            format!("probe child timed out after {timeout:?} (killed)"),
            started,
        );
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return EncoderInfo::unavailable(
            backend,
            format!("probe child crashed: {status}"),
            started,
        );
    }
    match serde_json::from_str::<EncoderInfo>(output.trim()) {
        Ok(info) if info.backend == backend => info,
        Ok(_) => {
            EncoderInfo::unavailable(backend, "probe child answered for another backend", started)
        }
        Err(e) => EncoderInfo::unavailable(
            backend,
            format!("probe child output unparseable: {e}"),
            started,
        ),
    }
}

/// If this process was spawned as a probe child, runs the probe, prints the
/// JSON result on stdout and exits. Call first thing in `main`.
pub fn maybe_run_probe_child() {
    let Ok(name) = std::env::var(PROBE_CHILD_ENV) else {
        return;
    };
    let code = match serde_json::from_value::<Backend>(serde_json::Value::String(name.clone())) {
        Ok(backend) => {
            let info = probe_backend(backend, &SystemLoader);
            println!("{}", serde_json::to_string(&info).unwrap_or_default());
            0
        }
        Err(_) => {
            eprintln!("unknown backend {name}");
            2
        }
    };
    std::process::exit(code);
}

/// Probes every compiled backend with `isolation`, in priority order.
pub fn probe_with(isolation: &Isolation) -> Vec<EncoderInfo> {
    compiled_backends()
        .into_iter()
        .map(|backend| match isolation {
            Isolation::InProcess => probe_backend(backend, &SystemLoader),
            Isolation::Subprocess {
                program,
                args,
                timeout,
            } => probe_in_child(backend, program, args, *timeout),
        })
        .collect()
}

fn cache() -> &'static Mutex<Option<Vec<EncoderInfo>>> {
    static CACHE: OnceLock<Mutex<Option<Vec<EncoderInfo>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Probes every compiled backend in process (cached).
pub fn probe() -> Vec<EncoderInfo> {
    probe_cached(&Isolation::InProcess)
}

/// Probes with `isolation`, caching the result for the process lifetime.
pub fn probe_cached(isolation: &Isolation) -> Vec<EncoderInfo> {
    let mut guard = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cached) = guard.as_ref() {
        return cached.clone();
    }
    let result = probe_with(isolation);
    *guard = Some(result.clone());
    result
}

/// Forgets cached probe results (call on GPU / display changes).
pub fn invalidate_cache() {
    *cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Decoders compiled into this build that work on this host.
#[allow(unused_mut)]
pub fn probe_decoders() -> Vec<DecoderInfo> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let codecs: Vec<VideoCodec> = [VideoCodec::H264, VideoCodec::Hevc]
            .into_iter()
            .filter(|c| crate::backends::open_decoder(Backend::VideoToolbox, *c).is_ok())
            .collect();
        let hardware = crate::backends::videotoolbox::hardware_decode_supported(VideoCodec::H264);
        out.push(DecoderInfo {
            backend: Backend::VideoToolbox,
            codecs,
            hardware,
        });
    }
    #[cfg(feature = "openh264")]
    if crate::backends::open_decoder(Backend::OpenH264, VideoCodec::H264).is_ok() {
        out.push(DecoderInfo {
            backend: Backend::OpenH264,
            codecs: vec![VideoCodec::H264],
            hardware: false,
        });
    }
    #[cfg(feature = "av1-dav1d")]
    if crate::backends::open_decoder(Backend::Dav1d, VideoCodec::Av1).is_ok() {
        out.push(DecoderInfo {
            backend: Backend::Dav1d,
            codecs: vec![VideoCodec::Av1],
            hardware: false,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_entries_are_not_usable() {
        let info = EncoderInfo::unavailable(Backend::Nvenc, "x", Instant::now());
        assert!(!info.is_usable());
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"backend\":\"nvenc\""));
        assert_eq!(serde_json::from_str::<EncoderInfo>(&json).unwrap(), info);
    }

    #[test]
    fn compiled_backends_follow_priority_order() {
        let list = compiled_backends();
        let mut sorted = list.clone();
        sorted.sort_by_key(|b| b.priority());
        assert_eq!(list, sorted);
    }

    #[test]
    fn missing_child_program_is_contained() {
        let info = probe_in_child(
            Backend::OpenH264,
            &PathBuf::from("/nonexistent/cua-codec-probe"),
            &[],
            Duration::from_secs(1),
        );
        assert!(matches!(info.status, ProbeStatus::Unavailable { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn crashing_child_is_contained() {
        // `sh -c 'kill -SEGV $$'` stands in for a driver that segfaults
        // (`ulimit -c 0`: no core file in the crate directory).
        let info = probe_in_child(
            Backend::Nvenc,
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "ulimit -c 0; kill -SEGV $$".into()],
            Duration::from_secs(5),
        );
        match info.status {
            ProbeStatus::Unavailable { reason } => assert!(reason.contains("crashed"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn hanging_child_is_killed() {
        let started = Instant::now();
        let info = probe_in_child(
            Backend::Vaapi,
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "sleep 30".into()],
            Duration::from_millis(300),
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        match info.status {
            ProbeStatus::Unavailable { reason } => {
                assert!(reason.contains("timed out"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }
}
