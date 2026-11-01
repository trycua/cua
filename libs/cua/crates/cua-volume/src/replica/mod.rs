// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Volume that lives on your machines: no bucket, no server.
//!
//! With no bucket configured and two or more machines signed in, a durable
//! agent's Volume is kept on the most reliable machine (the owner) and one
//! more (a replica), so closing the laptop loses nothing and the agent keeps
//! its files. See `DESIGN.md` of the just-works-machines work for the
//! reasoning; this is the map.
//!
//! | Module | What it owns |
//! |---|---|
//! | [`era`] | log positions and the history of owners (epochs, bases, divergence) |
//! | [`oplog`] | the replication log (what the owner did, in order) |
//! | [`detector`] | is that machine still there: adaptive threshold, hysteresis, goodbyes |
//! | [`roles`] | machines, reliability facts, who owns and who holds copies |
//! | [`merge`] | what to do when two machines changed one key |
//! | [`migrate`] | turning replication on for an existing Volume: format marker, backup, changes made behind its back |
//! | [`wire`], [`link`], [`transport`] | the peer protocol, one connection, how machines reach each other |
//! | [`outbox`] | changes saved here that the owner has not taken yet |
//! | [`node`] | the runtime: owner writes, log shipping, catch-up, takeover, hand-back |
//! | [`words`], [`status`] | what the app and the CLI say |
//! | [`supervision`] | which machine runs a durable agent, and who takes it over |
//! | [`simnet`] | an in-memory network with latency, loss, partitions and held bytes (tests, chaos) |

mod apply;
pub mod backend;
pub mod detector;
mod election;
pub mod era;
pub mod link;
pub mod merge;
pub mod migrate;
pub mod node;
pub mod oplog;
pub mod outbox;
mod owner;
pub mod roles;
mod ship;
pub mod simnet;
pub mod status;
pub mod supervision;
pub mod transport;
pub mod wire;
pub mod words;
mod xfer;

pub use backend::ReplicatedBackend;
pub use detector::{Health, Timing};
pub use era::{EraRec, Eras, Pos};
pub use node::{Config, MachineView, PowerEvent, ReplicaNode};
pub use roles::{Facts, Layout, Member, Role};
pub use status::{Durability, ReplicaStatus};
pub use transport::{TcpTransport, Transport};

/// A machine's id: its machine id on the account.
pub type MachineId = String;
