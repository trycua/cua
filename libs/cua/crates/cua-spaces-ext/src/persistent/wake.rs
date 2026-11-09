// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Agent-to-agent chatter limits. An agent's message wakes another agent
//! only through an explicit `@mention`, and a chain of such wakes stops at
//! [`MAX_HOPS`] hops from the person who started it; a Space also allows at
//! most [`MAX_WAKES_PER_MINUTE`] agent-to-agent wakes a minute. A person's
//! message resets the chain (their tasks carry no hops). When a limit
//! trips, the message is still posted (it just wakes nobody) and the
//! thread gets one plain note per episode; the daemon logs it too.

use std::collections::{HashMap, HashSet, VecDeque};

use super::Persistent;
use crate::Result;
use crate::thread::{Draft, ThreadStore};

/// Agent-to-agent hops a chain may go (a person asks A, A asks B, B asks C, C asks D).
pub const MAX_HOPS: u32 = 3;
/// Agent-to-agent wakes one Space allows a minute.
pub const MAX_WAKES_PER_MINUTE: usize = 20;

/// The note the thread gets when a limit trips.
pub const PAUSED_NOTE: &str = "Paused agent to agent messages for a moment";

/// The two limits (the defaults are the product's; not a user setting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WakeLimits {
    pub max_hops: u32,
    pub per_minute: usize,
}

impl Default for WakeLimits {
    fn default() -> Self {
        WakeLimits {
            max_hops: MAX_HOPS,
            per_minute: MAX_WAKES_PER_MINUTE,
        }
    }
}

/// Per process: each Space's recent agent-to-agent wakes and the Spaces already told.
#[derive(Default)]
pub(crate) struct LiveWake {
    limits: WakeLimits,
    wakes: HashMap<String, VecDeque<u64>>,
    noted: HashSet<String>,
}

/// Why a wake was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tripped {
    Depth,
    Rate,
}

impl Persistent {
    /// Sets the limits (tests and hosts; the defaults are [`WakeLimits::default`]).
    pub async fn set_wake_limits(&self, limits: WakeLimits) {
        self.live.lock().await.wake.limits = limits;
    }

    /// A person's message: the chain starts over, so the next trip says so again.
    pub(crate) async fn wake_reset(&self, space: &str) {
        self.live.lock().await.wake.noted.remove(space);
    }

    /// Whether an agent's message that would wake `n` agents, `hops` hops
    /// from a person, may. Allowed wakes are counted. A refusal is logged
    /// and noted in the thread once per episode.
    pub(crate) async fn wake_allowed(
        &self,
        store: &ThreadStore,
        hops: u32,
        n: usize,
    ) -> Result<bool> {
        let now = cua_volume::now_ms();
        let (tripped, first) = {
            let mut live = self.live.lock().await;
            let w = &mut live.wake;
            let limits = w.limits;
            let space = store.space().to_string();
            let q = w.wakes.entry(space.clone()).or_default();
            while q.front().is_some_and(|t| now.saturating_sub(*t) >= 60_000) {
                q.pop_front();
            }
            let tripped = if hops > limits.max_hops {
                Some(Tripped::Depth)
            } else if q.len() + n > limits.per_minute {
                Some(Tripped::Rate)
            } else {
                None
            };
            match tripped {
                Some(t) => (Some(t), w.noted.insert(space)),
                None => {
                    q.extend(std::iter::repeat_n(now, n));
                    (None, false)
                }
            }
        };
        let Some(t) = tripped else {
            return Ok(true);
        };
        tracing::warn!(
            space = store.space(),
            reason = ?t,
            hops,
            "paused agent to agent messages"
        );
        if first {
            store.append(Draft::note("limits", None, PAUSED_NOTE))?;
        }
        Ok(false)
    }
}
