// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Test rig for the replicated Volume (not shipped).
//!
//! - [`shape`]: a userspace TCP proxy that makes one path bad (latency,
//!   jitter, bandwidth, loss as TCP feels it, blackholes, resets, flaps),
//!   scoped to the ports it is given.
//! - [`node`]: one machine's replica node and durable-agent supervisor on
//!   real TCP, with a line control protocol, as a library and as the
//!   `replica-node` process.
//! - [`driver`]: the simulated durable agent (N steps, resumable, checksummed).
//! - [`chaos`]: the opt-in matrix over real sockets: impairments, process
//!   faults, scale.

#[cfg(unix)]
pub mod chaos;
#[cfg(unix)]
pub mod driver;
#[cfg(unix)]
pub mod node;
#[cfg(unix)]
pub mod shape;
