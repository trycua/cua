//! The typed Spaces control-plane client, over any MCP transport.
//!
//! Folded from `libs/spaces-sdk/rust/crates/cua-spaces-core`. It is the
//! client half of the contract: typed records, the honesty booleans, the
//! teleport consent gate and line framing, written against the
//! [`ToolTransport`] seam so a Swift / TypeScript / Tauri consumer that talks
//! MCP to `cua daemon mcp` decodes exactly what this crate decodes.
//!
//! The Python-spawning `StdioTransport` is gone: there is no Python server to
//! spawn. [`InProcessTransport`] drives the Rust server ([`crate::mcp`])
//! directly, and [`ScriptedTransport`] replays the conformance script, so the
//! same `cua.control.session/1` document is produced against both (see
//! `tests/client_conformance.rs`).
//!
//! ## The two transport bugs, made structurally impossible
//!
//! * **§1, the framer that dropped bytes after a newline.** See
//!   [`framing`]: the buffer is private with no accessor, so there is no API
//!   through which a caller could drop the remainder.
//! * **§3, failures shaped like values.** See [`transport`]: no type holds a
//!   payload and a failure flag at once, and the one function that reads
//!   `isError` returns a `Result`.
//!
//! ## The honesty apparatus
//!
//! Six flags, all plain `bool` fields, never optional, never inferred from the
//! presence of a value: [`model::AgentEvent::is_inferred`],
//! [`model::SchedulerFacts::is_server_backed`],
//! [`model::TransferLimits::is_server_published`],
//! [`model::AgentRunHandle::approvals_are_enforced`],
//! [`model::ProviderCapabilities::server_backstop`] and
//! [`model::AgentKindInfo::is_production_ready`]. Plus the teleport
//! [`teleport::Approval`] type-gate and the `nil`-selection rule, both in
//! [`teleport`].

pub mod conformance;
pub mod control;
pub mod coverage;
pub mod error;
pub mod framing;
pub mod metadata;
pub mod model;
pub mod persistent;
pub mod teleport;
pub mod transport;

pub use control::{Connection, Space};
pub use error::{Result, SpacesError};
pub use framing::LineFramer;
pub use transport::{InProcessTransport, ScriptedTransport, ToolTransport};

/// The version of this client.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
