// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The encoder probe behind the `h264_hw` capability, run off the caller's
//! thread.
//!
//! Probing loads GPU drivers (CUDA for NVENC, libva for VA-API). On a host
//! whose compositor holds the DRM modeset lock, a probe can block in the
//! kernel indefinitely. `GetCapabilities` runs on an async worker, so it
//! never waits for the probe: until the probe finishes, `h264_hw` reports
//! that it is still running.

use std::sync::{Once, OnceLock};
use std::time::Duration;

use cua_media_codec::probe::{EncoderInfo, Isolation};
use cua_media_codec::ProbeStatus;

use crate::grpc::FeatureStatus;

/// How long one backend's probe child may run before it is killed.
pub const PROBE_CHILD_TIMEOUT: Duration = Duration::from_secs(10);

/// Why `h264_hw` is unsupported while the probe runs.
pub const PROBE_PENDING: &str = "the hardware encoder probe has not finished";

/// One encoder probe, started at most once and read without blocking.
pub struct EncoderProbe {
    started: Once,
    result: OnceLock<Vec<EncoderInfo>>,
}

impl EncoderProbe {
    pub const fn new() -> Self {
        Self {
            started: Once::new(),
            result: OnceLock::new(),
        }
    }

    /// Runs `probe` on its own thread, unless a probe already started.
    pub fn start(&'static self, probe: impl FnOnce() -> Vec<EncoderInfo> + Send + 'static) {
        self.started.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("encoder-probe".into())
                .spawn(move || {
                    let _ = self.result.set(probe());
                });
            if let Err(error) = spawned {
                tracing::warn!(%error, "could not start the encoder probe; h264_hw stays unsupported");
            }
        });
    }

    /// The `h264_hw` feature; `missing` is the limitation when the probe
    /// found no hardware encoder.
    pub fn h264_hw(&self, missing: &str) -> FeatureStatus {
        let Some(encoders) = self.result.get() else {
            return FeatureStatus::no("h264_hw", PROBE_PENDING);
        };
        let hardware = encoders
            .iter()
            .find(|encoder| encoder.status == ProbeStatus::Available && encoder.hardware);
        match hardware {
            Some(encoder) => FeatureStatus {
                attributes: vec![(
                    "encoder".into(),
                    format!("{:?}", encoder.backend).to_lowercase(),
                )],
                ..FeatureStatus::yes("h264_hw")
            },
            None => FeatureStatus::no("h264_hw", missing),
        }
    }
}

impl Default for EncoderProbe {
    fn default() -> Self {
        Self::new()
    }
}

static HOST: EncoderProbe = EncoderProbe::new();

/// Starts this process's probe with `isolation`, filling the
/// `cua-media-codec` cache the stream encoders read. The daemon passes
/// [`Isolation::Subprocess`] so a hung driver stays in a child it kills.
pub fn start_host_probe(isolation: Isolation) {
    HOST.start(move || cua_media_codec::probe::probe_cached(&isolation));
}

/// The child-process isolation over this executable, which must call
/// `cua_media_codec::probe::maybe_run_probe_child` first in `main`.
pub fn subprocess_isolation() -> Option<Isolation> {
    Some(Isolation::Subprocess {
        program: std::env::current_exe().ok()?,
        args: Vec::new(),
        timeout: PROBE_CHILD_TIMEOUT,
    })
}

/// This process's `h264_hw` feature. Starts an in-process probe on a
/// background thread when the binary started none.
pub fn host_h264_hw(missing: &str) -> FeatureStatus {
    start_host_probe(Isolation::InProcess);
    HOST.h264_hw(missing)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use cua_media_codec::Backend;

    use super::*;

    fn found(backend: Backend) -> EncoderInfo {
        EncoderInfo {
            status: ProbeStatus::Available,
            codecs: vec![cua_media_codec::VideoCodec::H264],
            ..EncoderInfo::unavailable(backend, "", Instant::now())
        }
    }

    #[test]
    fn a_blocked_probe_never_blocks_the_feature() {
        let probe: &'static EncoderProbe = Box::leak(Box::new(EncoderProbe::new()));
        let (release, wait) = mpsc::channel::<()>();
        let (done, finished) = mpsc::channel::<()>();
        probe.start(move || {
            let _ = wait.recv();
            let _ = done.send(());
            vec![found(Backend::Vaapi)]
        });

        let started = Instant::now();
        let pending = probe.h264_hw("none");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!pending.supported);
        assert_eq!(pending.limitation, PROBE_PENDING);

        release.send(()).unwrap();
        finished.recv().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while probe.result.get().is_none() {
            assert!(Instant::now() < deadline, "the probe result never landed");
            std::thread::yield_now();
        }
        let ready = probe.h264_hw("none");
        assert!(ready.supported);
        assert_eq!(
            ready.attributes,
            vec![("encoder".to_owned(), "vaapi".to_owned())]
        );
    }

    #[test]
    fn a_probe_without_hardware_reports_the_missing_reason() {
        let probe: &'static EncoderProbe = Box::leak(Box::new(EncoderProbe::new()));
        let (done, finished) = mpsc::channel::<()>();
        probe.start(move || {
            let _ = done.send(());
            vec![
                found(Backend::OpenH264),
                EncoderInfo::unavailable(Backend::Nvenc, "timed out", Instant::now()),
            ]
        });
        finished.recv().unwrap();
        while probe.result.get().is_none() {
            std::thread::yield_now();
        }
        let feature = probe.h264_hw("no hardware H.264 encoder was found");
        assert!(!feature.supported);
        assert_eq!(feature.limitation, "no hardware H.264 encoder was found");
    }
}
