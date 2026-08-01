use thiserror::Error;

pub const MAX_STATUS_BODY_BYTES: usize = 4_096;

#[derive(Debug, Error, uniffi::Error)]
pub enum HttpError {
    #[error("transport error: {reason}")]
    Transport { reason: String },
}

#[derive(Debug, Error, uniffi::Error)]
pub enum AccessTokenProviderError {
    #[error("access token unavailable: {reason}")]
    Failed { reason: String },
}

#[derive(Debug, Error, uniffi::Error)]
pub enum SdkError {
    #[error("invalid configuration: {reason}")]
    Configuration { reason: String },
    #[error("invalid {field} {value:?}: {reason}")]
    InvalidResourceName {
        field: String,
        value: String,
        reason: String,
    },
    #[error("transport error: {reason}")]
    Transport { reason: String },
    #[error("token error: {reason}")]
    Token { reason: String },
    #[error("invalid response body: {reason}")]
    Body { reason: String },
    #[error("{operation} returned HTTP {status}: {body}")]
    Status {
        operation: String,
        status: u16,
        body: String,
    },
    #[error("unknown sandbox service {requested}; available services: {available:?}")]
    UnknownService {
        requested: String,
        available: Vec<String>,
    },
    #[error("invalid service path: {path}")]
    InvalidServicePath { path: String },
    #[error("claim entered terminal phase {phase}: {status}")]
    ClaimFailed { phase: String, status: String },
    #[error("claim did not bind before the polling limit")]
    ClaimTimeout,
}

impl SdkError {
    pub fn status(operation: impl Into<String>, status: u16, body: &[u8]) -> Self {
        Self::Status {
            operation: operation.into(),
            status,
            body: bounded_body(body),
        }
    }
}

pub fn bounded_body(body: &[u8]) -> String {
    let rendered = String::from_utf8_lossy(body);
    if rendered.len() <= MAX_STATUS_BODY_BYTES {
        return rendered.into_owned();
    }

    let mut end = MAX_STATUS_BODY_BYTES;
    while !rendered.is_char_boundary(end) {
        end -= 1;
    }
    rendered[..end].into()
}
