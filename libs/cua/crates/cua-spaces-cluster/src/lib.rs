// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Batch work on all of an account's machines (source-available,
//! FSL-1.1-MIT).
//!
//! The machines run the MIT job runner ([`cua_cluster::node`], served by
//! their daemon as `HostJobsService`); this crate is the coordinator that
//! places work on them ([`coordinator::Coordinator`]): a durable SQLite job
//! queue, Modal-style placement from memory, one feed (heartbeat) per
//! host, leases on its own monotonic clock, reconcile and fencing, metrics,
//! and every state change recorded in a journal first (the relay's pool
//! log). [`pool`] is the agent each signed-in machine runs: which machine
//! coordinates (a lease on the relay), taking over from the log, and the
//! requests the account's devices send. See
//! `rfcs/pool-self-coordination.md`.

pub mod coordinator;
pub mod pool;
