//! Orphaned children: which ones this host may end, and which it may only
//! list (`cua spaces orphans`).
//!
//! A child is ended by automation only on positive proof that its parent is
//! gone, from this host's own records:
//!
//! - its parent is a Space this host runs (`local:`), and
//! - that parent is not in this host's registry, or it is a different Space
//!   now (deleted and made again under the same name: its creation time
//!   changed), and
//! - that has been so for the whole grace period (15 minutes), across
//!   passes, measured by this host's clock.
//!
//! Anything else is never ended here: a parent on another machine that does
//! not answer is a partition, not proof. Such a child stays and is listed
//! as an orphan; its own lifetime (no longer than its parent's, set when it
//! was made) is the backstop that works with every daemon down. A read of
//! the registry that fails decides nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How long a parent must be provably gone before its children are ended.
pub const GRACE_SECS: u64 = 15 * 60;

/// One registered Space as the reaper sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Its id.
    pub id: String,
    /// Its parent's id, for a child.
    pub parent: Option<String>,
    /// When it was made (unix seconds; 0 unknown).
    pub created_at: u64,
}

/// What to do with one child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Its parent is here: nothing.
    Keep,
    /// Its parent is provably gone, but not for the whole grace yet.
    Waiting {
        /// Seconds left.
        left: u64,
    },
    /// Its parent is provably gone for the grace: end it.
    End {
        /// Why, for the event and the log.
        why: String,
    },
    /// Its parent is not this host's to judge: list it, never end it.
    Orphan {
        /// Why.
        why: String,
    },
}

/// What the reaper remembers between passes (`<home>/spawn/leases.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    /// Child → the creation time of the parent it was made under.
    #[serde(default)]
    pub parent_born: BTreeMap<String, u64>,
    /// Child → when this host first saw its parent gone.
    #[serde(default)]
    pub gone_since: BTreeMap<String, u64>,
}

impl Memory {
    fn path(home: &Path) -> PathBuf {
        home.join("spawn").join("leases.json")
    }

    /// Loads it (empty when missing or unreadable).
    pub fn load(home: &Path) -> Self {
        std::fs::read(Self::path(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Saves it.
    pub fn save(&self, home: &Path) -> std::io::Result<()> {
        let p = Self::path(home);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(&tmp, &p)
    }
}

/// Judges every child of `seen` at `now`, updating `memory`.
pub fn judge(seen: &[Seen], memory: &mut Memory, now: u64) -> BTreeMap<String, Verdict> {
    let by_id: BTreeMap<&str, &Seen> = seen.iter().map(|s| (s.id.as_str(), s)).collect();
    let children: BTreeSet<&str> = seen
        .iter()
        .filter(|s| s.parent.is_some())
        .map(|s| s.id.as_str())
        .collect();
    // Forget what is about Spaces that are gone.
    memory
        .parent_born
        .retain(|c, _| children.contains(c.as_str()));
    memory
        .gone_since
        .retain(|c, _| children.contains(c.as_str()));
    let mut out = BTreeMap::new();
    for s in seen {
        let Some(parent) = &s.parent else { continue };
        let verdict = if !parent.starts_with("local:") {
            Verdict::Orphan {
                why: format!(
                    "its parent {parent} runs on another machine; only that machine or its \
                     own lifetime ends it"
                ),
            }
            .only_if_gone(by_id.contains_key(parent.as_str()))
        } else {
            let here = by_id.get(parent.as_str());
            let born = here.map(|p| p.created_at);
            let first = *memory
                .parent_born
                .entry(s.id.clone())
                .or_insert_with(|| born.unwrap_or(0));
            let same = match (born, first) {
                (Some(b), f) => f == 0 || b == 0 || b == f,
                (None, _) => false,
            };
            if here.is_some() && same {
                memory.gone_since.remove(&s.id);
                Verdict::Keep
            } else {
                let since = *memory.gone_since.entry(s.id.clone()).or_insert(now);
                let gone_for = now.saturating_sub(since);
                if gone_for >= GRACE_SECS {
                    Verdict::End {
                        why: if here.is_some() {
                            format!("its parent {parent} was deleted and made again")
                        } else {
                            format!("its parent {parent} was deleted")
                        },
                    }
                } else {
                    Verdict::Waiting {
                        left: GRACE_SECS - gone_for,
                    }
                }
            }
        };
        out.insert(s.id.clone(), verdict);
    }
    out
}

impl Verdict {
    /// An `Orphan` only while its parent is not registered here at all (a
    /// remote parent this host also knows about is kept).
    fn only_if_gone(self, parent_known_here: bool) -> Self {
        if parent_known_here {
            Verdict::Keep
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, parent: Option<&str>, at: u64) -> Seen {
        Seen {
            id: id.into(),
            parent: parent.map(str::to_owned),
            created_at: at,
        }
    }

    #[test]
    fn a_child_ends_only_after_its_local_parent_is_gone_for_the_grace() {
        let mut m = Memory::default();
        let with = [s("local:p", None, 10), s("local:c", Some("local:p"), 20)];
        assert_eq!(judge(&with, &mut m, 100)["local:c"], Verdict::Keep);
        let without = [s("local:c", Some("local:p"), 20)];
        assert_eq!(
            judge(&without, &mut m, 1000)["local:c"],
            Verdict::Waiting { left: GRACE_SECS }
        );
        assert!(matches!(
            judge(&without, &mut m, 1000 + GRACE_SECS - 1)["local:c"],
            Verdict::Waiting { left: 1 }
        ));
        assert!(matches!(
            judge(&without, &mut m, 1000 + GRACE_SECS)["local:c"],
            Verdict::End { .. }
        ));
    }

    #[test]
    fn a_parent_that_comes_back_resets_the_clock() {
        let mut m = Memory::default();
        let with = [s("local:p", None, 10), s("local:c", Some("local:p"), 20)];
        let without = [s("local:c", Some("local:p"), 20)];
        judge(&with, &mut m, 0);
        judge(&without, &mut m, 100);
        judge(&with, &mut m, 200);
        assert!(matches!(
            judge(&without, &mut m, 300 + GRACE_SECS - 10)["local:c"],
            Verdict::Waiting { .. }
        ));
    }

    #[test]
    fn a_parent_made_again_under_the_same_name_is_not_the_parent() {
        let mut m = Memory::default();
        judge(
            &[s("local:p", None, 10), s("local:c", Some("local:p"), 20)],
            &mut m,
            0,
        );
        let again = [s("local:p", None, 99), s("local:c", Some("local:p"), 20)];
        assert!(matches!(
            judge(&again, &mut m, 50)["local:c"],
            Verdict::Waiting { .. }
        ));
        assert!(matches!(
            judge(&again, &mut m, 50 + GRACE_SECS)["local:c"],
            Verdict::End { .. }
        ));
    }

    #[test]
    fn a_remote_parent_is_never_proof() {
        let mut m = Memory::default();
        let seen = [s("local:c", Some("relay:abcdefgh"), 20)];
        for t in [0, GRACE_SECS * 10] {
            assert!(matches!(
                judge(&seen, &mut m, t)["local:c"],
                Verdict::Orphan { .. }
            ));
        }
    }

    #[test]
    fn memory_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Memory::default();
        judge(&[s("local:c", Some("local:p"), 20)], &mut m, 500);
        m.save(dir.path()).unwrap();
        let mut back = Memory::load(dir.path());
        assert_eq!(back, m);
        assert!(matches!(
            judge(
                &[s("local:c", Some("local:p"), 20)],
                &mut back,
                500 + GRACE_SECS
            )["local:c"],
            Verdict::End { .. }
        ));
    }
}
