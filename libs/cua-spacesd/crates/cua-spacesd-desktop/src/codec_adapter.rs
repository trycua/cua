// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Adapters between `cua-media-codec` and the capture-provider API
//! (`cua-spacesd-provider-api`), so the session runtime can use any backend
//! without depending on codec internals.
//!
//! How to plug in (for `cua-spacesd-session`'s own `FrameEncoder` trait):
//!
//! ```ignore
//! struct SessionEncoder(cua_media_codec::adapter::CodecFrameEncoder);
//! impl cua_spacesd_session::FrameEncoder for SessionEncoder {
//!     fn encode(&mut self, f: &OwnedFrame) -> Result<Option<OwnedFrame>, ProviderError> {
//!         self.0.encode_frame(f).map_err(into_provider_error)
//!     }
//!     fn request_keyframe(&mut self) { self.0.request_keyframe() }
//! }
//! ```
//!
//! Or wrap a capture sink directly with [`EncodingCaptureSink`], which
//! turns raw `Bgra8` frames into `H264AnnexB` frames on a worker thread.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use cua_spacesd_provider_api::{
    CaptureEvent, CaptureSink, OwnedFrame, PixelFormat as ProviderPixelFormat,
};

use cua_media_codec::error::{CodecError, Result};
use cua_media_codec::probe::EncoderInfo;
use cua_media_codec::select::{select, ClientDecoder, FallbackEncoder, SelectionPrefs};
use cua_media_codec::types::{AccessUnit, Backend, EncoderConfig, VideoCodec, VideoFrame};
use cua_media_codec::video::VideoEncoder;

/// The minimal encoder interface this crate publishes for session code.
pub trait FrameEncoder: Send {
    /// Encodes one raw frame; `None` when the encoder dropped it.
    fn encode_frame(&mut self, frame: &OwnedFrame) -> Result<Option<OwnedFrame>>;
    /// Makes the next output a keyframe.
    fn request_keyframe(&mut self);
    /// Live bitrate / fps change (may restart the session: new epoch).
    fn set_rate(&mut self, bitrate_kbps: u32, fps: u32) -> Result<()>;
    /// Backend in use.
    fn backend(&self) -> Backend;
}

/// A [`FrameEncoder`] over a [`FallbackEncoder`] (H.264 output).
pub struct CodecFrameEncoder {
    inner: FallbackEncoder,
}

impl CodecFrameEncoder {
    /// Selects the best H.264 encoder from `encoders` for a client with
    /// `client` decoders and opens it at `width`x`height`.
    pub fn open(
        encoders: &[EncoderInfo],
        client: &[ClientDecoder],
        prefs: &SelectionPrefs,
        config: &EncoderConfig,
    ) -> Result<Self> {
        let choice = select(encoders, client, prefs)?;
        Ok(Self {
            inner: FallbackEncoder::open(&choice, config)?,
        })
    }

    /// Wraps an existing fallback encoder.
    pub fn from_encoder(inner: FallbackEncoder) -> Self {
        Self { inner }
    }

    /// The underlying encoder.
    pub fn encoder(&mut self) -> &mut FallbackEncoder {
        &mut self.inner
    }
}

/// Converts an access unit to a provider frame.
pub fn access_unit_to_frame(au: AccessUnit) -> Result<OwnedFrame> {
    if au.codec != VideoCodec::H264 {
        return Err(CodecError::Unsupported(format!(
            "provider API carries H.264 only, got {}",
            au.codec.as_str()
        )));
    }
    Ok(OwnedFrame {
        bytes: au.data.into(),
        format: ProviderPixelFormat::H264AnnexB,
        width_px: au.width,
        height_px: au.height,
        bytes_per_row: None,
        capture_timestamp_us: au.pts_us,
        encode_duration_us: Some(au.encode_duration_us),
        codec_epoch: au.codec_epoch,
        keyframe: au.keyframe,
    })
}

impl FrameEncoder for CodecFrameEncoder {
    fn encode_frame(&mut self, frame: &OwnedFrame) -> Result<Option<OwnedFrame>> {
        if frame.format != ProviderPixelFormat::Bgra8 {
            return Err(CodecError::InvalidArgument(
                "CodecFrameEncoder needs Bgra8 input".into(),
            ));
        }
        let stride = frame.bytes_per_row.unwrap_or(frame.width_px * 4) as usize;
        let vf = VideoFrame {
            width: frame.width_px,
            height: frame.height_px,
            pts_us: frame.capture_timestamp_us,
            data: cua_media_codec::types::FrameData::Bgra {
                data: &frame.bytes,
                stride,
            },
        };
        let mut units = self.inner.encode(&vf)?;
        units.pop().map(access_unit_to_frame).transpose()
    }

    fn request_keyframe(&mut self) {
        self.inner.force_keyframe();
    }

    fn set_rate(&mut self, bitrate_kbps: u32, fps: u32) -> Result<()> {
        let mut cfg = self.inner.config().clone();
        cfg.bitrate_kbps = bitrate_kbps;
        cfg.fps = fps;
        self.inner.reconfigure(&cfg).map(|_| ())
    }

    fn backend(&self) -> Backend {
        self.inner.backend()
    }
}

#[derive(Default)]
struct Mailbox {
    latest: Option<OwnedFrame>,
    last: Option<OwnedFrame>,
    keyframe: bool,
    closed: bool,
}

/// A `CaptureSink` that encodes `Bgra8` frames and forwards encoded frames
/// (and all non-frame events) to `downstream`. Frames that arrive while the
/// encoder is busy replace the pending one (latest-frame semantics).
pub struct EncodingCaptureSink {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    downstream: Arc<dyn CaptureSink>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl EncodingCaptureSink {
    /// Starts the worker.
    pub fn start(
        mut encoder: Box<dyn FrameEncoder>,
        downstream: Arc<dyn CaptureSink>,
    ) -> Arc<Self> {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker_shared = shared.clone();
        let worker_sink = downstream.clone();
        let worker = std::thread::Builder::new()
            .name("cua-codec-encode".into())
            .spawn(move || loop {
                let (frame, force) = {
                    let (lock, cv) = &*worker_shared;
                    let mut mb = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    while mb.latest.is_none() && !mb.closed {
                        mb = cv
                            .wait(mb)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    if mb.closed {
                        return;
                    }
                    (
                        mb.latest.take().expect("frame"),
                        std::mem::take(&mut mb.keyframe),
                    )
                };
                if force {
                    encoder.request_keyframe();
                }
                match encoder.encode_frame(&frame) {
                    Ok(Some(encoded)) => worker_sink.on_event(CaptureEvent::Frame(encoded)),
                    Ok(None) => {}
                    Err(e) => worker_sink
                        .on_event(CaptureEvent::Suspended(format!("encoder failed: {e}"))),
                }
            })
            .expect("spawn encoder worker");
        Arc::new(Self {
            shared,
            downstream,
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Forces a keyframe, re-encoding the last frame if the source is idle.
    pub fn request_keyframe(&self) {
        let (lock, cv) = &*self.shared;
        let mut mb = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mb.keyframe = true;
        if mb.latest.is_none() {
            mb.latest = mb.last.clone();
        }
        cv.notify_one();
    }

    /// Stops the worker.
    pub fn stop(&self) {
        {
            let (lock, cv) = &*self.shared;
            let mut mb = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            mb.closed = true;
            cv.notify_one();
        }
        if let Some(w) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = w.join();
        }
    }
}

impl Drop for EncodingCaptureSink {
    fn drop(&mut self) {
        self.stop();
    }
}

impl CaptureSink for EncodingCaptureSink {
    fn on_event(&self, event: CaptureEvent) {
        match event {
            CaptureEvent::Frame(frame) if frame.format == ProviderPixelFormat::Bgra8 => {
                let (lock, cv) = &*self.shared;
                let mut mb = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if mb.closed {
                    return;
                }
                mb.last = Some(frame.clone());
                // A replaced raw frame was never seen by the encoder, so no
                // keyframe is needed.
                mb.latest = Some(frame);
                cv.notify_one();
            }
            other => self.downstream.on_event(other),
        }
    }
}
