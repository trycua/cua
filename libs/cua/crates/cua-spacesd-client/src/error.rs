//! Typed errors. RPC failures are decoded from the `google.rpc.Status` in
//! `grpc-status-details-bin`, whose details carry a `cua.env.v1.ErrorInfo`;
//! when the details are absent the gRPC code alone is mapped.

use cua_proto::env::v1::{ErrorInfo, ErrorReason};
use prost::Message;
use std::{collections::HashMap, time::Duration};

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Boxed error used by transports.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// `google.rpc.Status` (the payload of `grpc-status-details-bin`).
#[derive(Clone, PartialEq, prost::Message)]
pub struct RpcStatus {
    /// gRPC code.
    #[prost(int32, tag = "1")]
    pub code: i32,
    /// Developer-facing message.
    #[prost(string, tag = "2")]
    pub message: String,
    /// Detail messages.
    #[prost(message, repeated, tag = "3")]
    pub details: Vec<pbjson_types::Any>,
}

/// Type URL of a packed `cua.env.v1.ErrorInfo`.
pub const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/cua.env.v1.ErrorInfo";

/// Context shared by every RPC error variant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorDetails {
    /// gRPC status code.
    pub code: i32,
    /// Human-readable message (from `ErrorInfo.message`, else the status).
    pub message: String,
    /// `ErrorInfo.metadata`.
    pub metadata: HashMap<String, String>,
}

impl std::fmt::Display for ErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:?})",
            self.message,
            tonic::Code::from_i32(self.code)
        )
    }
}

/// Every error the env client can return.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint string could not be parsed.
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    /// Could not reach the endpoint (connect, TLS, HTTP).
    #[error("transport error: {0}")]
    Transport(String),
    /// The endpoint answered, but not as a cua-spacesd (the
    /// `GetCapabilities` probe failed on every transport).
    #[error("cua-spacesd is not available at {endpoint}: {reason}")]
    SpacesdNotAvailable {
        /// Endpoint probed.
        endpoint: String,
        /// Why the probe failed.
        reason: String,
    },
    /// Missing or wrong token or ticket.
    #[error("unauthenticated: {0}")]
    Unauthenticated(ErrorDetails),
    /// The guest lacks a feature.
    #[error("feature {feature:?} unsupported: {details}")]
    FeatureUnsupported {
        /// Capability name.
        feature: String,
        /// Details.
        details: ErrorDetails,
    },
    /// The guest OS denied permission.
    #[error("permission denied: {0}")]
    PermissionDenied(ErrorDetails),
    /// Window handle epoch mismatch.
    #[error("stale window handle: {0}")]
    StaleHandle(ErrorDetails),
    /// Geometry changed since the referenced screenshot.
    #[error("stale geometry: {0}")]
    StaleGeometry(ErrorDetails),
    /// Accessibility snapshot no longer current.
    #[error("stale accessibility snapshot: {0}")]
    StaleSnapshot(ErrorDetails),
    /// Background delivery impossible.
    #[error("would require activation: {0}")]
    WouldRequireActivation(ErrorDetails),
    /// Target gone.
    #[error("target unavailable: {0}")]
    TargetUnavailable(ErrorDetails),
    /// Native delivery failed.
    #[error("delivery failed: {0}")]
    DeliveryFailed(ErrorDetails),
    /// No such process.
    #[error("process not found: {0}")]
    ProcessNotFound(ErrorDetails),
    /// No such path.
    #[error("path not found: {0}")]
    PathNotFound(ErrorDetails),
    /// Path already exists.
    #[error("path exists: {0}")]
    PathExists(ErrorDetails),
    /// Directory required.
    #[error("not a directory: {0}")]
    NotADirectory(ErrorDetails),
    /// Regular file required.
    #[error("is a directory: {0}")]
    IsADirectory(ErrorDetails),
    /// Guest disk full.
    #[error("disk full: {0}")]
    DiskFull(ErrorDetails),
    /// SHA-256 mismatch (server-side or client-side verification).
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Expected digest.
        expected: String,
        /// Actual digest.
        actual: String,
    },
    /// Unknown or expired session (upload, watcher, media, forward).
    #[error("session not found: {0}")]
    SessionNotFound(ErrorDetails),
    /// Upload chunk at the wrong offset.
    #[error("offset mismatch (server expects {expected_offset:?}): {details}")]
    OffsetMismatch {
        /// Offset the server expects, when reported.
        expected_offset: Option<u64>,
        /// Details.
        details: ErrorDetails,
    },
    /// Sequenced input skipped ahead.
    #[error("sequence gap (server expects {expected_sequence:?}): {details}")]
    SequenceGap {
        /// Sequence the server expects, when reported.
        expected_sequence: Option<u64>,
        /// Details.
        details: ErrorDetails,
    },
    /// Rate limited.
    #[error("rate limited: {details}")]
    RateLimited {
        /// Suggested delay.
        retry_after: Option<Duration>,
        /// Details.
        details: ErrorDetails,
    },
    /// A request limit was exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(ErrorDetails),
    /// `SystemService.Init` has not run.
    #[error("not initialized: {0}")]
    NotInitialized(ErrorDetails),
    /// Server-side bug.
    #[error("internal server error: {0}")]
    Internal(ErrorDetails),
    /// Any other RPC failure (no `ErrorInfo`, or an unknown reason).
    #[error("rpc failed: {0}")]
    Rpc(ErrorDetails),
    /// A client-side deadline elapsed.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// The guest's desktop session (display, window manager, cua-driver
    /// input) did not become ready within the wait; the message says what
    /// is missing.
    #[error("desktop not ready: {0}")]
    DesktopNotReady(String),
    /// The server sent something the contract does not allow.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// Local I/O.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The gRPC code, for RPC errors.
    pub fn code(&self) -> Option<tonic::Code> {
        self.details().map(|d| tonic::Code::from_i32(d.code))
    }

    /// Shared RPC details, for RPC errors.
    pub fn details(&self) -> Option<&ErrorDetails> {
        use Error::*;
        match self {
            Unauthenticated(d)
            | PermissionDenied(d)
            | StaleHandle(d)
            | StaleGeometry(d)
            | StaleSnapshot(d)
            | WouldRequireActivation(d)
            | TargetUnavailable(d)
            | DeliveryFailed(d)
            | ProcessNotFound(d)
            | PathNotFound(d)
            | PathExists(d)
            | NotADirectory(d)
            | IsADirectory(d)
            | DiskFull(d)
            | SessionNotFound(d)
            | LimitExceeded(d)
            | NotInitialized(d)
            | Internal(d)
            | Rpc(d) => Some(d),
            FeatureUnsupported { details, .. }
            | OffsetMismatch { details, .. }
            | SequenceGap { details, .. }
            | RateLimited { details, .. } => Some(details),
            _ => None,
        }
    }

    /// True when retrying the same idempotent call may succeed: connection
    /// level failures without a server-issued `ErrorInfo`, plus rate limits.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Transport(_) | Error::RateLimited { .. } => true,
            Error::Rpc(d) => matches!(
                tonic::Code::from_i32(d.code),
                tonic::Code::Unavailable
                    | tonic::Code::Unknown
                    | tonic::Code::Cancelled
                    | tonic::Code::Aborted
                    | tonic::Code::Internal
            ),
            _ => false,
        }
    }
}

impl From<tonic::Status> for Error {
    fn from(status: tonic::Status) -> Self {
        from_status(&status)
    }
}

/// Decodes `ErrorInfo` from a status, if present.
pub fn error_info(status: &tonic::Status) -> Option<ErrorInfo> {
    let details = status.details();
    if details.is_empty() {
        return None;
    }
    let rpc = RpcStatus::decode(details).ok()?;
    rpc.details
        .iter()
        .find(|any| {
            any.type_url.ends_with("/cua.env.v1.ErrorInfo")
                || any.type_url == "cua.env.v1.ErrorInfo"
        })
        .and_then(|any| ErrorInfo::decode(any.value.as_ref()).ok())
}

/// One human-readable line for a status: its message plus, for a
/// client-side transport failure, the innermost cause (`broken pipe`,
/// `connection refused`). Never the `Debug` dump of the source chain.
pub fn status_text(status: &tonic::Status) -> String {
    let message = status.message().trim();
    let message = if message.is_empty() {
        format!("{:?}", status.code())
    } else {
        message.to_string()
    };
    match transport_cause(status) {
        Some(cause) if !message.contains(&cause) => format!("{message} ({cause})"),
        _ => message,
    }
}

/// The innermost cause of a status raised by the client's transport
/// (connect, TLS, HTTP/2 or gRPC-Web framing), or `None` for a status the
/// server sent.
pub fn transport_cause(status: &tonic::Status) -> Option<String> {
    let mut cause: &(dyn std::error::Error + 'static) = std::error::Error::source(status)?;
    let mut text = cause.to_string();
    // Bounded: source chains are short; never loop on a cyclic one.
    for _ in 0..16 {
        let Some(next) = cause.source() else { break };
        cause = next;
        let t = cause.to_string();
        if !t.trim().is_empty() {
            text = t;
        }
    }
    Some(text.trim().to_string())
}

/// Maps a `tonic::Status` into a typed [`Error`].
pub fn from_status(status: &tonic::Status) -> Error {
    let info = error_info(status);
    let code = status.code() as i32;
    // The request never got a server answer (connection refused, reset,
    // closed while the spacesd starts): a transport error, not an RPC one.
    if info.is_none()
        && transport_cause(status).is_some()
        && matches!(
            status.code(),
            tonic::Code::Unknown | tonic::Code::Unavailable | tonic::Code::Internal
        )
    {
        return Error::Transport(status_text(status));
    }
    let Some(info) = info else {
        let details = ErrorDetails {
            code,
            message: status.message().to_string(),
            metadata: HashMap::new(),
        };
        return match status.code() {
            tonic::Code::Unauthenticated => Error::Unauthenticated(details),
            tonic::Code::PermissionDenied => Error::PermissionDenied(details),
            _ => Error::Rpc(details),
        };
    };
    let message = if info.message.is_empty() {
        status.message().to_string()
    } else {
        info.message.clone()
    };
    let details = ErrorDetails {
        code,
        message,
        metadata: info.metadata.clone(),
    };
    let meta_u64 = |k: &str| info.metadata.get(k).and_then(|v| v.parse::<u64>().ok());
    match ErrorReason::try_from(info.reason).unwrap_or(ErrorReason::Unspecified) {
        ErrorReason::Unauthenticated => Error::Unauthenticated(details),
        ErrorReason::FeatureUnsupported => Error::FeatureUnsupported {
            feature: info.feature.clone(),
            details,
        },
        ErrorReason::PermissionDenied => Error::PermissionDenied(details),
        ErrorReason::StaleHandle => Error::StaleHandle(details),
        ErrorReason::StaleGeometry => Error::StaleGeometry(details),
        ErrorReason::StaleSnapshot => Error::StaleSnapshot(details),
        ErrorReason::WouldRequireActivation => Error::WouldRequireActivation(details),
        ErrorReason::TargetUnavailable => Error::TargetUnavailable(details),
        ErrorReason::DeliveryFailed => Error::DeliveryFailed(details),
        ErrorReason::ProcessNotFound => Error::ProcessNotFound(details),
        ErrorReason::PathNotFound => Error::PathNotFound(details),
        ErrorReason::PathExists => Error::PathExists(details),
        ErrorReason::NotADirectory => Error::NotADirectory(details),
        ErrorReason::IsADirectory => Error::IsADirectory(details),
        ErrorReason::DiskFull => Error::DiskFull(details),
        ErrorReason::ChecksumMismatch => Error::ChecksumMismatch {
            expected: info
                .metadata
                .get("expected_sha256")
                .cloned()
                .unwrap_or_default(),
            actual: info
                .metadata
                .get("actual_sha256")
                .cloned()
                .unwrap_or_default(),
        },
        ErrorReason::SessionNotFound => Error::SessionNotFound(details),
        ErrorReason::OffsetMismatch => Error::OffsetMismatch {
            expected_offset: meta_u64("expected_offset"),
            details,
        },
        ErrorReason::SequenceGap => Error::SequenceGap {
            expected_sequence: meta_u64("expected_sequence"),
            details,
        },
        ErrorReason::RateLimited => Error::RateLimited {
            retry_after: meta_u64("retry_after_ms").map(Duration::from_millis),
            details,
        },
        ErrorReason::LimitExceeded => Error::LimitExceeded(details),
        ErrorReason::NotInitialized => Error::NotInitialized(details),
        ErrorReason::Internal => Error::Internal(details),
        ErrorReason::Unspecified => Error::Rpc(details),
    }
}

/// Builds a `tonic::Status` carrying `info` the way cua-spacesd does:
/// a `google.rpc.Status` with one packed `ErrorInfo` in
/// `grpc-status-details-bin`. Useful for servers and test doubles.
pub fn status_with_info(code: tonic::Code, info: ErrorInfo) -> tonic::Status {
    let message = info.message.clone();
    let rpc = RpcStatus {
        code: code as i32,
        message: message.clone(),
        details: vec![pbjson_types::Any {
            type_url: ERROR_INFO_TYPE_URL.to_string(),
            value: info.encode_to_vec().into(),
        }],
    };
    tonic::Status::with_details(code, message, rpc.encode_to_vec().into())
}

/// Shorthand for [`status_with_info`] with a reason and message.
pub fn status_with_reason(
    code: tonic::Code,
    reason: ErrorReason,
    message: impl Into<String>,
    metadata: impl IntoIterator<Item = (String, String)>,
) -> tonic::Status {
    status_with_info(
        code,
        ErrorInfo {
            reason: reason as i32,
            message: message.into(),
            feature: String::new(),
            metadata: metadata.into_iter().collect(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_error_info() {
        let status = status_with_reason(
            tonic::Code::FailedPrecondition,
            ErrorReason::OffsetMismatch,
            "bad offset",
            [("expected_offset".to_string(), "4096".to_string())],
        );
        match Error::from(status) {
            Error::OffsetMismatch {
                expected_offset,
                details,
            } => {
                assert_eq!(expected_offset, Some(4096));
                assert_eq!(details.message, "bad offset");
                assert_eq!(details.code, tonic::Code::FailedPrecondition as i32);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn feature_unsupported_keeps_feature() {
        let status = status_with_info(
            tonic::Code::Unimplemented,
            ErrorInfo {
                reason: ErrorReason::FeatureUnsupported as i32,
                message: "no a11y".into(),
                feature: "a11y".into(),
                metadata: Default::default(),
            },
        );
        assert!(matches!(
            Error::from(status),
            Error::FeatureUnsupported { feature, .. } if feature == "a11y"
        ));
    }

    #[test]
    fn plain_status_maps_by_code() {
        assert!(matches!(
            Error::from(tonic::Status::unauthenticated("no")),
            Error::Unauthenticated(_)
        ));
        let e = Error::from(tonic::Status::unavailable("down"));
        assert!(e.is_retryable());
        assert_eq!(e.code(), Some(tonic::Code::Unavailable));
        let e = Error::from(status_with_reason(
            tonic::Code::NotFound,
            ErrorReason::PathNotFound,
            "x",
            [],
        ));
        assert!(!e.is_retryable());
    }
}
