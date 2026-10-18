// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! AMD Advanced Media Framework probe (MIT headers; runtime loaded
//! dynamically).
//!
//! Written from AMD's public AMF API documentation: the runtime
//! (`amfrt64.dll` / `libamfrt64.so.1`) exports `AMFQueryVersion` and
//! `AMFInit`. The probe reads the runtime version; enumerating per-codec
//! component support requires the factory interface, which is part of the
//! (not yet implemented) encode session. Real-hardware lane:
//! `CUA_CODEC_TEST_AMF=1`.

use std::time::Instant;

use super::dynlib::{resolve, LibraryLoader};
use crate::probe::{EncoderInfo, ProbeStatus};
use crate::types::{Backend, LatencyClass, VideoCodec};

const BACKEND: Backend = Backend::Amf;

/// Runtime library names.
pub const AMF_LIBS: &[&str] = if cfg!(windows) {
    &["amfrt64.dll"]
} else {
    &["libamfrt64.so.1", "libamfrt64.so"]
};

type QueryVersionFn = unsafe extern "C" fn(*mut u64) -> i32;

/// Splits `AMF_FULL_VERSION` into (major, minor, release, build).
pub fn split_version(v: u64) -> (u16, u16, u16, u16) {
    (
        (v >> 48) as u16,
        (v >> 32) as u16,
        (v >> 16) as u16,
        v as u16,
    )
}

/// Probes AMF through `loader`.
pub fn probe(loader: &dyn LibraryLoader) -> EncoderInfo {
    let started = Instant::now();
    if !(cfg!(target_os = "linux") || cfg!(windows)) {
        return EncoderInfo::unavailable(BACKEND, "AMF exists only on Linux and Windows", started);
    }
    let lib = match loader.open(AMF_LIBS) {
        Ok(lib) => lib,
        Err(e) => {
            return EncoderInfo::unavailable(
                BACKEND,
                format!("AMF runtime not found ({e})"),
                started,
            )
        }
    };
    let version = unsafe {
        let query: QueryVersionFn = match resolve(lib.as_ref(), "AMFQueryVersion") {
            Ok(f) => f,
            Err(e) => return EncoderInfo::unavailable(BACKEND, e, started),
        };
        let mut v = 0u64;
        let status = query(&mut v);
        if status != 0 {
            return EncoderInfo::unavailable(
                BACKEND,
                format!("AMFQueryVersion returned {status}"),
                started,
            );
        }
        v
    };
    let (major, minor, release, build) = split_version(version);
    let mut codecs = vec![VideoCodec::H264, VideoCodec::Hevc];
    // AV1 encode arrived with AMF 1.4.28 (RDNA3); still unverified per GPU.
    if (major, minor, release) >= (1, 4, 28) {
        codecs.push(VideoCodec::Av1);
    }
    EncoderInfo {
        backend: BACKEND,
        codecs,
        hardware: true,
        max_width: 4096,
        max_height: 4096,
        latency_class: LatencyClass::Hardware,
        limitations: vec![
            "encode session not implemented in this build (probe only); selection skips AMF".into(),
            "codec list inferred from runtime version, not per-GPU component query".into(),
        ],
        status: ProbeStatus::DetectedOnly,
        device: Some(format!(
            "AMF {major}.{minor}.{release}.{build} via {}",
            lib.name()
        )),
        probe_ms: started.elapsed().as_millis() as u32,
    }
}

#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::super::dynlib::FakeLoader;
    use super::*;

    unsafe extern "C" fn fake_version(v: *mut u64) -> i32 {
        unsafe { *v = (1u64 << 48) | (4u64 << 32) | (33u64 << 16) | 7 };
        0
    }

    #[test]
    fn missing_runtime_is_unavailable() {
        assert!(matches!(
            probe(&FakeLoader::default()).status,
            ProbeStatus::Unavailable { .. }
        ));
    }

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn fake_runtime_version_is_reported() {
        let loader = FakeLoader::default().with_library(
            AMF_LIBS[0],
            &[("AMFQueryVersion", fake_version as *const () as usize)],
        );
        let info = probe(&loader);
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
        assert!(info.device.as_deref().unwrap().starts_with("AMF 1.4.33.7"));
        assert!(info.codecs.contains(&VideoCodec::Av1));
    }

    #[test]
    fn amf_real_hardware_probe() {
        if std::env::var_os("CUA_CODEC_TEST_AMF").is_none() {
            eprintln!("skipped: set CUA_CODEC_TEST_AMF=1 on an AMD host");
            return;
        }
        let info = probe(&super::super::dynlib::SystemLoader);
        eprintln!("{info:#?}");
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
    }
}
