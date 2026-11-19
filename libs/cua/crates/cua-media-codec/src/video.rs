// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Encoder and decoder traits implemented by every backend.

use crate::error::Result;
use crate::types::{
    AccessUnit, Backend, DecodedFrame, EncoderConfig, Reconfigured, VideoCodec, VideoFrame,
};

/// A low-latency video encoder session.
///
/// Contract shared by every backend:
/// - No B-frames and no reordering: each `encode` call returns the access
///   unit for that frame (or nothing if the backend dropped it for rate
///   control), never a later one.
/// - The first access unit of every codec epoch is a keyframe carrying
///   in-band parameter sets (SPS/PPS[/VPS] or an AV1 sequence header).
/// - Codec epochs start at 1 and only increase.
pub trait VideoEncoder: Send {
    /// Backend implementing this session.
    fn backend(&self) -> Backend;

    /// Active configuration.
    fn config(&self) -> &EncoderConfig;

    /// Current codec epoch.
    fn codec_epoch(&self) -> u64;

    /// Applies a new configuration. Bitrate, fps and rate control changes
    /// are applied in place where the backend supports it; anything else
    /// rebuilds the session and bumps the codec epoch.
    fn reconfigure(&mut self, config: &EncoderConfig) -> Result<Reconfigured>;

    /// Makes the next encoded frame an IDR (with parameter sets).
    fn force_keyframe(&mut self);

    /// Encodes one frame. Frames whose size differs from the configuration
    /// trigger a restart (new epoch) first.
    fn encode(&mut self, frame: &VideoFrame<'_>) -> Result<Vec<AccessUnit>>;

    /// Drains any pending output (no-op for synchronous backends).
    fn flush(&mut self) -> Result<Vec<AccessUnit>> {
        Ok(Vec::new())
    }
}

/// A video decoder (native clients and tests).
pub trait VideoDecoder: Send {
    /// Backend implementing this decoder.
    fn backend(&self) -> Backend;

    /// Codec it decodes.
    fn codec(&self) -> VideoCodec;

    /// Decodes one access unit. Returns `Ok(None)` while the decoder is
    /// waiting for a keyframe or buffering.
    fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Option<DecodedFrame>>;

    /// Drops all state; the next frame must be a keyframe (call on a codec
    /// epoch change).
    fn reset(&mut self) -> Result<()>;

    /// Whether frames are decoded in hardware: `Some(false)` for software
    /// backends, `None` while unknown (no session yet).
    fn is_hardware(&self) -> Option<bool> {
        None
    }
}

/// Shared epoch/keyframe bookkeeping used by the backends.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct EpochState {
    pub epoch: u64,
    pub force_keyframe: bool,
}

#[allow(dead_code)]
impl EpochState {
    pub fn new() -> Self {
        Self {
            epoch: 1,
            force_keyframe: true,
        }
    }

    pub fn restart(&mut self) -> u64 {
        self.epoch += 1;
        self.force_keyframe = true;
        self.epoch
    }
}
