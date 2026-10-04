// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::types::TransferScope;

/// Errors produced while describing, exporting, bundling, or importing an
/// app session. Shared by the sender and the receiver.
#[derive(Debug)]
pub enum TeleportError {
    /// Underlying I/O failure.
    Io(std::io::Error),
    /// Manifest or metadata (de)serialization failure.
    Json(serde_json::Error),
    /// A bundle entry's content hash does not match its declared SHA-256.
    ChecksumMismatch { rel_path: String },
    /// The bundle (declared or actual) exceeds the configured total size.
    BundleTooLarge { limit_bytes: u64 },
    /// The bundle stream is structurally invalid.
    InvalidBundle(String),
    /// An entry path is absolute, escapes the root, or is otherwise unsafe.
    InvalidEntryPath(String),
    /// The provider does not support the requested transfer scope.
    UnsupportedScope {
        provider_id: String,
        scope: TransferScope,
    },
    /// No registered provider matches the application.
    NoProviderForApp { app_id: String },
    /// No registered provider has the requested identifier.
    UnknownProvider { provider_id: String },
    /// Provider-specific failure.
    Provider(String),
}

impl std::fmt::Display for TeleportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::ChecksumMismatch { rel_path } => {
                write!(f, "checksum mismatch for bundle entry {rel_path:?}")
            }
            Self::BundleTooLarge { limit_bytes } => {
                write!(f, "bundle exceeds the {limit_bytes}-byte total size limit")
            }
            Self::InvalidBundle(reason) => write!(f, "invalid session bundle: {reason}"),
            Self::InvalidEntryPath(path) => write!(f, "invalid bundle entry path {path:?}"),
            Self::UnsupportedScope { provider_id, scope } => {
                write!(f, "provider {provider_id} does not support scope {scope:?}")
            }
            Self::NoProviderForApp { app_id } => {
                write!(f, "no session provider matches app {app_id:?}")
            }
            Self::UnknownProvider { provider_id } => {
                write!(f, "no session provider with id {provider_id:?}")
            }
            Self::Provider(reason) => write!(f, "provider error: {reason}"),
        }
    }
}

impl std::error::Error for TeleportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TeleportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for TeleportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, TeleportError>;
