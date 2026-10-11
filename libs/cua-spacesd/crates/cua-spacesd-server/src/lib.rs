// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! gRPC, gRPC-Web and HTTP server core of cua-spacesd.
//!
//! One port serves the `cua.env.v1` contract (native gRPC over h2c and
//! gRPC-Web over HTTP/1.1), gRPC reflection, and the plain-HTTP side channels
//! (`/health`, `/files`, `/mcp`, `/tunnel`, `/hotspot`, `/media`). The desktop
//! services (Computer, Windows, Accessibility, Stream, Presence) plug in
//! through [`ServiceProvider`]; see the crate README.

pub mod access_log;
pub mod auth;
pub mod browser_guard;
pub mod config;
pub mod context;
pub mod error;
pub mod filesystem;
pub mod host_spaces;
pub mod http;
pub mod peer;
pub mod process;
pub mod provider;
pub mod relay_account;
pub mod server;
pub mod services;
pub mod telemetry;
pub mod token_file;
pub mod util;

pub use axum;
pub use tonic;

pub use auth::{caller, CallerIdentity, TicketClaims, TicketError, TicketScope, ViewerGrant};
pub use config::ServerConfig;
pub use context::{AudioUplinkPolicy, ServerContext};
pub use provider::ServiceProvider;
pub use server::{bind, spawn_local, RouteManifest, Server, ServerBuilder};
