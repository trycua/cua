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

pub mod browser_eval;
pub mod protocol;
pub mod recording;
pub mod recording_tools;
pub mod server;
pub mod tool;

pub use recording::RecordingSession;
