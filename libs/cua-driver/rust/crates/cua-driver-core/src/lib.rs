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

pub mod capture_mode;
pub mod cdp;
pub mod element_cache;
pub mod element_token;
pub mod ffmpeg_install;
pub mod health_report;
pub mod image_utils;
pub mod page;
pub mod pip_hook;
pub mod protocol;
pub mod cursor_sampler;
pub mod recording;
pub mod recording_loader;
pub mod recording_render;
pub mod recording_tools;
pub mod recording_zoom;
pub mod server;
pub mod session;
pub mod session_tools;
pub mod text_sanitize;
pub mod tool;
pub mod tool_schema;
pub mod tool_args;
pub mod video;
pub mod video_ffmpeg;

pub use recording::RecordingSession;
