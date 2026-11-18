// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The encoder probe behind the `h264_hw` capability, run off the caller's
//! thread.
//!
//! Probing loads GPU drivers (CUDA for NVENC, libva for VA-API). On a host
//! whose compositor holds the DRM modeset lock, a probe can block in the
//! kernel indefinitely. `GetCapabilities` runs on an async worker, so it
//! never waits for the probe: until the probe finishes, `h264_hw` reports
//! that it is still running. Stream encoders wait for the same probe rather
//! than starting their own.

use std::sync::{Condvar, Mutex, Once};
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
    result: Mutex<Option<Vec<EncoderInfo>>>,
    landed: Condvar,
}

impl EncoderProbe {
    pub const fn new() -> Self {
        Self {
            started: Once::new(),
            result: Mutex::new(None),
            landed: Condvar::new(),
        }
    }

    /// Runs `probe` on its own thread, unless a probe already started.
    pub fn start(&'static self, probe: impl FnOnce() -> Vec<EncoderInfo> + Send + 'static) {
        self.started.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("encoder-probe".into())
                .spawn(move || {
                    let encoders = probe();
                    *self.lock() = Some(encoders);
                    self.landed.notify_all();
                });
            if let Err(error) = spawned {
                tracing::warn!(%error, "could not start the encoder probe; h264_hw stays unsupported");
            }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Vec<EncoderInfo>>> {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The probe's result, waiting for it to land. Never call this from an
    /// async worker.
    pub fn encoders(&self) -> Vec<EncoderInfo> {
        let mut result = self.lock();
        loop {
            if let Some(encoders) = result.as_ref() {
                return encoders.clone();
            }
            result = self
                .landed
                .wait(result)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// The `h264_hw` feature, without waiting; `missing` is the limitation
    /// when the probe found no hardware encoder.
    pub fn h264_hw(&self, missing: &str) -> FeatureStatus {
        let result = self.lock();
        let Some(encoders) = result.as_ref() else {
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

/// Starts this process's probe with `isolation`, unless one started. The
/// daemon passes [`Isolation::Subprocess`] over its own executable (which
/// calls `cua_media_codec::probe::maybe_run_probe_child` first in `main`),
/// so a hung driver stays in a child it kills.
pub fn start_host_probe(isolation: Isolation) {
    HOST.start(move || cua_media_codec::probe::probe_with(&isolation));
}

/// This process's `h264_hw` feature. Starts an in-process probe on a
/// background thread when the binary started none.
pub fn host_h264_hw(missing: &str) -> FeatureStatus {
    start_host_probe(Isolation::InProcess);
    HOST.h264_hw(missing)
}

/// This process's probe result for a stream encoder, waiting for it to
/// land. Starts an in-process probe when the binary started none.
pub fn host_encoders() -> Vec<EncoderInfo> {
    start_host_probe(Isolation::InProcess);
    HOST.encoders()
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
        probe.start(move || {
            let _ = wait.recv();
            vec![found(Backend::Vaapi)]
        });

        let started = Instant::now();
        let pending = probe.h264_hw("none");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!pending.supported);
        assert_eq!(pending.limitation, PROBE_PENDING);

        release.send(()).unwrap();
        assert_eq!(probe.encoders().len(), 1);
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
        probe.start(move || {
            vec![
                found(Backend::OpenH264),
                EncoderInfo::unavailable(Backend::Nvenc, "timed out", Instant::now()),
            ]
        });
        probe.encoders();
        let feature = probe.h264_hw("no hardware H.264 encoder was found");
        assert!(!feature.supported);
        assert_eq!(feature.limitation, "no hardware H.264 encoder was found");
    }
}
