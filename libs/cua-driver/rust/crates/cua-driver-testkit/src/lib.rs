//! Shared test harness for cua-driver integration tests.
//!
//! Before this crate, every `tests/*.rs` re-implemented the same machinery:
//! `workspace_root()` / `driver_binary()` (×9), the JSON-RPC-over-stdio client
//! (`send`/`call`/`init`, ×10), the Windows kill-on-close Job Object reaper
//! (×2), and the `result_text`/`is_error` response accessors. This crate is the
//! single home for all of it.
//!
//! ## Two transports, one shape
//! cua-driver is driven two ways, and a test should be able to target either:
//!   - **MCP** ([`McpDriver`]) — one long-lived `cua-driver` stdio proxy backed
//!     by a test-owned daemon. State persists in the daemon.
//!     Returns the `{"result":{"content",…,"structuredContent"}}` envelope.
//!   - **CLI** ([`CliDriver`]) — a fresh `cua-driver call <tool> <json>` process
//!     per action, backed by the same test-owned daemon. Prints
//!     `structuredContent` (or text) directly, NOT the JSON-RPC envelope.
//!
//! Both implement [`Driver`] and normalize their differing payloads into one
//! [`ToolResponse`], so a scenario reads `resp.text()` / `resp.structured()` /
//! `resp.is_error()` regardless of transport. This is what makes the
//! transport axis (CLI vs MCP) testable instead of MCP-only.
//!
//! ## Child hygiene
//! [`ChildReaper`] kills every spawned child on drop. On Windows it also assigns
//! them to a `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job, so the OS reaps the whole
//! tree even on panic / SIGKILL / Ctrl-C — no orphaned windows or held ports.
//!
//! ## Relocated runners
//!
//! The testkit normally discovers the driver and staged apps under the Cargo
//! workspace. CI runners that build those artifacts in an immutable or remote
//! workspace can override the paths with `CUA_TEST_DRIVER_BIN`,
//! `CUA_TEST_APPS_ROOT`, and `CUA_TEST_WORKSPACE_ROOT`. These variables affect
//! tests only; they are never read by the shipped driver.

pub mod ax;
mod browser_fixture;
mod cli;
mod daemon;
mod driver;
pub mod e2e;
mod journal;
mod mcp;
pub mod observer;
mod paths;
mod raw;
mod reaper;
mod response;
pub mod sentinel;
mod windows_setup;

pub use browser_fixture::BrowserFixtureServer;
pub use cli::CliDriver;
pub use driver::{BehaviorRecording, Driver};
pub use journal::FixtureJournal;
pub use mcp::McpDriver;
pub use paths::{driver_binary, harness_app, workspace_root};
pub use raw::RawDriver;
pub use reaper::{spawn_in_job, ChildReaper};
pub use response::ToolResponse;

use std::time::Duration;

/// Hard ceiling on any single tool call: a hung driver becomes a fast, localized
/// failure instead of an indefinite wall-clock hang.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(25);
