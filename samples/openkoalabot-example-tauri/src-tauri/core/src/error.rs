// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One error type for the app; the Tauri shell renders it as a string.

/// Everything that can go wrong in the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Spaces runtime refused or failed (typed: capability missing,
    /// teleport refused, timeout, transport, ...).
    #[error(transparent)]
    Spaces(#[from] cua_spaces::Error),
    /// Cua Cloud could not be configured (credentials; see `cua auth login`).
    #[error("Cua Cloud: {0}")]
    Cloud(#[from] cua_fleet::Error),
    /// Local I/O.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A bounded wait ran out.
    #[error("timed out: {0}")]
    Timeout(String),
    /// A check failed or an argument was wrong.
    #[error("{0}")]
    Invalid(String),
}

/// Core result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
