// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! VA-API probe (libva, MIT; loaded dynamically).
//!
//! Written from the public libva API reference (`va.h`, `va_drm.h`). The
//! probe does not trust device nodes: it opens each `/dev/dri/renderD*`,
//! initialises a display and asks the driver for its encode entry points.
//! Low-power (`VAEntrypointEncSliceLP`) entry points are preferred, per the
//! research notes, and reported in `limitations` when only the full-power
//! path exists.
//!
//! The encode session (sequence/picture/slice parameter buffers and our own
//! packed headers) is not implemented yet; see [`super::nvenc`] for the same
//! caveat. Real-hardware lane: `CUA_CODEC_TEST_VAAPI=1`.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::time::Instant;

use super::dynlib::{resolve, Library, LibraryLoader};
use crate::probe::{EncoderInfo, ProbeStatus};
use crate::types::{Backend, LatencyClass, VideoCodec};

const BACKEND: Backend = Backend::Vaapi;

/// libva core library names.
pub const VA_LIBS: &[&str] = &["libva.so.2", "libva.so"];
/// libva DRM backend names.
pub const VA_DRM_LIBS: &[&str] = &["libva-drm.so.2", "libva-drm.so"];

// VAProfile values (va.h).
const PROFILE_H264_MAIN: i32 = 6;
const PROFILE_H264_HIGH: i32 = 7;
const PROFILE_H264_CONSTRAINED_BASELINE: i32 = 13;
const PROFILE_HEVC_MAIN: i32 = 17;
const PROFILE_AV1_PROFILE0: i32 = 32;
// VAEntrypoint values.
const ENTRYPOINT_ENC_SLICE: i32 = 6;
const ENTRYPOINT_ENC_SLICE_LP: i32 = 8;

type GetDisplayDrmFn = unsafe extern "C" fn(c_int) -> *mut c_void;
type InitializeFn = unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int;
type TerminateFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type MaxNumFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type QueryProfilesFn = unsafe extern "C" fn(*mut c_void, *mut i32, *mut c_int) -> c_int;
type QueryEntrypointsFn = unsafe extern "C" fn(*mut c_void, i32, *mut i32, *mut c_int) -> c_int;
type VendorStringFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;

/// Per-codec encode capability found on a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecSupport {
    /// Codec.
    pub codec: VideoCodec,
    /// Low-power (VDEnc / fixed-function) entry point present.
    pub low_power: bool,
}

/// Probes every render node through `loader`.
pub fn probe(loader: &dyn LibraryLoader) -> EncoderInfo {
    let started = Instant::now();
    if !cfg!(target_os = "linux") {
        return EncoderInfo::unavailable(BACKEND, "VA-API exists only on Linux", started);
    }
    let (va, drm) = match (loader.open(VA_LIBS), loader.open(VA_DRM_LIBS)) {
        (Ok(va), Ok(drm)) => (va, drm),
        (Err(e), _) | (_, Err(e)) => {
            return EncoderInfo::unavailable(BACKEND, format!("libva not found ({e})"), started)
        }
    };
    let mut errors = Vec::new();
    for n in 128..136 {
        let node = format!("/dev/dri/renderD{n}");
        let Some(fd) = loader.open_render_node(&node) else {
            continue;
        };
        let result = unsafe { probe_node(va.as_ref(), drm.as_ref(), fd) };
        loader.close_fd(fd);
        match result {
            Ok((vendor, support)) if !support.is_empty() => {
                let mut limitations =
                    vec!["encode session not implemented in this build (probe only); selection skips VA-API".into()];
                for s in &support {
                    if !s.low_power {
                        limitations
                            .push(format!("{} has no low-power entry point", s.codec.as_str()));
                    }
                }
                return EncoderInfo {
                    backend: BACKEND,
                    codecs: support.iter().map(|s| s.codec).collect(),
                    hardware: true,
                    max_width: 4096,
                    max_height: 4096,
                    latency_class: LatencyClass::Hardware,
                    limitations,
                    status: ProbeStatus::DetectedOnly,
                    device: Some(format!("{node}: {vendor}")),
                    probe_ms: started.elapsed().as_millis() as u32,
                };
            }
            Ok((vendor, _)) => errors.push(format!("{node} ({vendor}): no encode entry points")),
            Err(e) => errors.push(format!("{node}: {e}")),
        }
    }
    let reason = if errors.is_empty() {
        "no /dev/dri/renderD* node could be opened".to_owned()
    } else {
        errors.join("; ")
    };
    EncoderInfo::unavailable(BACKEND, reason, started)
}

unsafe fn probe_node(
    va: &dyn Library,
    drm: &dyn Library,
    fd: i32,
) -> Result<(String, Vec<CodecSupport>), String> {
    unsafe {
        let get_display: GetDisplayDrmFn = resolve(drm, "vaGetDisplayDRM")?;
        let initialize: InitializeFn = resolve(va, "vaInitialize")?;
        let terminate: TerminateFn = resolve(va, "vaTerminate")?;
        let max_profiles: MaxNumFn = resolve(va, "vaMaxNumProfiles")?;
        let max_entrypoints: MaxNumFn = resolve(va, "vaMaxNumEntrypoints")?;
        let query_profiles: QueryProfilesFn = resolve(va, "vaQueryConfigProfiles")?;
        let query_entrypoints: QueryEntrypointsFn = resolve(va, "vaQueryConfigEntrypoints")?;
        let vendor_string: VendorStringFn = resolve(va, "vaQueryVendorString")?;

        let display = get_display(fd);
        if display.is_null() {
            return Err("vaGetDisplayDRM returned null".into());
        }
        let (mut major, mut minor) = (0, 0);
        let status = initialize(display, &mut major, &mut minor);
        if status != 0 {
            return Err(format!("vaInitialize returned {status}"));
        }
        let vendor = {
            let p = vendor_string(display);
            if p.is_null() {
                format!("VA-API {major}.{minor}")
            } else {
                format!(
                    "{} (VA-API {major}.{minor})",
                    CStr::from_ptr(p).to_string_lossy()
                )
            }
        };
        let mut profiles = vec![0i32; max_profiles(display).max(0) as usize];
        let mut n = 0;
        let status = query_profiles(display, profiles.as_mut_ptr(), &mut n);
        profiles.truncate(n.max(0) as usize);
        let mut support: Vec<CodecSupport> = Vec::new();
        if status == 0 {
            let mut entrypoints = vec![0i32; max_entrypoints(display).max(1) as usize];
            for profile in profiles {
                let codec = match profile {
                    PROFILE_H264_MAIN | PROFILE_H264_HIGH | PROFILE_H264_CONSTRAINED_BASELINE => {
                        VideoCodec::H264
                    }
                    PROFILE_HEVC_MAIN => VideoCodec::Hevc,
                    PROFILE_AV1_PROFILE0 => VideoCodec::Av1,
                    _ => continue,
                };
                let mut m = 0;
                if query_entrypoints(display, profile, entrypoints.as_mut_ptr(), &mut m) != 0 {
                    continue;
                }
                let eps = &entrypoints[..m.max(0) as usize];
                let lp = eps.contains(&ENTRYPOINT_ENC_SLICE_LP);
                if !(lp || eps.contains(&ENTRYPOINT_ENC_SLICE)) {
                    continue;
                }
                match support.iter_mut().find(|s| s.codec == codec) {
                    Some(existing) => existing.low_power |= lp,
                    None => support.push(CodecSupport {
                        codec,
                        low_power: lp,
                    }),
                }
            }
        }
        terminate(display);
        support.sort_by_key(|s| s.codec);
        Ok((vendor, support))
    }
}

#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::super::dynlib::FakeLoader;
    use super::*;

    #[test]
    fn missing_libva_is_unavailable() {
        let info = probe(&FakeLoader::default());
        assert!(matches!(info.status, ProbeStatus::Unavailable { .. }));
    }

    #[cfg(target_os = "linux")]
    use fake_driver::*;
    #[cfg(target_os = "linux")]
    mod fake_driver {
        use super::*;
        // A fake driver with H.264 (low-power) and HEVC (full-power only).
        unsafe extern "C" fn get_display(fd: c_int) -> *mut c_void {
            (fd as usize) as *mut c_void
        }
        pub unsafe extern "C" fn initialize(
            _d: *mut c_void,
            a: *mut c_int,
            b: *mut c_int,
        ) -> c_int {
            unsafe {
                *a = 1;
                *b = 20;
            }
            0
        }
        unsafe extern "C" fn terminate(_d: *mut c_void) -> c_int {
            0
        }
        unsafe extern "C" fn max_n(_d: *mut c_void) -> c_int {
            8
        }
        unsafe extern "C" fn profiles(_d: *mut c_void, out: *mut i32, n: *mut c_int) -> c_int {
            let list = [PROFILE_H264_MAIN, PROFILE_HEVC_MAIN, 0];
            unsafe {
                for (i, p) in list.iter().enumerate() {
                    *out.add(i) = *p;
                }
                *n = list.len() as c_int;
            }
            0
        }
        unsafe extern "C" fn entrypoints(
            _d: *mut c_void,
            profile: i32,
            out: *mut i32,
            n: *mut c_int,
        ) -> c_int {
            let list: &[i32] = if profile == PROFILE_H264_MAIN {
                &[1, ENTRYPOINT_ENC_SLICE_LP]
            } else if profile == PROFILE_HEVC_MAIN {
                &[1, ENTRYPOINT_ENC_SLICE]
            } else {
                &[1]
            };
            unsafe {
                for (i, p) in list.iter().enumerate() {
                    *out.add(i) = *p;
                }
                *n = list.len() as c_int;
            }
            0
        }
        unsafe extern "C" fn vendor(_d: *mut c_void) -> *const c_char {
            c"Fake Intel iHD driver".as_ptr()
        }
        pub unsafe extern "C" fn failing_initialize(
            _d: *mut c_void,
            _a: *mut c_int,
            _b: *mut c_int,
        ) -> c_int {
            -1
        }

        pub fn fake(init: usize, nodes: &[&str]) -> FakeLoader {
            let mut loader = FakeLoader::default()
                .with_library(
                    "libva.so.2",
                    &[
                        ("vaInitialize", init),
                        ("vaTerminate", terminate as *const () as usize),
                        ("vaMaxNumProfiles", max_n as *const () as usize),
                        ("vaMaxNumEntrypoints", max_n as *const () as usize),
                        ("vaQueryConfigProfiles", profiles as *const () as usize),
                        (
                            "vaQueryConfigEntrypoints",
                            entrypoints as *const () as usize,
                        ),
                        ("vaQueryVendorString", vendor as *const () as usize),
                    ],
                )
                .with_library(
                    "libva-drm.so.2",
                    &[("vaGetDisplayDRM", get_display as *const () as usize)],
                );
            loader.render_nodes = nodes.iter().map(|s| (*s).to_owned()).collect();
            loader
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn fake_driver_reports_codecs_and_low_power() {
        let info = probe(&fake(
            initialize as *const () as usize,
            &["/dev/dri/renderD128"],
        ));
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
        assert_eq!(info.codecs, vec![VideoCodec::H264, VideoCodec::Hevc]);
        assert!(info.device.as_deref().unwrap().contains("Fake Intel"));
        assert!(info
            .limitations
            .iter()
            .any(|l| l.contains("hevc has no low-power")));
        assert!(!info
            .limitations
            .iter()
            .any(|l| l.contains("h264 has no low-power")));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn node_present_but_driver_broken_is_unavailable() {
        // Research note: a /dev/dri node can exist with no working driver.
        let info = probe(&fake(
            failing_initialize as *const () as usize,
            &["/dev/dri/renderD128"],
        ));
        match info.status {
            ProbeStatus::Unavailable { reason } => {
                assert!(reason.contains("vaInitialize returned -1"), "{reason}")
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn no_render_nodes_is_unavailable() {
        let info = probe(&fake(initialize as *const () as usize, &[]));
        assert!(matches!(info.status, ProbeStatus::Unavailable { .. }));
    }

    #[test]
    fn vaapi_real_hardware_probe() {
        if std::env::var_os("CUA_CODEC_TEST_VAAPI").is_none() {
            eprintln!(
                "skipped: set CUA_CODEC_TEST_VAAPI=1 on a host with /dev/dri and a VA driver"
            );
            return;
        }
        let info = probe(&super::super::dynlib::SystemLoader);
        eprintln!("{info:#?}");
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
    }
}
