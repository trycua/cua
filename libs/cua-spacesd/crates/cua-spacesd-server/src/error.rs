// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `tonic::Status` construction with a `cua.env.v1.ErrorInfo` detail.
//!
//! Every error the server returns carries a `google.rpc.Status` in the
//! `grpc-status-details-bin` trailer whose single detail is a packed
//! `ErrorInfo` (see `common.proto`). Clients treat it as optional.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use cua_proto::env::v1::{ErrorInfo, ErrorReason};
use cua_proto::wkt::Any;
use prost::Message;
use tonic::{Code, Status};

/// Type URL of a packed `ErrorInfo`.
pub const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/cua.env.v1.ErrorInfo";

/// `google.rpc.Status`, declared locally (three fields) to avoid a dependency
/// on the googleapis protos.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RpcStatus {
    /// gRPC status code.
    #[prost(int32, tag = "1")]
    pub code: i32,
    /// Developer-facing message.
    #[prost(string, tag = "2")]
    pub message: String,
    /// Packed details.
    #[prost(message, repeated, tag = "3")]
    pub details: Vec<Any>,
}

/// Builds a status whose details carry an `ErrorInfo`.
pub fn status(code: Code, reason: ErrorReason, message: impl Into<String>) -> Status {
    StatusBuilder::new(code, reason, message).build()
}

/// Builder for statuses that need `ErrorInfo.feature` or metadata.
pub struct StatusBuilder {
    code: Code,
    info: ErrorInfo,
}

impl StatusBuilder {
    /// Starts a status.
    pub fn new(code: Code, reason: ErrorReason, message: impl Into<String>) -> Self {
        Self {
            code,
            info: ErrorInfo {
                reason: reason as i32,
                message: message.into(),
                feature: String::new(),
                metadata: HashMap::new(),
            },
        }
    }

    /// Sets `ErrorInfo.feature`.
    pub fn feature(mut self, feature: impl Into<String>) -> Self {
        self.info.feature = feature.into();
        self
    }

    /// Adds one `ErrorInfo.metadata` entry.
    pub fn meta(mut self, key: &str, value: impl ToString) -> Self {
        self.info.metadata.insert(key.to_owned(), value.to_string());
        self
    }

    /// Finishes the status.
    pub fn build(self) -> Status {
        let rpc = RpcStatus {
            code: self.code as i32,
            message: self.info.message.clone(),
            details: vec![Any {
                type_url: ERROR_INFO_TYPE_URL.to_owned(),
                value: self.info.encode_to_vec().into(),
            }],
        };
        Status::with_details(self.code, self.info.message, rpc.encode_to_vec().into())
    }
}

/// Decodes the `ErrorInfo` carried by a status, if any.
pub fn error_info(status: &Status) -> Option<ErrorInfo> {
    let rpc = RpcStatus::decode(status.details()).ok()?;
    rpc.details
        .iter()
        .find(|any| any.type_url == ERROR_INFO_TYPE_URL)
        .and_then(|any| ErrorInfo::decode(any.value.clone()).ok())
}

/// `INVALID_ARGUMENT` with no specific reason.
pub fn invalid(message: impl Into<String>) -> Status {
    status(
        Code::InvalidArgument,
        ErrorReason::Unspecified,
        message.into(),
    )
}

/// `INTERNAL` / `ERROR_REASON_INTERNAL`.
pub fn internal(message: impl Into<String>) -> Status {
    status(Code::Internal, ErrorReason::Internal, message.into())
}

/// `FAILED_PRECONDITION` / `ERROR_REASON_FEATURE_UNSUPPORTED` naming `feature`.
pub fn unsupported(feature: &str, limitation: impl Into<String>) -> Status {
    StatusBuilder::new(
        Code::FailedPrecondition,
        ErrorReason::FeatureUnsupported,
        limitation,
    )
    .feature(feature)
    .build()
}

/// `NOT_FOUND` / `ERROR_REASON_SESSION_NOT_FOUND`.
pub fn session_not_found(what: &str, id: &str) -> Status {
    status(
        Code::NotFound,
        ErrorReason::SessionNotFound,
        format!("unknown or expired {what} {id:?}"),
    )
}

/// `UNAUTHENTICATED` / `ERROR_REASON_UNAUTHENTICATED`.
pub fn unauthenticated(message: impl Into<String>) -> Status {
    status(
        Code::Unauthenticated,
        ErrorReason::Unauthenticated,
        message.into(),
    )
}

/// True if the error means "no space left on device" (or quota exceeded).
pub fn is_disk_full(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::StorageFull || error.kind() == io::ErrorKind::QuotaExceeded {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(libc::ENOSPC) | Some(libc::EDQUOT)
        )
    }
    #[cfg(not(unix))]
    {
        // ERROR_DISK_FULL (112) and ERROR_HANDLE_DISK_FULL (39).
        matches!(error.raw_os_error(), Some(112) | Some(39))
    }
}

/// Maps an I/O error on `path` to a status with the matching reason.
pub fn io_status(error: &io::Error, path: &Path) -> Status {
    let shown = path.display().to_string();
    if is_disk_full(error) {
        return StatusBuilder::new(
            Code::ResourceExhausted,
            ErrorReason::DiskFull,
            format!("{shown}: disk full: {error}"),
        )
        .meta("path", &shown)
        .build();
    }
    #[cfg(unix)]
    let raw = error.raw_os_error();
    #[cfg(unix)]
    if raw == Some(libc::ENOTDIR) {
        return StatusBuilder::new(
            Code::FailedPrecondition,
            ErrorReason::NotADirectory,
            format!("{shown}: not a directory"),
        )
        .meta("path", &shown)
        .build();
    }
    #[cfg(unix)]
    if raw == Some(libc::EISDIR) {
        return StatusBuilder::new(
            Code::FailedPrecondition,
            ErrorReason::IsADirectory,
            format!("{shown}: is a directory"),
        )
        .meta("path", &shown)
        .build();
    }
    #[cfg(unix)]
    if raw == Some(libc::ENOTEMPTY) {
        return StatusBuilder::new(
            Code::FailedPrecondition,
            ErrorReason::Unspecified,
            format!("{shown}: directory not empty (use recursive)"),
        )
        .meta("path", &shown)
        .build();
    }
    let (code, reason) = match error.kind() {
        io::ErrorKind::NotFound => (Code::NotFound, ErrorReason::PathNotFound),
        io::ErrorKind::AlreadyExists => (Code::AlreadyExists, ErrorReason::PathExists),
        io::ErrorKind::PermissionDenied => (Code::PermissionDenied, ErrorReason::PermissionDenied),
        io::ErrorKind::NotADirectory => (Code::FailedPrecondition, ErrorReason::NotADirectory),
        io::ErrorKind::IsADirectory => (Code::FailedPrecondition, ErrorReason::IsADirectory),
        io::ErrorKind::DirectoryNotEmpty => (Code::FailedPrecondition, ErrorReason::Unspecified),
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidFilename => {
            (Code::InvalidArgument, ErrorReason::Unspecified)
        }
        _ => (Code::Internal, ErrorReason::Internal),
    };
    StatusBuilder::new(code, reason, format!("{shown}: {error}"))
        .meta("path", &shown)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_info_round_trips_through_status_details() {
        let status = StatusBuilder::new(Code::FailedPrecondition, ErrorReason::SequenceGap, "gap")
            .meta("expected_sequence", 7)
            .build();
        let info = error_info(&status).expect("details");
        assert_eq!(info.reason, ErrorReason::SequenceGap as i32);
        assert_eq!(info.metadata["expected_sequence"], "7");
        assert_eq!(status.code(), Code::FailedPrecondition);
    }

    #[test]
    fn disk_full_maps_to_resource_exhausted() {
        #[cfg(unix)]
        let error = io::Error::from_raw_os_error(libc::ENOSPC);
        #[cfg(not(unix))]
        let error = io::Error::from_raw_os_error(112);
        let status = io_status(&error, Path::new("/x"));
        assert_eq!(status.code(), Code::ResourceExhausted);
        assert_eq!(
            error_info(&status).unwrap().reason,
            ErrorReason::DiskFull as i32
        );
    }
}
