//! Cua product telemetry: one module for the SDK core, the bindings, the
//! `cua` CLI, `cua daemon`, cua-spacesd and the Spaces app.
//!
//! Usage analytics ([`client`], [`events`], [`schema`]): anonymous and
//! content-free. On by default; off with `DO_NOT_TRACK=1`,
//! `CUA_TELEMETRY=0`, `cua config set telemetry off`, `cua telemetry off`
//! or the Spaces app setting; off in CI unless `CUA_TELEMETRY=1`. Nothing
//! is sent before the first-run notice has been shown once.
//!
//! Privacy rules, enforced by [`schema::validate`] on every payload and by
//! the tests in `tests/privacy.rs`: telemetry never carries file paths,
//! usernames, hostnames, IPs, emails, window titles, URLs, clipboard,
//! keystrokes, screenshots or prompts; identifiers are per-install salted
//! hashes or absent; nothing from the Keyvault but counts.

pub mod client;
pub mod config;
pub mod events;
mod fsutil;
pub mod identity;
pub mod notice;
pub mod schema;
pub mod sink;

pub use client::{Captured, NoticeMode, Status, Telemetry, global, init};
pub use events::{CallerKind, CallerKindSource, Event, Outcome, TeleportInfo, TeleportOutcome};

/// Captures `event` on the process-wide client. Never blocks, never fails.
pub fn capture(event: events::Event) -> Captured {
    global().capture(event)
}

/// Flushes the process-wide client, waiting at most `timeout`.
pub fn flush(timeout: std::time::Duration) {
    global().flush(timeout)
}

/// Process exit: flush for at most `budget`, spool the rest.
pub fn shutdown(budget: std::time::Duration) {
    global().shutdown(budget)
}
