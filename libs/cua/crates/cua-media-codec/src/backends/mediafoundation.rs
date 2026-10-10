// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Windows Media Foundation hardware encoder probe.
//!
//! Enumerates hardware video-encoder MFTs per output subtype with
//! `MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | ...)`.
//! Media Foundation cannot always force an IDR on demand, so it ranks below
//! the vendor SDKs (research notes). The MFT encode session is not
//! implemented yet (probe only). Real-hardware lane: `CUA_CODEC_TEST_MF=1`.

use std::time::Instant;

use crate::probe::{EncoderInfo, ProbeStatus};
use crate::types::{Backend, LatencyClass, VideoCodec};

const BACKEND: Backend = Backend::MediaFoundation;

/// Enumerates hardware encoder MFTs (abstracted so tests can fake it).
pub trait MftEnumerator {
    /// Friendly names of hardware encoder MFTs producing `codec`.
    fn hardware_encoders(&self, codec: VideoCodec) -> Result<Vec<String>, String>;
}

/// Probes Media Foundation with `enumerator`.
pub fn probe_with(enumerator: &dyn MftEnumerator) -> EncoderInfo {
    let started = Instant::now();
    let mut codecs = Vec::new();
    let mut names = Vec::new();
    let mut errors = Vec::new();
    for codec in VideoCodec::ALL {
        match enumerator.hardware_encoders(codec) {
            Ok(found) if !found.is_empty() => {
                codecs.push(codec);
                names.extend(found);
            }
            Ok(_) => {}
            Err(e) => errors.push(e),
        }
    }
    if codecs.is_empty() {
        let reason = if errors.is_empty() {
            "no hardware encoder MFTs registered".to_owned()
        } else {
            errors.join("; ")
        };
        return EncoderInfo::unavailable(BACKEND, reason, started);
    }
    names.dedup();
    EncoderInfo {
        backend: BACKEND,
        codecs,
        hardware: true,
        max_width: 4096,
        max_height: 4096,
        latency_class: LatencyClass::OsHardware,
        limitations: vec![
            "encode session not implemented in this build (probe only); selection skips Media Foundation".into(),
            "some MFTs cannot force an IDR on demand (periodic GOP required)".into(),
        ],
        status: ProbeStatus::DetectedOnly,
        device: Some(names.join(", ")),
        probe_ms: started.elapsed().as_millis() as u32,
    }
}

/// Probes the real Media Foundation (Windows) or reports unavailable.
pub fn probe() -> EncoderInfo {
    #[cfg(windows)]
    {
        probe_with(&windows_impl::SystemMft)
    }
    #[cfg(not(windows))]
    {
        EncoderInfo::unavailable(
            BACKEND,
            "Media Foundation exists only on Windows",
            Instant::now(),
        )
    }
}

#[cfg(windows)]
mod windows_impl {
    use windows::core::{GUID, PWSTR};
    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx,
        MFT_FRIENDLY_NAME_Attribute, MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC,
        MFSTARTUP_LITE, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE,
        MFT_ENUM_FLAG_SORTANDFILTER, MFT_REGISTER_TYPE_INFO,
    };
    use windows::Win32::System::Com::CoTaskMemFree;

    use super::MftEnumerator;
    use crate::types::VideoCodec;

    const MF_VERSION: u32 = 0x0002_0070;

    pub struct SystemMft;

    fn subtype(codec: VideoCodec) -> GUID {
        match codec {
            VideoCodec::H264 => MFVideoFormat_H264,
            VideoCodec::Hevc => MFVideoFormat_HEVC,
            VideoCodec::Av1 => MFVideoFormat_AV1,
        }
    }

    impl MftEnumerator for SystemMft {
        fn hardware_encoders(&self, codec: VideoCodec) -> Result<Vec<String>, String> {
            unsafe {
                MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| format!("MFStartup: {e}"))?;
                let output = MFT_REGISTER_TYPE_INFO {
                    guidMajorType: MFMediaType_Video,
                    guidSubtype: subtype(codec),
                };
                let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
                let mut count = 0u32;
                let flags = MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0);
                let result = MFTEnumEx(
                    MFT_CATEGORY_VIDEO_ENCODER,
                    flags,
                    None,
                    Some(&output),
                    &mut activates,
                    &mut count,
                );
                let mut names = Vec::new();
                if result.is_ok() && !activates.is_null() {
                    for i in 0..count as usize {
                        if let Some(activate) = (*activates.add(i)).take() {
                            let mut name = PWSTR::null();
                            let mut len = 0u32;
                            if activate
                                .GetAllocatedString(
                                    &MFT_FRIENDLY_NAME_Attribute,
                                    &mut name,
                                    &mut len,
                                )
                                .is_ok()
                            {
                                names.push(name.to_string().unwrap_or_default());
                                CoTaskMemFree(Some(name.0 as *const _));
                            } else {
                                names.push("hardware MFT".into());
                            }
                        }
                    }
                    CoTaskMemFree(Some(activates as *const _));
                }
                let _ = MFShutdown();
                result.map_err(|e| format!("MFTEnumEx: {e}"))?;
                Ok(names)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Vec<(VideoCodec, &'static str)>);
    impl MftEnumerator for Fake {
        fn hardware_encoders(&self, codec: VideoCodec) -> Result<Vec<String>, String> {
            Ok(self
                .0
                .iter()
                .filter(|(c, _)| *c == codec)
                .map(|(_, n)| (*n).to_owned())
                .collect())
        }
    }

    #[test]
    fn fake_mfts_are_reported_as_os_hardware() {
        let info = probe_with(&Fake(vec![
            (VideoCodec::H264, "NVIDIA H.264 Encoder MFT"),
            (VideoCodec::Hevc, "NVIDIA HEVC Encoder MFT"),
        ]));
        assert_eq!(info.status, ProbeStatus::DetectedOnly);
        assert_eq!(info.codecs, vec![VideoCodec::H264, VideoCodec::Hevc]);
        assert_eq!(info.latency_class, LatencyClass::OsHardware);
    }

    #[test]
    fn no_mfts_is_unavailable() {
        assert!(matches!(
            probe_with(&Fake(vec![])).status,
            ProbeStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn mf_real_hardware_probe() {
        if std::env::var_os("CUA_CODEC_TEST_MF").is_none() {
            eprintln!("skipped: set CUA_CODEC_TEST_MF=1 on a Windows host with a GPU");
            return;
        }
        let info = probe();
        eprintln!("{info:#?}");
        assert!(info.codecs.contains(&VideoCodec::H264));
    }
}
