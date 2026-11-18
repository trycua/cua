// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Error type shared by every codec and audio backend.

use crate::types::Backend;

/// Result alias for this crate.
pub type Result<T, E = CodecError> = std::result::Result<T, E>;

/// Why a codec operation failed.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The backend is not present on this host (library missing, no device,
    /// wrong OS, feature compiled out). Selection skips it.
    #[error("{backend:?} unavailable: {reason}")]
    Unavailable {
        /// Backend that is unavailable.
        backend: Backend,
        /// Human-readable reason.
        reason: String,
    },
    /// The backend exists but cannot do what was asked (codec, size, pixel
    /// format, in-place reconfiguration).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Invalid configuration or input from the caller.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A runtime failure inside the backend (driver error, session lost).
    /// [`crate::select::FallbackEncoder`] reacts to this by moving to the next
    /// candidate.
    #[error("{backend:?} failed: {message}")]
    Backend {
        /// Backend that failed.
        backend: Backend,
        /// Driver or library message.
        message: String,
    },
    /// Audio device or sound-server failure.
    #[error("audio: {0}")]
    Audio(String),
    /// Every candidate encoder failed.
    #[error("no usable encoder: {0}")]
    NoEncoder(String),
}

impl CodecError {
    /// Shorthand for [`CodecError::Backend`].
    pub fn backend(backend: Backend, message: impl Into<String>) -> Self {
        Self::Backend {
            backend,
            message: message.into(),
        }
    }

    /// Shorthand for [`CodecError::Unavailable`].
    pub fn unavailable(backend: Backend, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            backend,
            reason: reason.into(),
        }
    }

    /// True if the error means "try another backend" rather than "the caller
    /// passed something wrong".
    pub fn is_backend_failure(&self) -> bool {
        matches!(
            self,
            Self::Backend { .. } | Self::Unavailable { .. } | Self::Unsupported(_)
        )
    }
}
