// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The rcdp wire v2 media plane, independent of any socket library.
//!
//! [`MediaRuntime`] owns every media session of a driver. `StreamService`
//! (gRPC) creates sessions with [`MediaRuntime::open`]; the `/media`
//! WebSocket and the QUIC listener attach sockets with
//! [`MediaSession::attach`] and pump [`Viewer::next_outbound`] /
//! [`Viewer::handle`]. See `libs/cua/proto/MEDIA.md`.

pub mod audio;
pub mod clock;
pub mod encoder;
pub mod leases;
pub mod rate;
mod session;
mod viewer;

pub use session::{
    AudioPreferenceUpdate, AudioRequest, MediaError, MediaPrincipal, MediaProviders, MediaRuntime,
    MediaSession, MediaTargetSpec, NegotiatedAudioInfo, NegotiatedTrack, OpenMediaParams,
    OpenedMedia, UplinkGrant, UplinkRequest, VideoPacket, DEFAULT_BITRATE_KBPS, DEFAULT_MAX_FPS,
};
pub use viewer::{Outbound, Viewer};

#[cfg(test)]
mod tests;
