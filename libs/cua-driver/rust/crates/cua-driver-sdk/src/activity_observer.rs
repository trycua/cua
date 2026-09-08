// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Optional content-free activity callbacks for embedding hosts.

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DriverActivityKind {
    AuthorizedAction,
    AuthorizationRefused,
    ActionFailed,
    GrantIssued,
    GrantRevoked,
    SessionStarted,
    SessionEnded,
}

/// A content-free lifecycle event emitted after native authorization decides
/// a call. It never carries arguments, page text, paths, typed input, images,
/// or raw resource identities.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DriverActivityEvent {
    pub kind: DriverActivityKind,
    pub unix_ms: u64,
    pub tool_name: String,
    pub adapter_ids: Vec<String>,
    pub risk_class: String,
    pub public_session: Option<String>,
    pub refusal_code: Option<String>,
}

/// Optional observer implemented by trusted embedding-host code.
///
/// Observations are informational and cannot grant authority or change a tool
/// result. Implementations should return quickly and hand off expensive work.
#[uniffi::export(with_foreign)]
pub trait DriverActivityObserver: Send + Sync {
    fn on_activity(&self, event: DriverActivityEvent);
}
