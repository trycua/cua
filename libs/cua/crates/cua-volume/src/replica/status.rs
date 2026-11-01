// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the app and the CLI show about the replicated volume.

use serde::{Deserialize, Serialize};

use super::detector::Health;
use super::era::Pos;
use super::roles::Role;

/// How a write stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "copies", rename_all = "snake_case")]
pub enum Durability {
    /// This many machines hold it (2 or more when replication is on).
    Replicated(usize),
    /// Only this many (1): the other machine was away or slow.
    Local(usize),
    /// Saved on this machine only, waiting for the owner.
    Pending,
}

/// A machine as the status shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerView {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub health: String,
    pub connected: bool,
    pub owner: bool,
    /// Records it has applied that this machine knows of.
    pub applied: Pos,
    /// Records behind the owner's tail (0: caught up).
    pub lag: u64,
    pub problem: Option<String>,
}

/// A change waiting for another machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingView {
    pub key: String,
    pub size: u64,
    pub since_ms: u64,
    pub deleted: bool,
}

/// The whole picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplicaStatus {
    pub me: String,
    pub name: String,
    pub role: Role,
    pub owner: String,
    pub owner_name: String,
    pub i_own: bool,
    pub epoch: u64,
    pub applied: Pos,
    pub tail: Pos,
    pub peers: Vec<PeerView>,
    pub pending: Vec<PendingView>,
    pub pending_bytes: u64,
    /// Machines (this one included) holding everything up to the owner's tail.
    pub copies_now: usize,
    /// Copies wanted.
    pub copies_wanted: usize,
    /// Records the owner holds that fewer than `copies_wanted` machines hold.
    pub unreplicated: u64,
    pub volume_bytes: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub takeovers: u64,
    pub handbacks: u64,
    pub fenced_writes: u64,
    pub conflict_copies: u64,
}

impl Health {
    pub fn word(self) -> &'static str {
        match self {
            Health::Alive => "up",
            Health::Suspect => "slow",
            Health::Dead => "down",
            Health::Away => "away",
        }
    }
}
