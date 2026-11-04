// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Task claims: a lease on a task, held by one session of its agent member.
//!
//! A claim is the same lease as an agent home's ([`cua_volume::lease`]):
//! a holder, an expiry, taken with a compare-and-swap so exactly one
//! claimant wins, renewed while the holder works, and taken over by anyone
//! once it expires. It lives with the thread (`claims.json` in the thread's
//! folder) rather than as a Volume object because the thread's owner is
//! its only writer: the claim and its `task_update` entry are written under
//! the thread's one lock, so a claim never exists without its entry, and
//! claiming never waits on the Volume.
//!
//! The holder is `<member>/<session>@<epoch>`. The epoch is the session's
//! drive epoch: 1 for a plain run, raised by each durable re-drive. The
//! claim IS the durable session's lease: a re-drive at a higher epoch takes
//! the claim over even before it expires ([`ThreadStore::claim`] with the
//! higher epoch), and from then on the old epoch can neither renew nor
//! claim again. One mechanism serves the inbox claim and crash recovery.
//!
//! An expired claim nobody renewed sends its task back to the queue, with a
//! `task_update queued` and a system note about the task
//! ([`ThreadStore::requeue_expired`]).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{Author, Draft, Fold, TaskInfo, TaskState, ThreadEntry, ThreadStore, Update, io};
use cua_spaces::{Error, Result};

/// The file under the thread's folder.
pub const FILE: &str = "claims.json";
/// The error a second claimant gets.
pub const TASK_CLAIMED: &str = "task_claimed";

/// Who holds a claim: one session of an agent member at one drive epoch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Holder {
    pub member: String,
    /// 1-based.
    pub session: u32,
    /// The session's drive epoch (1: a plain run).
    pub epoch: u64,
}

impl Holder {
    pub fn new(member: &str, session: u32, epoch: u64) -> Holder {
        Holder {
            member: member.to_string(),
            session: session.max(1),
            epoch: epoch.max(1),
        }
    }

    /// Parses `<member>/<session>@<epoch>`.
    pub fn parse(s: &str) -> Option<Holder> {
        let (who, epoch) = s.rsplit_once('@')?;
        let (member, session) = who.rsplit_once('/')?;
        Some(Holder {
            member: member.to_string(),
            session: session.parse().ok()?,
            epoch: epoch.parse().ok()?,
        })
    }

    /// Whether `other` is the same session (any epoch).
    pub fn same_session(&self, other: &Holder) -> bool {
        self.member == other.member && self.session == other.session
    }
}

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}@{}", self.member, self.session, self.epoch)
    }
}

/// One claim, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// `<member>/<session>@<epoch>`.
    pub holder: String,
    pub acquired_ms: u64,
    pub expires_ms: u64,
}

impl Claim {
    pub fn holder(&self) -> Option<Holder> {
        Holder::parse(&self.holder)
    }

    pub fn live(&self, now_ms: u64) -> bool {
        self.expires_ms > now_ms
    }
}

/// `task_claimed`: the task is someone else's (or a newer epoch's) now.
pub fn claimed(seq: u64, holder: &str, until_ms: u64) -> Error {
    Error::extension(
        TASK_CLAIMED,
        format!("task #{seq} is claimed by {holder} until {until_ms}"),
    )
}

/// What a claim writes besides the claim itself.
#[derive(Clone, Debug, Default)]
pub struct ClaimEntries {
    /// The task states to record, in order (`claimed`, then maybe
    /// `working`).
    pub states: Vec<TaskState>,
    /// The `task_update` entries' author.
    pub author: Option<Author>,
    /// A note on the updates (for example "taken over at epoch 2").
    pub note: String,
}

impl ThreadStore {
    fn claims_path(&self) -> std::path::PathBuf {
        self.dir.join(FILE)
    }

    /// Every claim (live or not) by task.
    pub fn claims(&self) -> Result<BTreeMap<u64, Claim>> {
        match std::fs::read(self.claims_path()) {
            Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(BTreeMap::new()),
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(io(e)),
        }
    }

    fn write_claims(&self, all: &BTreeMap<u64, Claim>) -> Result<()> {
        cua_home::write_private(&self.claims_path(), &serde_json::to_vec_pretty(all)?).map_err(io)
    }

    /// Claims task `seq` for `holder` for `ttl_ms`, and records it in the
    /// thread (`entries`), all under the thread's lock. Exactly one of any
    /// number of racing claimants wins; the others get [`TASK_CLAIMED`]
    /// with the holder. The same session may claim again (a renewal), and
    /// at a higher epoch it takes the claim over before it expires (a
    /// durable re-drive); a lower epoch than the claim's is refused.
    pub fn claim(
        &self,
        seq: u64,
        holder: &Holder,
        ttl_ms: u64,
        entries: ClaimEntries,
    ) -> Result<(Claim, Vec<ThreadEntry>)> {
        let _l = self.lock()?;
        let fold = self.fold()?;
        let t = fold
            .tasks
            .get(&seq)
            .ok_or_else(|| Error::NotFound(format!("task #{seq} in this thread")))?;
        if t.state.is_closed() {
            return Err(Error::invalid(format!(
                "task #{seq} is {}",
                t.state.as_str()
            )));
        }
        if t.to != holder.member {
            return Err(Error::invalid(format!(
                "task #{seq} is for {}, not {}",
                t.to, holder.member
            )));
        }
        let now = cua_volume::now_ms();
        let mut all = self.claims()?;
        match all.get(&seq) {
            Some(c) if c.live(now) => {
                let cur = c.holder();
                let ok = cur
                    .as_ref()
                    .is_some_and(|h| h.same_session(holder) && holder.epoch >= h.epoch);
                if !ok {
                    return Err(claimed(seq, &c.holder, c.expires_ms));
                }
            }
            Some(_) => {}
            // A claim from before claims were stored (only its entry).
            None if matches!(t.state, TaskState::Claimed | TaskState::Working)
                && t.lease_until_ms.is_some_and(|l| l > now) =>
            {
                let h = Holder::new(&t.to, t.session.unwrap_or(1), 1);
                if !h.same_session(holder) {
                    return Err(claimed(seq, &h.to_string(), t.lease_until_ms.unwrap_or(0)));
                }
            }
            None => {}
        }
        let acquired_ms = match all.get(&seq) {
            Some(c) if c.live(now) && c.holder == holder.to_string() => c.acquired_ms,
            _ => now,
        };
        let claim = Claim {
            holder: holder.to_string(),
            acquired_ms,
            expires_ms: now + ttl_ms,
        };
        all.insert(seq, claim.clone());
        self.write_claims(&all)?;
        let author = entries
            .author
            .unwrap_or_else(|| Author::agent(&holder.member));
        let drafts = entries
            .states
            .iter()
            .map(|state| {
                Draft::update(
                    author.clone(),
                    Update {
                        task: seq,
                        state: *state,
                        session: Some(holder.session),
                        lease_until_ms: Some(claim.expires_ms),
                        note: entries.note.clone(),
                    },
                )
            })
            .collect();
        let written = self.append_locked(drafts)?;
        Ok((claim, written))
    }

    /// Extends every claim `holder` holds (exactly: the same epoch) to
    /// `ttl_ms` from now, when less than half of that is left. `only`:
    /// just these tasks. Returns the tasks renewed.
    pub fn renew(&self, holder: &Holder, ttl_ms: u64, only: Option<&[u64]>) -> Result<Vec<u64>> {
        let h = holder.to_string();
        let now = cua_volume::now_ms();
        let due = |c: &Claim| c.holder == h && c.expires_ms < now + ttl_ms / 2;
        let wanted = |seq: &u64| only.is_none_or(|o| o.contains(seq));
        // Most passes have nothing to renew: look before locking.
        if !self.claims()?.iter().any(|(s, c)| wanted(s) && due(c)) {
            return Ok(vec![]);
        }
        let _l = self.lock()?;
        let mut all = self.claims()?;
        let mut out = vec![];
        for (seq, c) in all.iter_mut() {
            if wanted(seq) && due(c) {
                c.expires_ms = now + ttl_ms;
                out.push(*seq);
            }
        }
        if !out.is_empty() {
            self.write_claims(&all)?;
        }
        Ok(out)
    }

    /// Whether `holder` (exactly) holds task `seq` now.
    pub fn holds(&self, seq: u64, holder: &Holder) -> Result<bool> {
        let now = cua_volume::now_ms();
        Ok(self
            .claims()?
            .get(&seq)
            .is_some_and(|c| c.live(now) && c.holder == holder.to_string()))
    }

    /// Ends the claim on `seq` (the task closed or went back to the queue),
    /// whoever holds it, and appends `drafts` under the same lock.
    pub fn release(&self, seq: u64, drafts: Vec<Draft>) -> Result<Vec<ThreadEntry>> {
        let _l = self.lock()?;
        let mut all = self.claims()?;
        if all.remove(&seq).is_some() {
            self.write_claims(&all)?;
        }
        self.append_locked(drafts)
    }

    /// Sends `member`'s tasks whose claim ran out back to the queue (each
    /// with a `task_update queued` and a system note about the task),
    /// except those in `keep` (a session still working on them). A task
    /// claimed before claims were stored expires by its entry's lease.
    /// Returns the tasks requeued.
    pub fn requeue_expired(&self, member: &str, keep: &[u64]) -> Result<Vec<u64>> {
        let now = cua_volume::now_ms();
        let pick = |fold: &Fold, all: &BTreeMap<u64, Claim>| -> Vec<(u64, String)> {
            fold.open_tasks(Some(member))
                .into_iter()
                .filter(|t| !keep.contains(&t.seq))
                .filter(|t| matches!(t.state, TaskState::Claimed | TaskState::Working))
                .filter_map(|t| match all.get(&t.seq) {
                    Some(c) if !c.live(now) => Some((t.seq, c.holder.clone())),
                    Some(_) => None,
                    None if t.lease_until_ms.is_some_and(|l| l < now) => Some((
                        t.seq,
                        Holder::new(&t.to, t.session.unwrap_or(1), 1).to_string(),
                    )),
                    None => None,
                })
                .collect()
        };
        if pick(&self.fold()?, &self.claims()?).is_empty() {
            return Ok(vec![]);
        }
        let _l = self.lock()?;
        let mut all = self.claims()?;
        let expired = pick(&self.fold()?, &all);
        let mut drafts = vec![];
        for (seq, holder) in &expired {
            all.remove(seq);
            let session = Holder::parse(holder).map_or(1, |h| h.session);
            drafts.push(Draft::update(
                Author::system("dispatch"),
                Update {
                    task: *seq,
                    state: TaskState::Queued,
                    session: None,
                    lease_until_ms: None,
                    note: "the claim's lease ran out".into(),
                },
            ));
            drafts.push(Draft::note(
                "dispatch",
                Some(*seq),
                format!(
                    "Session {session}'s claim on task #{seq} ran out ({holder}); the task is queued again."
                ),
            ));
        }
        self.write_claims(&all)?;
        self.append_locked(drafts)?;
        Ok(expired.into_iter().map(|(s, _)| s).collect())
    }

    /// `tasks` with their live claims filled in (holder, session, lease).
    pub fn with_claims(&self, mut tasks: Vec<TaskInfo>) -> Result<Vec<TaskInfo>> {
        let all = self.claims()?;
        let now = cua_volume::now_ms();
        for t in &mut tasks {
            if let Some(c) = all.get(&t.seq).filter(|c| c.live(now)) {
                t.holder = Some(c.holder.clone());
                t.lease_until_ms = Some(c.expires_ms);
                if let Some(h) = c.holder() {
                    t.session = Some(h.session);
                }
            }
        }
        Ok(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{EntryKind, TaskTo};

    fn store() -> (tempfile::TempDir, ThreadStore) {
        let d = tempfile::tempdir().unwrap();
        let s = ThreadStore::open(d.path(), "local:dev");
        (d, s)
    }

    fn task(s: &ThreadStore, to: &str) -> u64 {
        let mut d = Draft::new(Author::owner(), EntryKind::Message, format!("@{to} go"));
        d.task = Some(TaskTo {
            to: to.into(),
            session: None,
        });
        s.append(d).unwrap().seq
    }

    fn claimed_entries() -> ClaimEntries {
        ClaimEntries {
            states: vec![TaskState::Claimed],
            ..Default::default()
        }
    }

    #[test]
    fn holders_print_and_parse() {
        let h = Holder::new("re.view-1", 2, 7);
        assert_eq!(h.to_string(), "re.view-1/2@7");
        assert_eq!(Holder::parse("re.view-1/2@7"), Some(h));
        assert_eq!(Holder::parse("ada"), None);
        assert_eq!(Holder::new("a", 0, 0).to_string(), "a/1@1");
    }

    #[test]
    fn n_racing_claims_exactly_one_wins() {
        let (_d, s) = store();
        let seq = task(&s, "ada");
        let n = 8u32;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n as usize));
        let handles: Vec<_> = (1..=n)
            .map(|k| {
                let s = s.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    s.claim(seq, &Holder::new("ada", k, 1), 60_000, claimed_entries())
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners: Vec<_> = results.iter().filter(|r| r.is_ok()).collect();
        assert_eq!(winners.len(), 1, "{results:?}");
        let winner = winners[0].as_ref().unwrap().0.holder.clone();
        for r in results.iter().filter(|r| r.is_err()) {
            let e = r.as_ref().unwrap_err();
            assert_eq!(e.tag(), TASK_CLAIMED);
            assert!(e.to_string().contains(&winner), "{e}");
        }
        // One claim, one entry.
        let f = s.fold().unwrap();
        assert_eq!(f.tasks[&seq].state, TaskState::Claimed);
        let updates = s
            .read(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EntryKind::TaskUpdate)
            .count();
        assert_eq!(updates, 1);
    }

    #[test]
    fn a_higher_epoch_takes_over_and_fences_the_old_one() {
        let (_d, s) = store();
        let seq = task(&s, "ada");
        let e1 = Holder::new("ada", 2, 1);
        s.claim(seq, &e1, 60_000, claimed_entries()).unwrap();
        // Another session: refused, naming the holder.
        let e = s
            .claim(seq, &Holder::new("ada", 1, 5), 60_000, claimed_entries())
            .unwrap_err();
        assert_eq!(e.tag(), TASK_CLAIMED);
        assert!(e.to_string().contains("ada/2@1"));
        // The same session re-claims (a renewal) ...
        s.claim(seq, &e1, 60_000, claimed_entries()).unwrap();
        // ... and its re-drive at epoch 2 takes it over before it expires.
        let e2 = Holder::new("ada", 2, 2);
        let (c, written) = s
            .claim(
                seq,
                &e2,
                60_000,
                ClaimEntries {
                    states: vec![TaskState::Working],
                    author: Some(Author::system("dispatch")),
                    note: "taken over at epoch 2".into(),
                },
            )
            .unwrap();
        assert_eq!(c.holder, "ada/2@2");
        assert_eq!(written[0].update.as_ref().unwrap().session, Some(2));
        // Epoch 1 is fenced: no claim, no renewal.
        let e = s.claim(seq, &e1, 60_000, claimed_entries()).unwrap_err();
        assert_eq!(e.tag(), TASK_CLAIMED);
        assert!(s.holds(seq, &e2).unwrap());
        assert!(!s.holds(seq, &e1).unwrap());
        assert!(s.renew(&e1, 60_000, None).unwrap().is_empty());
    }

    #[test]
    fn an_expired_claim_is_requeued_with_a_note_and_claimable_again() {
        let (_d, s) = store();
        let seq = task(&s, "ada");
        let h = Holder::new("ada", 3, 1);
        s.claim(seq, &h, 0, claimed_entries()).unwrap();
        // Kept while a session still works on it.
        assert!(s.requeue_expired("ada", &[seq]).unwrap().is_empty());
        assert_eq!(s.requeue_expired("ada", &[]).unwrap(), vec![seq]);
        assert_eq!(s.fold().unwrap().tasks[&seq].state, TaskState::Queued);
        let note = s
            .read(0, 100)
            .unwrap()
            .into_iter()
            .rfind(|e| e.kind == EntryKind::System)
            .unwrap();
        assert_eq!(note.task_seq, Some(seq));
        assert!(note.text.contains("Session 3"), "{}", note.text);
        assert!(s.claims().unwrap().is_empty());
        // Nothing more to do on the next pass.
        assert!(s.requeue_expired("ada", &[]).unwrap().is_empty());
        s.claim(seq, &Holder::new("ada", 1, 1), 60_000, claimed_entries())
            .unwrap();
    }

    #[test]
    fn renewals_touch_only_the_exact_holder_and_only_when_due() {
        let (_d, s) = store();
        let a = task(&s, "ada");
        let b = task(&s, "ada");
        let h1 = Holder::new("ada", 1, 1);
        let h2 = Holder::new("ada", 2, 1);
        s.claim(a, &h1, 1_000, claimed_entries()).unwrap();
        s.claim(b, &h2, 1_000, claimed_entries()).unwrap();
        assert_eq!(s.renew(&h1, 60_000, None).unwrap(), vec![a]);
        // Not due again (more than half left).
        assert!(s.renew(&h1, 60_000, None).unwrap().is_empty());
        assert_eq!(s.renew(&h2, 60_000, Some(&[a])).unwrap(), Vec::<u64>::new());
        let all = s.claims().unwrap();
        assert!(all[&a].expires_ms > cua_volume::now_ms() + 30_000);
        assert!(all[&b].expires_ms < cua_volume::now_ms() + 2_000);
    }

    #[test]
    fn closed_and_foreign_tasks_are_refused() {
        let (_d, s) = store();
        let seq = task(&s, "ada");
        let e = s
            .claim(seq, &Holder::new("bob", 1, 1), 60_000, claimed_entries())
            .unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        s.release(
            seq,
            vec![Draft::update(
                Author::agent("ada"),
                Update {
                    task: seq,
                    state: TaskState::Done,
                    session: None,
                    lease_until_ms: None,
                    note: String::new(),
                },
            )],
        )
        .unwrap();
        let e = s
            .claim(seq, &Holder::new("ada", 1, 1), 60_000, claimed_entries())
            .unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert_eq!(
            s.claim(999, &Holder::new("ada", 1, 1), 1, claimed_entries())
                .unwrap_err()
                .tag(),
            "not_found"
        );
    }
}
