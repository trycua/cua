// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Positions in the replication log and the history of who owned it.
//!
//! An *era* is a stretch of the log with one owner. A takeover or a
//! hand-back starts a new era at `epoch + 1` and records its `base`: the
//! newest position the new owner had applied. Whatever another machine holds
//! past a later era's base was written by an owner that era did not see (the
//! divergent tail): it is rebased, never replayed.

use serde::{Deserialize, Serialize};

use super::MachineId;

/// A position in the log: records are ordered by `(epoch, seq)`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Pos {
    pub epoch: u64,
    pub seq: u64,
}

impl Pos {
    pub const ZERO: Pos = Pos { epoch: 0, seq: 0 };
}

impl std::fmt::Display for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.epoch, self.seq)
    }
}

/// One era.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraRec {
    pub epoch: u64,
    pub owner: MachineId,
    /// What the owner had applied when it took the era (zero for the first).
    pub base: Pos,
    /// Wall time it started (display only).
    pub at_ms: u64,
}

/// The eras a machine knows, oldest first, of one lineage (the id of the
/// volume's origin: two machines that each started a volume alone have
/// different lineages until one joins the other).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eras {
    pub lineage: String,
    pub recs: Vec<EraRec>,
}

impl Eras {
    /// A volume that starts here, owned by `owner`.
    pub fn genesis(owner: &str, now_ms: u64) -> Eras {
        Eras {
            lineage: crate::new_id(),
            recs: vec![EraRec {
                epoch: 1,
                owner: owner.to_string(),
                base: Pos::ZERO,
                at_ms: now_ms,
            }],
        }
    }

    pub fn current(&self) -> &EraRec {
        self.recs.last().expect("an era always exists")
    }

    pub fn epoch(&self) -> u64 {
        self.current().epoch
    }

    pub fn owner(&self) -> &str {
        &self.current().owner
    }

    /// Adds `rec` when it is newer. Returns whether it was added.
    pub fn adopt(&mut self, rec: EraRec) -> bool {
        if rec.epoch <= self.epoch() {
            return false;
        }
        self.recs.push(rec);
        true
    }

    /// Adopts the history `other` carries when it extends ours (same
    /// lineage): every era newer than ours.
    pub fn adopt_from(&mut self, other: &[EraRec]) -> bool {
        let mut any = false;
        for r in other {
            if self.adopt(r.clone()) {
                any = true;
            }
        }
        any
    }

    /// Where a machine whose newest applied record is `p` stops agreeing
    /// with this history: `Some(keep)` when it holds records past the base
    /// of an era that came after it (those are its divergent tail; it keeps
    /// everything up to `keep`).
    pub fn divergence(&self, p: Pos) -> Option<Pos> {
        self.recs
            .iter()
            .filter(|r| r.epoch > p.epoch && r.epoch > 1)
            .filter(|r| p > r.base)
            .map(|r| r.base)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(epoch: u64, owner: &str, base: (u64, u64)) -> EraRec {
        EraRec {
            epoch,
            owner: owner.into(),
            base: Pos {
                epoch: base.0,
                seq: base.1,
            },
            at_ms: 0,
        }
    }

    #[test]
    fn a_machine_ahead_of_a_later_eras_base_diverges() {
        let mut e = Eras::genesis("laptop", 0);
        assert_eq!(e.divergence(Pos { epoch: 1, seq: 50 }), None);
        // The studio took over having applied up to 1.40.
        assert!(e.adopt(rec(2, "studio", (1, 40))));
        assert!(!e.adopt(rec(2, "other", (1, 1))), "an old epoch is ignored");
        // The laptop applied 1.50 (ten records the studio never saw).
        assert_eq!(
            e.divergence(Pos { epoch: 1, seq: 50 }),
            Some(Pos { epoch: 1, seq: 40 })
        );
        // One that stopped at 1.40 or earlier agrees.
        assert_eq!(e.divergence(Pos { epoch: 1, seq: 40 }), None);
        assert_eq!(e.divergence(Pos { epoch: 1, seq: 7 }), None);
        // A machine already in the new era never diverges.
        assert_eq!(e.divergence(Pos { epoch: 2, seq: 9 }), None);
    }

    #[test]
    fn the_earliest_base_wins_across_several_eras() {
        let mut e = Eras::genesis("a", 0);
        e.adopt(rec(2, "b", (1, 40)));
        e.adopt(rec(3, "c", (1, 30)));
        assert_eq!(
            e.divergence(Pos { epoch: 1, seq: 45 }),
            Some(Pos { epoch: 1, seq: 30 })
        );
        assert_eq!(e.divergence(Pos { epoch: 1, seq: 30 }), None);
    }
}
