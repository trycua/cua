// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Video and audio codec layer for cua-spacesd (plan §8.3, §8.5).
//!
//! # Video
//! - [`VideoEncoder`] / [`VideoDecoder`]: backend-neutral traits. Every
//!   encoder is low-latency (no B-frames, one access unit per input frame),
//!   emits Annex B with in-band parameter sets on keyframes (AV1: sequence
//!   header), and tags output with a monotonically increasing codec epoch.
//! - [`probe::probe`] / [`probe::probe_with`]: runtime probing by a real
//!   test encode (or driver/session query), optionally in an isolated child
//!   process per backend so a crashing driver cannot take down the daemon.
//! - [`select::select`]: pairs usable encoders with the client's decoders,
//!   fastest first; [`select::FallbackEncoder`] walks that ranking on runtime
//!   errors.
//!
//! Backends (priority order): NVENC, VA-API, QSV/oneVPL, AMF (probe only in
//! this build), VideoToolbox (macOS, full), Media Foundation (probe only),
//! OpenH264 (software, full), rav1e (optional AV1, full). Decoders:
//! VideoToolbox, OpenH264, dav1d (optional).
//!
//! # Audio
//! - [`audio::AudioCapture`]: PulseAudio/PipeWire (Linux), ScreenCaptureKit
//!   (macOS), WASAPI loopback / process loopback (Windows).
//! - [`audio::codec`]: Opus (libopus) and PCM s16le, defaults matching the
//!   `cua.env.v1.AudioEncoding` contract.
//! - [`audio::AudioSink`]: uplink into a guest virtual microphone (Linux).
//! - [`audio::packet`]: the `RAU2` wire header (MEDIA.md §12.2).
//!
//! # Plugging into the session runtime
//! This crate is shared by the server (cua-spacesd) and clients
//! (cua-sdk, cua-viewer) and depends on neither. The driver's
//! `cua-spacesd-desktop` adapts [`video::VideoEncoder`] to its capture
//! providers (`codec_encoder.rs`).

#![warn(missing_docs)]

pub mod audio;
pub mod backends;
pub mod bitstream;
pub mod convert;
pub mod error;
pub mod probe;
#[cfg(feature = "proto")]
pub mod proto;
pub mod select;
pub mod types;
pub mod video;

pub use error::{CodecError, Result};
pub use probe::{probe, DecoderInfo, EncoderInfo, ProbeStatus};
pub use select::{select, ClientDecoder, DecoderKind, FallbackEncoder, SelectionPrefs};
pub use types::*;
pub use video::{VideoDecoder, VideoEncoder};
