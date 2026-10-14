// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `tonic::Status` with a packed `cua.env.v1.ErrorInfo` detail, and the
//! mapping from provider errors.

use cua_proto::env::v1::{ErrorInfo, ErrorReason};
use cua_proto::wkt::Any;
use cua_spacesd_provider_api::{ProviderError, ProviderErrorCode};
use cua_spacesd_session::media::MediaError;
use prost::Message;
use tonic::{Code, Status};

const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/cua.env.v1.ErrorInfo";

/// `google.rpc.Status` (declared locally to avoid the googleapis protos).
#[derive(Clone, PartialEq, prost::Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<Any>,
}

pub(crate) fn status_with(
    code: Code,
    reason: ErrorReason,
    message: impl Into<String>,
    feature: &str,
    metadata: &[(&str, String)],
) -> Status {
    let message = message.into();
    let info = ErrorInfo {
        reason: reason as i32,
        message: message.clone(),
        feature: feature.to_owned(),
        metadata: metadata
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    };
    let rpc = RpcStatus {
        code: code as i32,
        message: message.clone(),
        details: vec![Any {
            type_url: ERROR_INFO_TYPE_URL.into(),
            value: info.encode_to_vec().into(),
        }],
    };
    Status::with_details(code, message, rpc.encode_to_vec().into())
}

pub(crate) fn status(code: Code, reason: ErrorReason, message: impl Into<String>) -> Status {
    status_with(code, reason, message, "", &[])
}

pub(crate) fn unsupported(feature: &str, message: impl Into<String>) -> Status {
    status_with(
        Code::FailedPrecondition,
        ErrorReason::FeatureUnsupported,
        message,
        feature,
        &[],
    )
}

pub(crate) fn invalid(message: impl Into<String>) -> Status {
    status(Code::InvalidArgument, ErrorReason::Unspecified, message)
}

pub(crate) fn not_found(message: impl Into<String>) -> Status {
    status(Code::NotFound, ErrorReason::SessionNotFound, message)
}

pub(crate) fn provider(error: ProviderError) -> Status {
    let (code, reason) = match error.code {
        ProviderErrorCode::StaleTarget => (Code::FailedPrecondition, ErrorReason::StaleHandle),
        ProviderErrorCode::PermissionDenied | ProviderErrorCode::ConsentRequired => {
            (Code::PermissionDenied, ErrorReason::PermissionDenied)
        }
        ProviderErrorCode::TargetUnavailable => (Code::NotFound, ErrorReason::TargetUnavailable),
        ProviderErrorCode::Unsupported => {
            (Code::FailedPrecondition, ErrorReason::FeatureUnsupported)
        }
        ProviderErrorCode::ViewOnly => (Code::PermissionDenied, ErrorReason::PermissionDenied),
        ProviderErrorCode::WouldRequireActivation => (
            Code::FailedPrecondition,
            ErrorReason::WouldRequireActivation,
        ),
        ProviderErrorCode::DeliveryFailed => (Code::Internal, ErrorReason::DeliveryFailed),
        ProviderErrorCode::CaptureFailed => (Code::Unavailable, ErrorReason::Internal),
        ProviderErrorCode::Internal => (Code::Internal, ErrorReason::Internal),
    };
    status(code, reason, error.message)
}

pub(crate) fn media(error: MediaError) -> Status {
    match error {
        MediaError::NotFound(message) => {
            status(Code::NotFound, ErrorReason::TargetUnavailable, message)
        }
        MediaError::InvalidArgument(message) => invalid(message),
        MediaError::FailedPrecondition(message) => {
            status(Code::FailedPrecondition, ErrorReason::Unspecified, message)
        }
        MediaError::ResourceExhausted(message) => {
            status(Code::ResourceExhausted, ErrorReason::LimitExceeded, message)
        }
        MediaError::Unavailable(message) => {
            status(Code::Unavailable, ErrorReason::Internal, message)
        }
        MediaError::Provider(error) => provider(error),
    }
}

pub(crate) fn join_error(error: tokio::task::JoinError) -> Status {
    status(Code::Internal, ErrorReason::Internal, error.to_string())
}
