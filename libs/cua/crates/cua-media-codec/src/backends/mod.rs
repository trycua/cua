// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Backend implementations and the factory that opens them.

pub mod dynlib;

#[cfg(feature = "openh264")]
pub mod openh264;

#[cfg(target_os = "macos")]
pub mod videotoolbox;

#[cfg(feature = "nvenc")]
pub mod nvenc;

#[cfg(feature = "vaapi")]
pub mod vaapi;

#[cfg(feature = "qsv")]
pub mod qsv;

#[cfg(feature = "amf")]
pub mod amf;

#[cfg(feature = "mediafoundation")]
pub mod mediafoundation;

#[cfg(any(feature = "av1-rav1e", feature = "av1-dav1d"))]
pub mod av1;

use crate::error::{CodecError, Result};
use crate::types::{Backend, EncoderConfig, VideoCodec};
use crate::video::{VideoDecoder, VideoEncoder};

/// Opens encoder sessions by backend. [`crate::select::FallbackEncoder`]
/// uses this indirection so tests can inject failing backends.
pub trait EncoderFactory: Send + Sync {
    /// Opens `backend` with `config`.
    fn open(&self, backend: Backend, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>>;
}

/// The factory for the backends compiled into this build.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultFactory;

impl EncoderFactory for DefaultFactory {
    fn open(&self, backend: Backend, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>> {
        open_encoder(backend, config)
    }
}

/// Opens an encoder session on `backend`.
#[allow(unused_variables)]
pub fn open_encoder(backend: Backend, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>> {
    match backend {
        #[cfg(feature = "openh264")]
        Backend::OpenH264 => Ok(Box::new(openh264::OpenH264Encoder::new(config)?)),
        #[cfg(target_os = "macos")]
        Backend::VideoToolbox => Ok(Box::new(videotoolbox::VtEncoder::new(config)?)),
        #[cfg(feature = "av1-rav1e")]
        Backend::Rav1e => Ok(Box::new(av1::Rav1eEncoder::new(config)?)),
        Backend::Nvenc
        | Backend::Vaapi
        | Backend::Qsv
        | Backend::Amf
        | Backend::MediaFoundation => Err(CodecError::unavailable(
            backend,
            "encode session not implemented in this build (probe only)",
        )),
        other => Err(CodecError::unavailable(
            other,
            "not compiled into this build",
        )),
    }
}

/// Opens a decoder for `codec` on `backend`.
pub fn open_decoder(backend: Backend, codec: VideoCodec) -> Result<Box<dyn VideoDecoder>> {
    match (backend, codec) {
        #[cfg(feature = "openh264")]
        (Backend::OpenH264, VideoCodec::H264) => Ok(Box::new(openh264::OpenH264Decoder::new()?)),
        #[cfg(target_os = "macos")]
        (Backend::VideoToolbox, VideoCodec::H264 | VideoCodec::Hevc) => {
            Ok(Box::new(videotoolbox::VtDecoder::new(codec)?))
        }
        #[cfg(feature = "av1-dav1d")]
        (Backend::Dav1d, VideoCodec::Av1) => Ok(Box::new(av1::Dav1dDecoder::new()?)),
        (backend, codec) => Err(CodecError::unavailable(
            backend,
            format!(
                "no {} decoder for this backend in this build",
                codec.as_str()
            ),
        )),
    }
}

/// Opens the best available decoder for `codec` (hardware first).
pub fn open_best_decoder(codec: VideoCodec) -> Result<Box<dyn VideoDecoder>> {
    let order: &[Backend] = &[Backend::VideoToolbox, Backend::OpenH264, Backend::Dav1d];
    let mut errors = Vec::new();
    for backend in order {
        match open_decoder(*backend, codec) {
            Ok(d) => return Ok(d),
            Err(e) => errors.push(e.to_string()),
        }
    }
    Err(CodecError::NoEncoder(format!(
        "no {} decoder: {}",
        codec.as_str(),
        errors.join("; ")
    )))
}
