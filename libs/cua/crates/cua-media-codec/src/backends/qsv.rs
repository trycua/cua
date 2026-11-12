// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Intel Quick Sync probe through the oneVPL dispatcher (MIT; loaded
//! dynamically).
//!
//! Written from the public oneVPL specification: the dispatcher
//! (`libvpl.so.2` / `libvpl.dll`) exposes `MFXLoad`, `MFXCreateConfig`,
//! `MFXSetConfigFilterProperty`, `MFXCreateSession`, `MFXClose` and
//! `MFXUnload`. For each codec the probe builds a loader filtered to a
//! hardware implementation that advertises an encoder for that codec and
//! tries to create a session.
//!
//! Encode sessions are not implemented yet (probe only). Real-hardware lane:
//! `CUA_CODEC_TEST_QSV=1`.

use std::ffi::c_void;
use std::time::Instant;

use super::dynlib::{resolve, Library, LibraryLoader};
use crate::probe::{EncoderInfo, ProbeStatus};
use crate::types::{Backend, LatencyClass, VideoCodec};

const BACKEND: Backend = Backend::Qsv;

/// Dispatcher library names.
pub const VPL_LIBS: &[&str] = if cfg!(windows) {
    &["libvpl.dll"]
} else {
    &["libvpl.so.2", "libvpl.so"]
};

const MFX_IMPL_TYPE_HARDWARE: u32 = 0x0002;
const MFX_VARIANT_TYPE_U32: u32 = 5;
const MFX_VARIANT_VERSION: u16 = 0x0100; // major 1, minor 0

const fn fourcc(s: &[u8; 4]) -> u32 {
    (s[0] as u32) | ((s[1] as u32) << 8) | ((s[2] as u32) << 16) | ((s[3] as u32) << 24)
}

/// oneVPL codec ids.
pub const CODEC_IDS: [(VideoCodec, u32); 3] = [
    (VideoCodec::H264, fourcc(b"AVC ")),
    (VideoCodec::Hevc, fourcc(b"HEVC")),
    (VideoCodec::Av1, fourcc(b"AV1 ")),
];

#[allow(missing_docs)]
/// `mfxVariant`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Variant {
    pub version: u16,
    pub pad: u16,
    pub ty: u32,
    pub data: u64,
}

type LoadFn = unsafe extern "C" fn() -> *mut c_void;
type UnloadFn = unsafe extern "C" fn(*mut c_void);
type CreateConfigFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type SetFilterFn = unsafe extern "C" fn(*mut c_void, *const u8, Variant) -> i32;
type CreateSessionFn = unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
type CloseFn = unsafe extern "C" fn(*mut c_void) -> i32;

struct Api {
    load: LoadFn,
    unload: UnloadFn,
    create_config: CreateConfigFn,
    set_filter: SetFilterFn,
    create_session: CreateSessionFn,
    close: CloseFn,
}

impl Api {
    unsafe fn new(lib: &dyn Library) -> Result<Self, String> {
        unsafe {
            Ok(Self {
                load: resolve(lib, "MFXLoad")?,
                unload: resolve(lib, "MFXUnload")?,
                create_config: resolve(lib, "MFXCreateConfig")?,
                set_filter: resolve(lib, "MFXSetConfigFilterProperty")?,
                create_session: resolve(lib, "MFXCreateSession")?,
                close: resolve(lib, "MFXClose")?,
            })
        }
    }

    unsafe fn filter(&self, loader: *mut c_void, name: &[u8], value: u32) -> Result<(), String> {
        unsafe {
            let cfg = (self.create_config)(loader);
            if cfg.is_null() {
                return Err("MFXCreateConfig returned null".into());
            }
            let v = Variant {
                version: MFX_VARIANT_VERSION,
                pad: 0,
                ty: MFX_VARIANT_TYPE_U32,
                data: u64::from(value),
            };
            let status = (self.set_filter)(cfg, name.as_ptr(), v);
            if status != 0 {
                return Err(format!("MFXSetConfigFilterProperty returned {status}"));
            }
            Ok(())
        }
    }

    /// True if a hardware implementation with an encoder for `codec_id`
    /// accepts a session.
    unsafe fn has_encoder(&self, codec_id: u32) -> Result<bool, String> {
        unsafe {
            let loader = (self.load)();
            if loader.is_null() {
                return Err("MFXLoad returned null".into());
            }
            let result = (|| {
                self.filter(loader, b"mfxImplDescription.Impl\0", MFX_IMPL_TYPE_HARDWARE)?;
                self.filter(
                    loader,
                    b"mfxImplDescription.mfxEncoderDescription.encoder.CodecID\0",
                    codec_id,
                )?;
                let mut session = std::ptr::null_mut();
                let status = (self.create_session)(loader, 0, &mut session);
                if status == 0 && !session.is_null() {
                    (self.close)(session);
                    Ok(true)
                } else {
                    Ok(false)
                }
            })();
            (self.unload)(loader);
            result
        }
    }
}

/// Probes Quick Sync through `loader`.
pub fn probe(loader: &dyn LibraryLoader) -> EncoderInfo {
    let started = Instant::now();
    if !(cfg!(target_os = "linux") || cfg!(windows)) {
        return EncoderInfo::unavailable(
            BACKEND,
            "Quick Sync exists only on Linux and Windows",
            started,
        );
    }
    let lib = match loader.open(VPL_LIBS) {
        Ok(lib) => lib,
        Err(e) => {
            return EncoderInfo::unavailable(
                BACKEND,
                format!("oneVPL dispatcher not found ({e})"),
                started,
            )
        }
    };
    let api = match unsafe { Api::new(lib.as_ref()) } {
        Ok(api) => api,
        Err(e) => return EncoderInfo::unavailable(BACKEND, e, started),
    };
    let mut codecs = Vec::new();
    let mut errors = Vec::new();
    for (codec, id) in CODEC_IDS {
        match unsafe { api.has_encoder(id) } {
            Ok(true) => codecs.push(codec),
            Ok(false) => {}
            Err(e) => errors.push(e),
        }
    }
    if codecs.is_empty() {
        let reason = if errors.is_empty() {
            "no hardware oneVPL implementation with an encoder".to_owned()
        } else {
            errors.join("; ")
        };
        return EncoderInfo::unavailable(BACKEND, reason, started);
    }
    EncoderInfo {
        backend: BACKEND,
        codecs,
        hardware: true,
        max_width: 4096,
        max_height: 4096,
        latency_class: LatencyClass::Hardware,
        limitations: vec![
            "encode session not implemented in this build (probe only); selection skips QSV".into(),
            "CBR is emulated with VBR on some drivers".into(),
        ],
        status: ProbeStatus::DetectedOnly,
        device: Some(format!("oneVPL via {}", lib.name())),
        probe_ms: started.elapsed().as_millis() as u32,
    }
}

#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::dynlib::FakeLoader;
    use super::*;

    static LAST_CODEC: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn load() -> *mut c_void {
        8 as *mut c_void
    }
    unsafe extern "C" fn unload(_l: *mut c_void) {}
    unsafe extern "C" fn create_config(_l: *mut c_void) -> *mut c_void {
        16 as *mut c_void
    }
    unsafe extern "C" fn set_filter(_c: *mut c_void, name: *const u8, v: Variant) -> i32 {
        let name = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
        assert_eq!(v.version, MFX_VARIANT_VERSION);
        assert_eq!(v.ty, MFX_VARIANT_TYPE_U32);
        if name.ends_with("CodecID") {
            LAST_CODEC.store(v.data as u32, Ordering::SeqCst);
        }
        0
    }
    // Only H.264 is "supported" by this fake GPU.
    unsafe extern "C" fn create_session(_l: *mut c_void, _i: u32, s: *mut *mut c_void) -> i32 {
        if LAST_CODEC.load(Ordering::SeqCst) == fourcc(b"AVC ") {
            unsafe { *s = 32 as *mut c_void };
            0
        } else {
            -9 // MFX_ERR_NOT_FOUND
        }
    }
    unsafe extern "C" fn close(_s: *mut c_void) -> i32 {
        0
    }

    #[test]
    fn missing_dispatcher_is_unavailable() {
        assert!(matches!(
            probe(&FakeLoader::default()).status,
            ProbeStatus::Unavailable { .. }
        ));
    }

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn fake_dispatcher_reports_supported_codecs() {
        let loader = FakeLoader::default().with_library(
            VPL_LIBS[0],
            &[
                ("MFXLoad", load as *const () as usize),
                ("MFXUnload", unload as *const () as usize),
                ("MFXCreateConfig", create_config as *const () as usize),
                (
                    "MFXSetConfigFilterProperty",
                    set_filter as *const () as usize,
                ),
                ("MFXCreateSession", create_session as *const () as usize),
                ("MFXClose", close as *const () as usize),
            ],
        );
        let info = probe(&loader);
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
        assert_eq!(info.codecs, vec![VideoCodec::H264]);
    }

    #[test]
    fn variant_layout_matches_the_spec() {
        assert_eq!(std::mem::size_of::<Variant>(), 16);
        assert_eq!(std::mem::offset_of!(Variant, ty), 4);
        assert_eq!(std::mem::offset_of!(Variant, data), 8);
    }

    #[test]
    fn qsv_real_hardware_probe() {
        if std::env::var_os("CUA_CODEC_TEST_QSV").is_none() {
            eprintln!("skipped: set CUA_CODEC_TEST_QSV=1 on an Intel host with oneVPL");
            return;
        }
        let info = probe(&super::super::dynlib::SystemLoader);
        eprintln!("{info:#?}");
        assert!(info.codecs.contains(&VideoCodec::H264));
    }
}
