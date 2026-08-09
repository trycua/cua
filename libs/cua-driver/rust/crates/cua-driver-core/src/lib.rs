//! MCP JSON-RPC 2.0 server over stdio — platform-independent core.
//!
//! Implements the Model Context Protocol (MCP) 2024-11-05 over stdio,
//! matching the interface of `libs/cua-driver` (Swift/macOS) and
//! `CuaDriver.Win` (.NET/Windows).
//!
//! # Protocol
//! - Line-delimited JSON-RPC 2.0 on stdin/stdout
//! - Methods: `initialize`, `notifications/initialized`, `tools/list`, `tools/call`
//! - Each request has `jsonrpc: "2.0"`, `id` (any), `method`, optional `params`
//! - Notifications (no `id`) are silently ignored

pub const RESPONSIBILITY_DISCLAIMED_ENV: &str = "CUA_DRIVER_RS_RESPONSIBILITY_DISCLAIMED";

/// Embedded mode (`CUA_DRIVER_EMBEDDED=1` / `--embedded`): the daemon runs as
/// a direct child of a host app and stays in its TCC responsibility chain —
/// no disclaim re-exec, standalone-app relaunch, or permission prompts.
/// See `Skills/cua-driver/EMBEDDING.md`.
///
/// Caller-controlled, which is safe only because embedded mode strictly
/// REMOVES capability claims; it must never feed into the `driver-daemon`
/// attribution decision (`permission_source` in platform-macos).
pub const EMBEDDED_ENV: &str = "CUA_DRIVER_EMBEDDED";

/// Advisory label for the embedding host's bundle id, echoed in
/// `check_permissions` output. NOT a trust signal — trust comes from the
/// OS responsibility chain.
pub const HOST_BUNDLE_ID_ENV: &str = "CUA_DRIVER_HOST_BUNDLE_ID";

/// Internal embedded-host contract: when set to the exact value `1`, the
/// daemon treats EOF on stdin as proof that its owning host has exited. The
/// Rust SDK sets this only on the directly-spawned `serve` child; MCP proxies
/// continue to use stdin for JSON-RPC and never set it.
pub const PARENT_LIVENESS_STDIN_ENV: &str = "CUA_DRIVER_PARENT_LIVENESS_STDIN";

/// Private service endpoint selected by a trusted embedded launcher.
///
/// MCP proxies may inherit this socket path, but the pane bearer is accepted
/// only as a consumed field of `trusted_session_begin` on the authenticated
/// original host connection.
pub const EMBEDDED_SOCKET_ENV: &str = "CUA_DRIVER_EMBEDDED_SOCKET";

use std::future::Future;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Opaque identity of one authenticated embedded-host service connection.
///
/// This value is process-local, is never serialized, and is deliberately
/// distinct from the browser bearer. Browser targets and pooled CDP sockets
/// use it to prevent authority minted on one connection from being reused by
/// an ordinary daemon/MCP call or by a later host connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EmbeddedBrowserAuthorityId(u64);

/// Authority available only while the original authenticated embedded-host
/// connection executes `trusted_session_call`.
#[derive(Clone)]
pub struct EmbeddedBrowserAuthority {
    id: EmbeddedBrowserAuthorityId,
    bearer: Arc<str>,
}

impl std::fmt::Debug for EmbeddedBrowserAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddedBrowserAuthority")
            .field("id", &self.id)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

tokio::task_local! {
    static EMBEDDED_BROWSER_AUTHORITY: EmbeddedBrowserAuthority;
}

fn valid_browser_endpoint_bearer(value: &str) -> bool {
    (32..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Consume a validated bearer received by `trusted_session_begin` on an
/// already authenticated embedded-host connection and mint authority for that
/// accepted connection. The bearer is never read from process environment or
/// daemon configuration and has no public accessor outside the task scope.
pub fn embedded_browser_authority_from_bearer(value: String) -> Option<EmbeddedBrowserAuthority> {
    if !valid_browser_endpoint_bearer(&value) {
        return None;
    }
    static NEXT_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);
    Some(EmbeddedBrowserAuthority {
        id: EmbeddedBrowserAuthorityId(NEXT_AUTHORITY_ID.fetch_add(1, Ordering::Relaxed).max(1)),
        bearer: Arc::from(value),
    })
}

/// Execute one future with the browser authority of the authenticated host
/// connection. The scope is async-task-local and ends before the daemon writes
/// the result, so the secret and authority never enter arguments, results, or
/// metadata.
pub async fn with_embedded_browser_authority<F>(
    authority: EmbeddedBrowserAuthority,
    future: F,
) -> F::Output
where
    F: Future,
{
    EMBEDDED_BROWSER_AUTHORITY.scope(authority, future).await
}

/// Return the current opaque authority identity without exposing the bearer.
pub fn embedded_browser_authority_id() -> Option<EmbeddedBrowserAuthorityId> {
    EMBEDDED_BROWSER_AUTHORITY
        .try_with(|authority| authority.id)
        .ok()
}

/// Return the private browser bearer only inside a connection-bound trusted
/// call. Ordinary daemon, SDK, CLI, and MCP calls always receive `None`.
pub fn browser_endpoint_bearer() -> Option<String> {
    EMBEDDED_BROWSER_AUTHORITY
        .try_with(|authority| authority.bearer.to_string())
        .ok()
}

/// Only the exact value `1` counts — fail-safe for anything else.
pub fn embedded_mode() -> bool {
    std::env::var_os(EMBEDDED_ENV).is_some_and(|v| v == "1")
}

/// Parent-EOF shutdown is valid only for a directly embedded daemon. Requiring
/// both sentinels prevents an ambient variable from changing ordinary
/// standalone or MCP stdin behavior.
pub fn parent_liveness_stdin_enabled() -> bool {
    embedded_mode() && std::env::var_os(PARENT_LIVENESS_STDIN_ENV).is_some_and(|value| value == "1")
}

#[cfg(test)]
mod embedded_browser_contract_tests {
    use super::{
        browser_endpoint_bearer, embedded_browser_authority_from_bearer,
        embedded_browser_authority_id, valid_browser_endpoint_bearer,
        with_embedded_browser_authority,
    };

    #[test]
    fn browser_endpoint_bearer_has_a_closed_url_safe_shape() {
        assert!(valid_browser_endpoint_bearer(
            "abcdefghijklmnopqrstuvwxyz_123456"
        ));
        assert!(!valid_browser_endpoint_bearer("too-short"));
        assert!(!valid_browser_endpoint_bearer(
            "abcdefghijklmnopqrstuvwxyz token 123456"
        ));
        assert!(!valid_browser_endpoint_bearer(&"a".repeat(257)));
    }

    #[tokio::test]
    async fn browser_authority_exists_only_inside_its_bound_task_scope() {
        let first =
            embedded_browser_authority_from_bearer("abcdefghijklmnopqrstuvwxyz_123456".into())
                .expect("valid first bearer");
        let second =
            embedded_browser_authority_from_bearer("abcdefghijklmnopqrstuvwxyz_123456".into())
                .expect("valid second bearer");

        assert_eq!(browser_endpoint_bearer(), None);
        assert_eq!(embedded_browser_authority_id(), None);
        let first_id = with_embedded_browser_authority(first, async {
            assert_eq!(
                browser_endpoint_bearer().as_deref(),
                Some("abcdefghijklmnopqrstuvwxyz_123456")
            );
            embedded_browser_authority_id().expect("bound authority")
        })
        .await;
        let second_id = with_embedded_browser_authority(second, async {
            embedded_browser_authority_id().expect("bound authority")
        })
        .await;
        assert_ne!(first_id, second_id);
        assert_eq!(browser_endpoint_bearer(), None);
        assert_eq!(embedded_browser_authority_id(), None);
    }

    #[tokio::test]
    async fn concurrent_untrusted_tasks_cannot_observe_a_scoped_browser_credential() {
        use std::sync::Arc;

        let authority =
            embedded_browser_authority_from_bearer("abcdefghijklmnopqrstuvwxyz_123456".into())
                .expect("valid bearer");
        let authority_id = authority.id;
        let entered = Arc::new(tokio::sync::Barrier::new(2));
        let release = Arc::new(tokio::sync::Barrier::new(2));

        let trusted = {
            let entered = entered.clone();
            let release = release.clone();
            tokio::spawn(with_embedded_browser_authority(authority, async move {
                assert_eq!(embedded_browser_authority_id(), Some(authority_id));
                assert_eq!(
                    browser_endpoint_bearer().as_deref(),
                    Some("abcdefghijklmnopqrstuvwxyz_123456")
                );
                entered.wait().await;
                release.wait().await;
                assert_eq!(embedded_browser_authority_id(), Some(authority_id));
                assert!(browser_endpoint_bearer().is_some());
            }))
        };

        entered.wait().await;
        let ordinary = tokio::spawn(async {
            assert_eq!(embedded_browser_authority_id(), None);
            assert_eq!(browser_endpoint_bearer(), None);
            tokio::task::yield_now().await;
            assert_eq!(embedded_browser_authority_id(), None);
            assert_eq!(browser_endpoint_bearer(), None);
        });
        ordinary.await.expect("ordinary task");
        assert_eq!(embedded_browser_authority_id(), None);
        assert_eq!(browser_endpoint_bearer(), None);
        release.wait().await;
        trusted.await.expect("trusted task");
    }
}

pub mod action_record;
pub mod authorization;
pub mod background_input;
pub mod browser;
pub mod capture_mode;
pub mod capture_scope;
pub mod cdp;
pub mod clipboard;
pub mod consent;
pub mod cursor_events;
pub mod cursor_sampler;
pub mod daemon;
pub mod element_cache;
pub mod element_query;
pub mod element_token;
pub mod expectation;
pub mod ffmpeg_install;
pub mod health_report;
pub mod image_utils;
pub mod page;
pub mod pip_hook;
pub mod policy;
pub mod protocol;
pub mod recording;
pub mod recording_loader;
pub mod recording_render;
pub mod recording_tools;
pub mod recording_zoom;
pub mod server;
pub mod session;
pub mod session_authorization;
pub mod session_manifest;
pub mod session_tools;
pub mod socket_io;
pub mod text_sanitize;
pub mod tool;
pub mod tool_args;
pub mod tool_schema;
pub mod video;
pub mod video_ffmpeg;
pub mod window_inspection;
pub mod window_target;

pub use cua_driver_contract::{CaptureScope, EscalationReason};
pub use recording::RecordingSession;
