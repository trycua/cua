// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! NVIDIA NVENC probe (Video Codec SDK, loaded dynamically).
//!
//! Written from NVIDIA's public "NVENC Video Encoder API Programming Guide"
//! and the MIT-licensed `nvEncodeAPI.h` interface description: the runtime
//! is `libnvidia-encode.so.1` (Linux) or `nvEncodeAPI64.dll` (Windows) and
//! exports `NvEncodeAPIGetMaxSupportedVersion` and
//! `NvEncodeAPICreateInstance`. A session needs a CUDA context
//! (`libcuda.so.1` / `nvcuda.dll`).
//!
//! What the probe does, in order:
//! 1. load the runtime and read the driver's maximum API version;
//! 2. load CUDA, create a context on device 0;
//! 3. fill the API function list and open an encode session on the context;
//! 4. enumerate the supported encode GUIDs (H.264, HEVC, AV1);
//! 5. destroy the session and context.
//!
//! The encode session itself (input/bitstream buffers, picture params) is
//! not implemented yet: it needs a GPU runner to validate the struct
//! layouts. Probe results therefore report `ProbeStatus::DetectedOnly`, and
//! selection skips NVENC until the session lands (gated real-hardware lane:
//! `CUA_CODEC_TEST_NVENC=1`).

use std::ffi::c_void;
use std::time::Instant;

use super::dynlib::{resolve, LibraryLoader};
use crate::probe::{EncoderInfo, ProbeStatus};
use crate::types::{Backend, LatencyClass, VideoCodec};

const BACKEND: Backend = Backend::Nvenc;

/// Runtime library names.
pub const RUNTIME_LIBS: &[&str] = if cfg!(windows) {
    &["nvEncodeAPI64.dll"]
} else {
    &["libnvidia-encode.so.1", "libnvidia-encode.so"]
};
/// CUDA driver library names.
pub const CUDA_LIBS: &[&str] = if cfg!(windows) {
    &["nvcuda.dll"]
} else {
    &["libcuda.so.1", "libcuda.so"]
};

/// Encode GUIDs from the public API (`NV_ENC_CODEC_*_GUID`).
pub const CODEC_GUIDS: [(VideoCodec, Guid); 3] = [
    (
        VideoCodec::H264,
        Guid(
            0x6BC8_2762,
            0x4E63,
            0x4CA4,
            [0xAA, 0x85, 0x1E, 0x50, 0xF3, 0x21, 0xF6, 0xBF],
        ),
    ),
    (
        VideoCodec::Hevc,
        Guid(
            0x790C_DC88,
            0x4522,
            0x4D7B,
            [0x94, 0x25, 0xBD, 0xA9, 0x97, 0x5F, 0x76, 0x03],
        ),
    ),
    (
        VideoCodec::Av1,
        Guid(
            0x0A35_2289,
            0x0AA7,
            0x4759,
            [0x86, 0x2D, 0x5D, 0x15, 0xCD, 0x16, 0xD2, 0x54],
        ),
    ),
];

/// A Windows-layout GUID.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

type GetMaxVersionFn = unsafe extern "C" fn(*mut u32) -> i32;
type CreateInstanceFn = unsafe extern "C" fn(*mut FunctionList) -> i32;
type CuInitFn = unsafe extern "C" fn(u32) -> i32;
type CuDeviceGetFn = unsafe extern "C" fn(*mut i32, i32) -> i32;
type CuDeviceGetNameFn = unsafe extern "C" fn(*mut u8, i32, i32) -> i32;
type CuCtxCreateFn = unsafe extern "C" fn(*mut *mut c_void, u32, i32) -> i32;
type CuCtxDestroyFn = unsafe extern "C" fn(*mut c_void) -> i32;
type OpenSessionExFn = unsafe extern "C" fn(*mut OpenSessionExParams, *mut *mut c_void) -> i32;
type GetGuidCountFn = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type GetGuidsFn = unsafe extern "C" fn(*mut c_void, *mut Guid, u32, *mut u32) -> i32;
type DestroyEncoderFn = unsafe extern "C" fn(*mut c_void) -> i32;

#[allow(missing_docs)]
/// `NV_ENCODE_API_FUNCTION_LIST`: version, reserved, then 41 entry points
/// followed by reserved pointers. Only the slots the probe uses are named.
#[repr(C)]
pub struct FunctionList {
    pub version: u32,
    pub reserved: u32,
    pub slots: [*mut c_void; 320],
}

// Slot indices in the function table (declaration order in the API).
const SLOT_GET_ENCODE_GUID_COUNT: usize = 1;
const SLOT_GET_ENCODE_GUIDS: usize = 4;
const SLOT_DESTROY_ENCODER: usize = 27;
const SLOT_OPEN_ENCODE_SESSION_EX: usize = 29;

#[allow(missing_docs)]
/// `NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS`.
#[repr(C)]
pub struct OpenSessionExParams {
    pub version: u32,
    pub device_type: u32,
    pub device: *mut c_void,
    pub reserved: *mut c_void,
    pub api_version: u32,
    pub reserved1: [u32; 253],
    pub reserved2: [*mut c_void; 64],
}

/// `NVENCAPI_VERSION` for a driver-reported `(major << 4) | minor`.
pub fn api_version(max_supported: u32) -> u32 {
    let major = max_supported >> 4;
    let minor = max_supported & 0xf;
    major | (minor << 24)
}

/// `NVENCAPI_STRUCT_VERSION(ver)`.
pub fn struct_version(api: u32, ver: u32) -> u32 {
    api | (ver << 16) | (0x7 << 28)
}

fn unavailable(reason: impl Into<String>, started: Instant) -> EncoderInfo {
    EncoderInfo::unavailable(BACKEND, reason, started)
}

/// Probes NVENC through `loader`.
pub fn probe(loader: &dyn LibraryLoader) -> EncoderInfo {
    let started = Instant::now();
    if !(cfg!(target_os = "linux") || cfg!(windows)) {
        return unavailable("NVENC exists only on Linux and Windows", started);
    }
    let runtime = match loader.open(RUNTIME_LIBS) {
        Ok(lib) => lib,
        Err(e) => return unavailable(format!("NVENC runtime not found ({e})"), started),
    };
    let max_version = unsafe {
        let Ok(get) =
            resolve::<GetMaxVersionFn>(runtime.as_ref(), "NvEncodeAPIGetMaxSupportedVersion")
        else {
            return unavailable("runtime lacks NvEncodeAPIGetMaxSupportedVersion", started);
        };
        let mut v = 0u32;
        let status = get(&mut v);
        if status != 0 {
            return unavailable(
                format!("NvEncodeAPIGetMaxSupportedVersion returned {status}"),
                started,
            );
        }
        v
    };
    let driver_api = format!("{}.{}", max_version >> 4, max_version & 0xf);
    let mut info = EncoderInfo {
        backend: BACKEND,
        codecs: Vec::new(),
        hardware: true,
        max_width: 8192,
        max_height: 8192,
        latency_class: LatencyClass::Hardware,
        limitations: vec![
            "encode session not implemented in this build (probe only); selection skips NVENC"
                .into(),
        ],
        status: ProbeStatus::DetectedOnly,
        device: Some(format!("NVENC API {driver_api} via {}", runtime.name())),
        probe_ms: 0,
    };
    // Session-level probe (needs CUDA).
    match open_session_and_list(loader, runtime.as_ref(), api_version(max_version)) {
        Ok((device_name, codecs)) => {
            info.codecs = codecs;
            if let Some(name) = device_name {
                info.device = Some(format!("{name}, NVENC API {driver_api}"));
            }
        }
        Err(e) => {
            // Without a session we cannot enumerate codecs; assume H.264.
            info.codecs = vec![VideoCodec::H264];
            info.limitations.push(format!("session probe failed: {e}"));
        }
    }
    info.probe_ms = started.elapsed().as_millis() as u32;
    info
}

fn open_session_and_list(
    loader: &dyn LibraryLoader,
    runtime: &dyn super::dynlib::Library,
    api: u32,
) -> Result<(Option<String>, Vec<VideoCodec>), String> {
    let cuda = loader.open(CUDA_LIBS)?;
    unsafe {
        let cu_init: CuInitFn = resolve(cuda.as_ref(), "cuInit")?;
        let cu_device_get: CuDeviceGetFn = resolve(cuda.as_ref(), "cuDeviceGet")?;
        let cu_device_get_name: CuDeviceGetNameFn = resolve(cuda.as_ref(), "cuDeviceGetName")?;
        let cu_ctx_create: CuCtxCreateFn = resolve(cuda.as_ref(), "cuCtxCreate_v2")?;
        let cu_ctx_destroy: CuCtxDestroyFn = resolve(cuda.as_ref(), "cuCtxDestroy_v2")?;
        let create_instance: CreateInstanceFn = resolve(runtime, "NvEncodeAPICreateInstance")?;

        check(cu_init(0), "cuInit")?;
        let mut device = 0i32;
        check(cu_device_get(&mut device, 0), "cuDeviceGet")?;
        let mut name = [0u8; 128];
        let device_name = (cu_device_get_name(name.as_mut_ptr(), name.len() as i32, device) == 0)
            .then(|| {
                let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
                String::from_utf8_lossy(&name[..end]).into_owned()
            });
        let mut ctx = std::ptr::null_mut();
        check(cu_ctx_create(&mut ctx, 0, device), "cuCtxCreate")?;

        let result = (|| {
            let mut list = Box::new(FunctionList {
                version: struct_version(api, 2),
                reserved: 0,
                slots: [std::ptr::null_mut(); 320],
            });
            check(create_instance(&mut *list), "NvEncodeAPICreateInstance")?;
            let slot = |i: usize| -> Result<*mut c_void, String> {
                let p = list.slots[i];
                (!p.is_null())
                    .then_some(p)
                    .ok_or_else(|| format!("function slot {i} is null"))
            };
            let open: OpenSessionExFn = std::mem::transmute(slot(SLOT_OPEN_ENCODE_SESSION_EX)?);
            let guid_count: GetGuidCountFn = std::mem::transmute(slot(SLOT_GET_ENCODE_GUID_COUNT)?);
            let guids_fn: GetGuidsFn = std::mem::transmute(slot(SLOT_GET_ENCODE_GUIDS)?);
            let destroy: DestroyEncoderFn = std::mem::transmute(slot(SLOT_DESTROY_ENCODER)?);
            let mut params = Box::new(OpenSessionExParams {
                version: struct_version(api, 1),
                device_type: 1, // NV_ENC_DEVICE_TYPE_CUDA
                device: ctx,
                reserved: std::ptr::null_mut(),
                api_version: api,
                reserved1: [0; 253],
                reserved2: [std::ptr::null_mut(); 64],
            });
            let mut encoder = std::ptr::null_mut();
            check(open(&mut *params, &mut encoder), "nvEncOpenEncodeSessionEx")?;
            let mut count = 0u32;
            let listed = (|| {
                check(guid_count(encoder, &mut count), "nvEncGetEncodeGUIDCount")?;
                let mut guids = vec![Guid::default(); count as usize];
                let mut n = 0u32;
                check(
                    guids_fn(encoder, guids.as_mut_ptr(), count, &mut n),
                    "nvEncGetEncodeGUIDs",
                )?;
                guids.truncate(n as usize);
                Ok::<_, String>(codecs_from_guids(&guids))
            })();
            destroy(encoder);
            listed
        })();
        cu_ctx_destroy(ctx);
        result.map(|codecs| (device_name, codecs))
    }
}

fn check(status: i32, what: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{what} returned {status}"))
    }
}

/// Maps encode GUIDs to codecs.
pub fn codecs_from_guids(guids: &[Guid]) -> Vec<VideoCodec> {
    CODEC_GUIDS
        .iter()
        .filter(|(_, g)| guids.contains(g))
        .map(|(c, _)| *c)
        .collect()
}

#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::super::dynlib::FakeLoader;
    use super::*;

    #[test]
    fn version_macros_match_the_documented_encoding() {
        // Driver reports 12.2 as 0xC2.
        let api = api_version(0xC2);
        assert_eq!(api, 12 | (2 << 24));
        assert_eq!(struct_version(api, 2), api | (2 << 16) | (7 << 28));
    }

    #[test]
    fn missing_runtime_is_reported_unavailable() {
        let info = probe(&FakeLoader::default());
        assert!(!info.is_usable());
        assert!(matches!(info.status, ProbeStatus::Unavailable { .. }));
    }

    unsafe extern "C" fn fake_max_version(v: *mut u32) -> i32 {
        unsafe { *v = 0xC1 };
        0
    }
    unsafe extern "C" fn fake_failing_version(_v: *mut u32) -> i32 {
        15 // NV_ENC_ERR_INVALID_VERSION
    }

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn runtime_without_cuda_is_detected_but_not_selectable() {
        let loader = FakeLoader::default().with_library(
            RUNTIME_LIBS[0],
            &[(
                "NvEncodeAPIGetMaxSupportedVersion",
                fake_max_version as *const () as usize,
            )],
        );
        let info = probe(&loader);
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
        assert!(!info.is_usable());
        assert!(info.device.as_deref().unwrap().contains("12.1"));
        assert!(info
            .limitations
            .iter()
            .any(|l| l.contains("session probe failed")));
    }

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn failing_version_query_is_unavailable() {
        let loader = FakeLoader::default().with_library(
            RUNTIME_LIBS[0],
            &[(
                "NvEncodeAPIGetMaxSupportedVersion",
                fake_failing_version as *const () as usize,
            )],
        );
        assert!(matches!(
            probe(&loader).status,
            ProbeStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn guid_mapping() {
        let guids = [CODEC_GUIDS[2].1, CODEC_GUIDS[0].1];
        assert_eq!(
            codecs_from_guids(&guids),
            vec![VideoCodec::H264, VideoCodec::Av1]
        );
    }

    /// Real-hardware lane: `CUA_CODEC_TEST_NVENC=1 cargo test -p cua-media-codec nvenc_real`.
    #[test]
    fn nvenc_real_hardware_probe() {
        if std::env::var_os("CUA_CODEC_TEST_NVENC").is_none() {
            eprintln!("skipped: set CUA_CODEC_TEST_NVENC=1 on an NVIDIA host");
            return;
        }
        let info = probe(&super::super::dynlib::SystemLoader);
        eprintln!("{info:#?}");
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
        assert!(info.codecs.contains(&VideoCodec::H264));
        assert!(!info
            .limitations
            .iter()
            .any(|l| l.contains("session probe failed")));
    }
}
