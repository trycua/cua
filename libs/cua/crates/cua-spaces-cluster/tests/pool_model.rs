// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A model-checked spec of the pool's self-coordination protocol
//! (`rfcs/pool-self-coordination.md`, "Coordination: lease, fenced log,
//! mailbox" and "Why it is correct"), checked exhaustively with
//! [Stateright](https://www.stateright.rs) over a bounded state space.
//!
//! # Parts and the code they abstract
//!
//! | Model action | Real code |
//! |---|---|
//! | `Acquire` / `Expire` / `Renew` | relay lease `acquire` (epoch += 1, fsynced) and renew; expiry is a free nondeterministic step, so no property depends on timing (skew, jumps, pauses) |
//! | relay side of every append (`Pool::append`) | relay log `append(epoch, from_seq, entries)`: the epoch check (fenced), the `from_seq == head` check with idempotent retry (conflict), the refusal of a second terminal entry |
//! | coordinator side of every append (`Pool::decide`, `Op::Retry`) | the holder's writer: evaluate a change, append it, and only after the relay acknowledged it apply it and act (`coordinator/mod.rs` `persist`/`writer`; `store.rs` `apply` gives the conditional updates that `job_view` folds); a lost answer (`reply_lost`) leaves the change pending and it is sent again |
//! | `Op::TakeSubmit` | `Coordinator::submit` behind the relay mailbox, deduplicated by the idempotency key; the client is answered only after the submit is in the log |
//! | `Op::Dispatch` | `Coordinator::plan` + `send`: the attempt is the job's newest + 1, `Dispatched` is persisted before the batch goes out |
//! | `Op::GiveUp` | `plan`: a job out of attempts ends `error` (`EndQueued`) |
//! | `Op::Query` | the per-host long-poll feed (`poll`/`on_feed`) and `reconcile` (`GetHostJob`): a snapshot of the host's record, delivered later (so possibly stale) |
//! | `Op::Rec` | `on_record` (completion only for the current node and attempt; a result of any other attempt is a suppressed duplicate; a non-current running attempt is canceled) and `reconcile` (an attempt the host never got is fenced and requeued) |
//! | `Op::Unreachable` | `on_unreachable`: requeue with a fence (`Requeue` + `FenceAdd`, delivered as a cancel) |
//! | lost append request (`Op::Retry`) | also a `rekey` refusal: the log is sealed (`pool::keys::SealedLog`) and the relay refuses data sealed under an old key generation; the holder seals again and resends, as after a lost request. Who holds which keys is checked in `pool_keys_model.rs`. Also a refusal for space (`snapshot_needed`, `quota`, `disk_reserve`): nothing is appended; the holder snapshots (the snapshot only shortens what a new holder replays, which this model does not cut) and resends, as after a lost request |
//! | `Crash` | a coordinator process dies and restarts: all volatile state is gone; it rebuilds from the log on its next `Acquire` |
//! | host `Deliver(Run)` / `Deliver(Cancel)` / `Finish` | `cua-cluster/src/node.rs` `NodeJobs::run_batch` (idempotent per job id + attempt, refuses a lower attempt, a higher one supersedes a lower running one), `cancel` (a tombstone for an attempt not seen yet), `execute` (a superseded attempt's result is dropped) |
//! | client `Submit` / `Deliver(Ack)` / `Observe` | a client submitting through the mailbox with an idempotency key (the job id), retrying until acknowledged, and reading results only from the log |
//!
//! The network is a set of messages: any message can be delivered in any
//! order (reorder) or dropped (loss); a host behind a partition is a host
//! whose messages are dropped for a while. Host answers can also be
//! delivered and kept (duplicated). Duplicates of `Run` and `Cancel` are
//! not modeled because they are provably no-ops: after the first delivery
//! the host's newest attempt is at least the message's, and node.rs ignores
//! a lower or equal one. At most one answer per host and job is in flight
//! (a feed is read in order). A coordinator whose lease expired keeps acting
//! on its stale view (a paused process), including on host answers, until
//! the relay refuses one of its appends or renews.
//!
//! # Abstractions and assumptions
//!
//! - One entry per append (the real writer appends a group; a group is
//!   atomic at the relay, so one entry is the finer interleaving).
//! - A coordinator's view is exactly the log prefix it read at `Acquire`
//!   plus its own acknowledged appends (`relay.log[..seq]`): its SQLite
//!   state is a deterministic fold of that prefix (`job_view`).
//! - No `Running` entry in the log, no host capacity or placement ranking
//!   (any host may be chosen), no user cancel, no durable Spaces, no
//!   snapshot/truncation or hash chain, no access checks (invariant 5),
//!   one account, no lease priority/yield. A non-zero exit is a result like
//!   any other (`Done`); `Error` is an infrastructure failure that is
//!   retried. Every requeue uses up an attempt (the real code does not
//!   count a host-side cancel as a retry).
//! - The relay does not lose the epoch or the log (they are fsynced); it
//!   may lose its in-memory mailbox (`LoseMailbox`).
//! - Hosts do not crash; a host restart ends running jobs with `error`
//!   (node.rs `recover`), which is `Finish { ok: false }`.
//! - Hosts and contenders are interchangeable: the checker visits one state
//!   per renaming (`State::representative`).
//!
//! # Liveness ("no lost jobs")
//!
//! Faults (loss, duplication, lease expiry of a live holder, coordinator
//! crashes, false suspicions of hosts, mailbox loss, lost append answers)
//! may happen until the single `StopFaults` step; after it the lease only
//! expires when its holder is dead (a live holder renews in time), and no
//! message is lost or duplicated. Stateright checks `eventually` properties
//! on terminal paths only (paths that end in a state with no successor;
//! a path that ends in a cycle is not checked, see `Property::eventually`).
//! Every terminal state here is after `StopFaults` with an empty network,
//! so the property says: every maximal fault-free continuation that
//! terminates has every job submitted, acknowledged, ended in the log (a
//! result or `GaveUp`) and seen by the client. It does not rule out a
//! livelock (a cycle that never ends a job). The last epoch is kept for
//! recovery: before `StopFaults` the lease of the last epoch neither
//! expires nor loses its holder, so a dead holder can always be replaced.
//!
//! # Finding
//!
//! The design as first written let a stale holder cancel the new holder's
//! current attempt (`finding_stale_holder_cancels_current_attempt`); safety
//! held, but the attempt ran again. `Mutations::fix_stale_cancel` is the
//! fix, checked by `fix_stale_cancel_keeps_current_attempts`; the real
//! coordinator applies it (`on_record` leaves a running attempt newer than
//! any it dispatched alone and counts it as `stale_view`). Here a job a
//! stale holder never saw has newest attempt 0 in its view, so the fix
//! covers it too; the code first covered only jobs in its view, and the
//! simulation found that gap (a job above the coordinator's newest id is
//! now left alone as well).
//!
//! # Running
//!
//! `cargo test -p cua-spaces-cluster --test pool_model` checks the default
//! configuration (1 job, 2 attempts, 2 epochs: one takeover with a stale
//! old holder) in debug in about two minutes (3.5M states). The larger ones
//! (3 epochs, 41.6M states; 2 jobs, partial; the fix over the full space)
//! are `#[ignore]`d: run
//! them with `cargo test --release -p cua-spaces-cluster --test pool_model
//! -- --ignored`. `POOL_MODEL_MAX_STATES=<n>` caps a run.

use std::collections::BTreeSet;
use std::time::Instant;

use stateright::{Checker, Model, Property};

const HOSTS: u8 = 2;
/// A property the design as written does not have (see `finding_*`).
const FINDING: &str = "cancels hit only superseded attempts";
const CONTENDERS: u8 = 2;

/// Protocol rules a mutation test turns off to show the model has teeth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Mutations {
    /// (a) the relay appends without checking the epoch.
    skip_epoch_check: bool,
    /// (b) the relay accepts a second terminal entry and the coordinator
    /// records a result of any attempt.
    allow_second_result: bool,
    /// (c) the coordinator sends a dispatch before the relay stored it.
    send_before_persist: bool,
    /// Not a mutation but the proposed fix of a finding: a coordinator
    /// cancels a running attempt it does not know as current only when the
    /// attempt is not newer than the newest one in its view (a newer one
    /// means a newer holder dispatched it, so this coordinator is stale).
    fix_stale_cancel: bool,
}

#[derive(Clone, Debug)]
struct Pool {
    jobs: u8,
    max_attempts: u8,
    max_epoch: u8,
    mutations: Mutations,
    /// Check only this property (mutation tests stop at its first discovery).
    only: Option<&'static str>,
}

/// The result value an attempt produced: it names the host and the attempt,
/// so values of different attempts differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Value {
    host: u8,
    attempt: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Kind {
    Submit { job: u8 },
    Dispatched { job: u8, host: u8, attempt: u8 },
    Requeue { job: u8, host: u8, attempt: u8 },
    Complete { job: u8, attempt: u8, value: Value },
    GaveUp { job: u8 },
}

impl Kind {
    fn job(&self) -> u8 {
        match *self {
            Kind::Submit { job }
            | Kind::Dispatched { job, .. }
            | Kind::Requeue { job, .. }
            | Kind::Complete { job, .. }
            | Kind::GaveUp { job } => job,
        }
    }

    fn terminal(&self) -> bool {
        matches!(self, Kind::Complete { .. } | Kind::GaveUp { .. })
    }

    /// What a reader of the log sees as the job's outcome.
    fn outcome(&self) -> Option<Outcome> {
        match *self {
            Kind::Complete { value, .. } => Some(Outcome::Result(value)),
            Kind::GaveUp { .. } => Some(Outcome::GaveUp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Outcome {
    Result(Value),
    GaveUp,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Entry {
    epoch: u8,
    kind: Kind,
    /// Ghost: the contender that appended it.
    author: u8,
    /// Ghost: the relay's epoch when it was appended.
    relay_epoch: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Relay {
    epoch: u8,
    holder: Option<u8>,
    expired: bool,
    /// Ghost: `grants[e - 1]` is the contender granted epoch `e`.
    grants: Vec<u8>,
    log: Vec<Entry>,
    mailbox: BTreeSet<u8>,
}

/// A contender's volatile state while it believes it holds the lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Lead {
    epoch: u8,
    /// Its view is `relay.log[..seq]`.
    seq: u8,
    /// A change decided and sent (or about to be sent) to the relay, not yet
    /// acknowledged.
    pending: Option<Kind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum HSt {
    None,
    Running,
    Done,
    Error,
    Canceled,
}

/// A host's ledger record of a job: the newest attempt seen and its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct HRec {
    attempt: u8,
    st: HSt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Msg {
    Run {
        host: u8,
        job: u8,
        attempt: u8,
    },
    Cancel {
        host: u8,
        job: u8,
        attempt: u8,
    },
    /// A host's record as its feed or `GetHostJob` returned it, on its way
    /// to the coordinator (so possibly stale); `attempt` 0 is "no record".
    Rec {
        host: u8,
        job: u8,
        attempt: u8,
        st: HSt,
    },
    Ack {
        job: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct State {
    faults: bool,
    relay: Relay,
    coords: Vec<Option<Lead>>,
    hosts: Vec<Vec<HRec>>,
    net: BTreeSet<Msg>,
    acked: Vec<bool>,
    observed: Vec<Option<Outcome>>,
    /// Ghost: (job, value) of every result a host produced.
    produced: BTreeSet<(u8, Value)>,
    /// Ghost: a host's newest attempt of a job went down in some step.
    regressed: bool,
    /// Ghost flags for the reachability (`sometimes`) properties only.
    seen: Seen,
}

/// Ghost flags that only `sometimes` properties read. They are left out of
/// state identity (`Hash`/`Eq`), so they do not multiply the state space;
/// the cost is that an example may be missed (never invented), and the test
/// asserts every example is found.
#[derive(Clone, Copy, Debug, Default, PartialOrd, Ord)]
struct Seen {
    /// A stale coordinator was refused by the relay.
    fenced: bool,
    /// A host's result of a non-current attempt was dropped.
    suppressed: bool,
}

impl PartialEq for Seen {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Seen {}

impl std::hash::Hash for Seen {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Action {
    StopFaults,
    Submit(u8),
    Observe(u8),
    LoseMailbox(u8),
    Expire,
    Acquire(u8),
    Renew(u8),
    Crash(u8),
    /// A step of a coordinator that believes it holds the lease. When the
    /// step appends, `reply_lost` loses the relay's answer.
    Coord {
        c: u8,
        op: Op,
        reply_lost: bool,
    },
    /// A message reaches a host or the client.
    Deliver {
        msg: Msg,
    },
    Drop(Msg),
    Finish {
        host: u8,
        job: u8,
        ok: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
    TakeSubmit(u8),
    Dispatch {
        job: u8,
        host: u8,
    },
    GiveUp(u8),
    /// Poll the host of a job in flight (feed / `GetHostJob`).
    Query(u8),
    /// The host of a job in flight is suspected down.
    Unreachable(u8),
    /// Send the pending change again.
    Retry,
    /// A host's record arrives (`keep`: it is duplicated).
    Rec {
        msg: Msg,
        keep: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JSt {
    Absent,
    Queued,
    InFlight { host: u8, attempt: u8 },
    Ended,
}

#[derive(Clone, Copy, Debug)]
struct JobView {
    st: JSt,
    /// The highest attempt dispatched (attempts are numbered per job).
    last: u8,
}

/// The coordinator's state of a job: a fold of the log with `store.rs`
/// `apply`'s conditional updates (an entry whose condition fails is a no-op).
fn job_view(log: &[Entry], j: u8) -> JobView {
    let mut v = JobView {
        st: JSt::Absent,
        last: 0,
    };
    for e in log.iter().filter(|e| e.kind.job() == j) {
        match e.kind {
            Kind::Submit { .. } if v.st == JSt::Absent => v.st = JSt::Queued,
            Kind::Dispatched { host, attempt, .. } => {
                v.last = v.last.max(attempt);
                if v.st == JSt::Queued {
                    v.st = JSt::InFlight { host, attempt };
                }
            }
            Kind::Requeue { host, attempt, .. } if v.st == JSt::InFlight { host, attempt } => {
                v.st = JSt::Queued
            }
            Kind::Complete { attempt, .. } if matches!(v.st, JSt::InFlight { attempt: a, .. } if a == attempt) => {
                v.st = JSt::Ended
            }
            Kind::GaveUp { .. } if v.st == JSt::Queued => v.st = JSt::Ended,
            _ => {}
        }
    }
    v
}

impl State {
    fn view(&self, c: u8) -> Option<(Lead, &[Entry])> {
        let l = self.coords[c as usize]?;
        Some((l, &self.relay.log[..l.seq as usize]))
    }

    /// The holder at the current epoch is alive (it would renew in time).
    fn holder_alive(&self) -> bool {
        self.relay
            .holder
            .is_some_and(|h| self.coords[h as usize].is_some_and(|l| l.epoch == self.relay.epoch))
    }

    fn terminals(&self, j: u8) -> impl Iterator<Item = &Entry> {
        self.relay
            .log
            .iter()
            .filter(move |e| e.kind.job() == j && e.kind.terminal())
    }
}

impl Kind {
    fn rename_hosts(self, hp: [u8; 2]) -> Kind {
        match self {
            Kind::Dispatched { job, host, attempt } => Kind::Dispatched {
                job,
                host: hp[host as usize],
                attempt,
            },
            Kind::Requeue { job, host, attempt } => Kind::Requeue {
                job,
                host: hp[host as usize],
                attempt,
            },
            Kind::Complete {
                job,
                attempt,
                value,
            } => Kind::Complete {
                job,
                attempt,
                value: Value {
                    host: hp[value.host as usize],
                    attempt: value.attempt,
                },
            },
            k => k,
        }
    }
}

impl Msg {
    fn rename_hosts(self, hp: [u8; 2]) -> Msg {
        let h = |x: u8| hp[x as usize];
        match self {
            Msg::Run { host, job, attempt } => Msg::Run {
                host: h(host),
                job,
                attempt,
            },
            Msg::Cancel { host, job, attempt } => Msg::Cancel {
                host: h(host),
                job,
                attempt,
            },
            Msg::Rec {
                host,
                job,
                attempt,
                st,
            } => Msg::Rec {
                host: h(host),
                job,
                attempt,
                st,
            },
            m => m,
        }
    }
}

impl State {
    /// The state with hosts renamed by `hp` and contenders by `cp`.
    fn renamed(&self, hp: [u8; 2], cp: [u8; 2]) -> State {
        let mut s = self.clone();
        s.relay.holder = self.relay.holder.map(|c| cp[c as usize]);
        s.relay.grants = self.relay.grants.iter().map(|&c| cp[c as usize]).collect();
        for e in &mut s.relay.log {
            e.author = cp[e.author as usize];
            e.kind = e.kind.rename_hosts(hp);
        }
        for (c, l) in self.coords.iter().enumerate() {
            s.coords[cp[c] as usize] = l.map(|mut l| {
                l.pending = l.pending.map(|k| k.rename_hosts(hp));
                l
            });
        }
        for (h, r) in self.hosts.iter().enumerate() {
            s.hosts[hp[h] as usize] = r.clone();
        }
        s.net = self.net.iter().map(|m| m.rename_hosts(hp)).collect();
        s.produced = self
            .produced
            .iter()
            .map(|&(j, v)| {
                (
                    j,
                    Value {
                        host: hp[v.host as usize],
                        attempt: v.attempt,
                    },
                )
            })
            .collect();
        s
    }

    /// Symmetry reduction: hosts are interchangeable and so are contenders,
    /// so the checker visits one state per class (the least renaming).
    fn representative(&self) -> State {
        let mut best = self.clone();
        for hp in [[0, 1], [1, 0]] {
            for cp in [[0, 1], [1, 0]] {
                if hp == [0, 1] && cp == [0, 1] {
                    continue;
                }
                let r = self.renamed(hp, cp);
                if r < best {
                    best = r;
                }
            }
        }
        best
    }
}

impl Pool {
    fn small() -> Self {
        Pool {
            jobs: 1,
            max_attempts: 2,
            max_epoch: 2,
            mutations: Mutations::default(),
            only: None,
        }
    }

    /// The coordinator sends what an acknowledged change implies.
    fn effects(&self, s: &mut State, kind: Kind) {
        match kind {
            Kind::Submit { job } => {
                s.relay.mailbox.remove(&job);
                s.net.insert(Msg::Ack { job });
            }
            Kind::Dispatched { job, host, attempt } => {
                s.net.insert(Msg::Run { host, job, attempt });
            }
            Kind::Requeue { job, host, attempt } => {
                s.net.insert(Msg::Cancel { host, job, attempt });
            }
            Kind::Complete { .. } | Kind::GaveUp { .. } => {}
        }
    }

    /// A coordinator decides a change and appends it (the relay's answer
    /// may be lost: the change stays pending and is retried).
    fn decide(&self, s: &mut State, c: u8, kind: Kind, reply_lost: bool) {
        if let Some(l) = s.coords[c as usize].as_mut() {
            l.pending = Some(kind);
        }
        if self.mutations.send_before_persist
            && let Kind::Dispatched { job, host, attempt } = kind
        {
            // Mutation (c): sent now, appended by a later `Retry`.
            s.net.insert(Msg::Run { host, job, attempt });
            return;
        }
        self.append(s, c, reply_lost);
    }

    fn append(&self, s: &mut State, c: u8, reply_lost: bool) -> Option<()> {
        let l = s.coords[c as usize]?;
        let kind = l.pending?;
        let r = &mut s.relay;
        enum Answer {
            Fenced,
            Conflict,
            Refused,
            Ok,
        }
        let answer = if !self.mutations.skip_epoch_check && l.epoch != r.epoch {
            Answer::Fenced
        } else if l.seq as usize != r.log.len() {
            match r.log.get(l.seq as usize) {
                // A retry of an entry already stored: answered with the head.
                Some(e) if e.epoch == l.epoch && e.kind == kind => Answer::Ok,
                _ => Answer::Conflict,
            }
        } else if kind.terminal()
            && !self.mutations.allow_second_result
            && r.log
                .iter()
                .any(|e| e.kind.terminal() && e.kind.job() == kind.job())
        {
            Answer::Refused
        } else {
            r.log.push(Entry {
                epoch: l.epoch,
                kind,
                author: c,
                relay_epoch: r.epoch,
            });
            Answer::Ok
        };
        if reply_lost {
            return Some(());
        }
        match answer {
            Answer::Fenced => {
                s.coords[c as usize] = None;
                s.seen.fenced = true;
            }
            Answer::Conflict => s.coords[c as usize] = None,
            Answer::Refused => {
                if let Some(l) = s.coords[c as usize].as_mut() {
                    l.pending = None;
                }
            }
            Answer::Ok => {
                if let Some(l) = s.coords[c as usize].as_mut() {
                    l.seq += 1;
                    l.pending = None;
                }
                self.effects(s, kind);
            }
        }
        Some(())
    }

    /// `on_record` / `reconcile`: a host's record reaches coordinator `c`.
    #[allow(clippy::too_many_arguments)]
    fn on_rec(&self, s: &mut State, c: u8, host: u8, job: u8, attempt: u8, st: HSt, lost: bool) {
        let Some((l, view)) = s.view(c) else {
            return; // not coordinating: the answer is dropped
        };
        if l.pending.is_some() {
            return;
        }
        let v = job_view(view, job);
        let value = Value { host, attempt };
        if self.mutations.allow_second_result && st == HSt::Done && v.st != JSt::Absent {
            self.decide(
                s,
                c,
                Kind::Complete {
                    job,
                    attempt,
                    value,
                },
                lost,
            );
            return;
        }
        match v.st {
            JSt::InFlight {
                host: h,
                attempt: a,
            } if h == host && a == attempt => match st {
                HSt::Done => self.decide(
                    s,
                    c,
                    Kind::Complete {
                        job,
                        attempt,
                        value,
                    },
                    lost,
                ),
                HSt::Error | HSt::Canceled => {
                    self.decide(s, c, Kind::Requeue { job, host, attempt }, lost)
                }
                HSt::Running | HSt::None => {}
            },
            // The host never got this attempt: fence it there and run it again.
            JSt::InFlight {
                host: h,
                attempt: a,
            } if h == host && attempt < a => self.decide(
                s,
                c,
                Kind::Requeue {
                    job,
                    host,
                    attempt: a,
                },
                lost,
            ),
            _ => match st {
                HSt::Running if !(self.mutations.fix_stale_cancel && attempt > v.last) => {
                    s.net.insert(Msg::Cancel { host, job, attempt });
                }
                HSt::Done => s.seen.suppressed = true,
                _ => {}
            },
        }
    }

    fn on_host(&self, s: &mut State, msg: Msg) {
        match msg {
            Msg::Run { host, job, attempt } => {
                let r = &mut s.hosts[host as usize][job as usize];
                // Lower: stale, refused. Equal: a duplicate delivery.
                if attempt > r.attempt {
                    // A newer attempt supersedes a lower one still running,
                    // whose result is then dropped (node.rs `execute`).
                    *r = HRec {
                        attempt,
                        st: HSt::Running,
                    };
                }
            }
            Msg::Cancel { host, job, attempt } => {
                let r = &mut s.hosts[host as usize][job as usize];
                if r.attempt < attempt {
                    // Unknown or only older attempts seen: a tombstone.
                    *r = HRec {
                        attempt,
                        st: HSt::Canceled,
                    };
                } else if r.attempt == attempt && r.st == HSt::Running {
                    r.st = HSt::Canceled;
                }
            }
            Msg::Ack { job } => s.acked[job as usize] = true,
            Msg::Rec { .. } => {}
        }
    }
}

impl Model for Pool {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> Vec<State> {
        let jobs = self.jobs as usize;
        vec![State {
            faults: true,
            relay: Relay {
                epoch: 0,
                holder: None,
                expired: false,
                grants: Vec::new(),
                log: Vec::new(),
                mailbox: BTreeSet::new(),
            },
            coords: vec![None; CONTENDERS as usize],
            hosts: vec![
                vec![
                    HRec {
                        attempt: 0,
                        st: HSt::None
                    };
                    jobs
                ];
                HOSTS as usize
            ],
            net: BTreeSet::new(),
            acked: vec![false; jobs],
            observed: vec![None; jobs],
            produced: BTreeSet::new(),
            regressed: false,
            seen: Seen::default(),
        }]
    }

    fn actions(&self, s: &State, out: &mut Vec<Action>) {
        let f = s.faults;
        if f {
            out.push(Action::StopFaults);
            out.push(Action::Expire);
        } else if !s.holder_alive() {
            out.push(Action::Expire);
        }
        for j in 0..self.jobs {
            out.push(Action::Submit(j));
            out.push(Action::Observe(j));
            if f {
                out.push(Action::LoseMailbox(j));
            }
        }
        for c in 0..CONTENDERS {
            out.push(Action::Acquire(c));
            out.push(Action::Renew(c));
            if f {
                out.push(Action::Crash(c));
            }
            let Some((l, view)) = s.view(c) else {
                continue;
            };
            let mut ops = Vec::new();
            if l.pending.is_some() {
                ops.push(Op::Retry);
            } else {
                for j in 0..self.jobs {
                    if s.relay.mailbox.contains(&j) {
                        ops.push(Op::TakeSubmit(j));
                    }
                    let v = job_view(view, j);
                    match v.st {
                        JSt::Queued if v.last < self.max_attempts => {
                            for host in 0..HOSTS {
                                ops.push(Op::Dispatch { job: j, host });
                            }
                        }
                        JSt::Queued => ops.push(Op::GiveUp(j)),
                        JSt::InFlight { .. } => {
                            ops.push(Op::Query(j));
                            if f {
                                ops.push(Op::Unreachable(j));
                            }
                        }
                        JSt::Absent | JSt::Ended => {}
                    }
                }
                for &msg in &s.net {
                    if matches!(msg, Msg::Rec { .. }) {
                        ops.push(Op::Rec { msg, keep: false });
                        if f {
                            ops.push(Op::Rec { msg, keep: true });
                        }
                    }
                }
            }
            for op in ops {
                out.push(Action::Coord {
                    c,
                    op,
                    reply_lost: false,
                });
                // Lost answers to the appends of submits, dispatches and
                // results (whose retries the relay must answer idempotently).
                if f && matches!(
                    op,
                    Op::TakeSubmit(_) | Op::Dispatch { .. } | Op::Rec { .. } | Op::Retry
                ) {
                    out.push(Action::Coord {
                        c,
                        op,
                        reply_lost: true,
                    });
                }
            }
        }
        for &msg in &s.net {
            if f {
                out.push(Action::Drop(msg));
            }
            if !matches!(msg, Msg::Rec { .. }) {
                out.push(Action::Deliver { msg });
            }
        }
        for host in 0..HOSTS {
            for job in 0..self.jobs {
                if s.hosts[host as usize][job as usize].st == HSt::Running {
                    out.push(Action::Finish {
                        host,
                        job,
                        ok: true,
                    });
                    out.push(Action::Finish {
                        host,
                        job,
                        ok: false,
                    });
                }
            }
        }
    }

    fn next_state(&self, prev: &State, action: Action) -> Option<State> {
        let mut s = prev.clone();
        match action {
            Action::StopFaults => s.faults = false,
            Action::Submit(j) => {
                if !s.acked[j as usize] {
                    s.relay.mailbox.insert(j);
                }
            }
            Action::Observe(j) => {
                // The client reads the job's result from the log.
                if let Some(o) = s.terminals(j).last().and_then(|e| e.kind.outcome()) {
                    s.observed[j as usize] = Some(o);
                }
            }
            Action::LoseMailbox(j) => {
                s.relay.mailbox.remove(&j);
            }
            Action::Expire => {
                // Fault phase: any time (clock skew, jumps, a paused holder),
                // but never the last epoch, which is kept for recovery.
                // Afterwards only a dead holder's lease runs out.
                if s.relay.holder.is_none()
                    || (s.faults && s.relay.epoch >= self.max_epoch)
                    || (!s.faults && s.holder_alive())
                {
                    return None;
                }
                s.relay.expired = true;
            }
            Action::Acquire(c) => {
                if s.relay.holder.is_some() && !s.relay.expired {
                    return None;
                }
                s.relay.epoch += 1;
                s.relay.holder = Some(c);
                s.relay.expired = false;
                s.relay.grants.push(c);
                s.coords[c as usize] = Some(Lead {
                    epoch: s.relay.epoch,
                    seq: s.relay.log.len() as u8,
                    pending: None,
                });
            }
            Action::Renew(c) => {
                let l = s.coords[c as usize]?;
                if l.epoch != s.relay.epoch {
                    s.coords[c as usize] = None;
                    s.seen.fenced = true;
                } else if s.faults && s.relay.expired && s.relay.holder == Some(c) {
                    s.relay.expired = false;
                }
            }
            Action::Crash(c) => {
                // The last epoch's holder stays up so that liveness after
                // the faults does not need an epoch beyond the bound.
                if s.relay.holder == Some(c) && s.relay.epoch >= self.max_epoch {
                    return None;
                }
                s.coords[c as usize] = None;
            }
            Action::Coord { c, op, reply_lost } => {
                let (l, view) = s.view(c)?;
                if l.pending.is_some() != (op == Op::Retry) {
                    return None;
                }
                match op {
                    Op::TakeSubmit(job) => {
                        if !s.relay.mailbox.contains(&job) {
                            return None;
                        }
                        if view.iter().any(|e| e.kind == Kind::Submit { job }) {
                            // A retry of a submit already in the log: answer it.
                            s.relay.mailbox.remove(&job);
                            s.net.insert(Msg::Ack { job });
                        } else {
                            self.decide(&mut s, c, Kind::Submit { job }, reply_lost);
                        }
                    }
                    Op::Dispatch { job, host } => {
                        let attempt = job_view(view, job).last + 1;
                        self.decide(
                            &mut s,
                            c,
                            Kind::Dispatched { job, host, attempt },
                            reply_lost,
                        );
                    }
                    Op::GiveUp(job) => self.decide(&mut s, c, Kind::GaveUp { job }, reply_lost),
                    Op::Query(job) => {
                        let JSt::InFlight { host, .. } = job_view(view, job).st else {
                            return None;
                        };
                        // One answer per host and job on its way (a feed is
                        // read in order; an older answer still in flight is
                        // what makes the coordinator's view of it stale).
                        if s.net.iter().any(
                            |m| matches!(*m, Msg::Rec { host: h, job: j, .. } if h == host && j == job),
                        ) {
                            return None;
                        }
                        let r = s.hosts[host as usize][job as usize];
                        s.net.insert(Msg::Rec {
                            host,
                            job,
                            attempt: r.attempt,
                            st: r.st,
                        });
                    }
                    Op::Unreachable(job) => {
                        let JSt::InFlight { host, attempt } = job_view(view, job).st else {
                            return None;
                        };
                        self.decide(&mut s, c, Kind::Requeue { job, host, attempt }, reply_lost);
                    }
                    Op::Retry => self.append(&mut s, c, reply_lost)?,
                    Op::Rec { msg, keep } => {
                        let Msg::Rec {
                            host,
                            job,
                            attempt,
                            st,
                        } = msg
                        else {
                            return None;
                        };
                        if !keep {
                            s.net.remove(&msg);
                        }
                        self.on_rec(&mut s, c, host, job, attempt, st, reply_lost);
                    }
                }
            }
            Action::Deliver { msg } => {
                s.net.remove(&msg);
                self.on_host(&mut s, msg);
            }
            Action::Drop(msg) => {
                s.net.remove(&msg);
            }
            Action::Finish { host, job, ok } => {
                let r = &mut s.hosts[host as usize][job as usize];
                if r.st != HSt::Running {
                    return None;
                }
                if ok {
                    r.st = HSt::Done;
                    let value = Value {
                        host,
                        attempt: r.attempt,
                    };
                    s.produced.insert((job, value));
                } else {
                    r.st = HSt::Error;
                }
            }
        }
        // The newest attempt a host saw of a job never goes down (node.rs:
        // a lower attempt is refused, a cancel fences upwards only).
        if s.hosts
            .iter()
            .flatten()
            .zip(prev.hosts.iter().flatten())
            .any(|(n, p)| n.attempt < p.attempt)
        {
            s.regressed = true;
        }
        // A step that changes nothing is not a step (it would hide terminal
        // states from the `eventually` check).
        (s != *prev).then_some(s)
    }

    fn within_boundary(&self, s: &State) -> bool {
        s.relay.epoch <= self.max_epoch
    }

    fn properties(&self) -> Vec<Property<Self>> {
        let all = vec![
            Property::always("single writer per epoch", |_, s: &State| {
                s.relay.log.iter().all(|e| {
                    e.epoch >= 1
                        && s.relay.grants.get(e.epoch as usize - 1) == Some(&e.author)
                        && e.relay_epoch == e.epoch
                })
            }),
            Property::always("no double results", |m: &Pool, s: &State| {
                (0..m.jobs).all(|j| s.terminals(j).count() <= 1)
            }),
            Property::always("results are real", |_, s: &State| {
                s.relay.log.iter().enumerate().all(|(i, e)| {
                    let Kind::Complete {
                        job,
                        attempt,
                        value,
                    } = e.kind
                    else {
                        return true;
                    };
                    // The attempt was the job's current one when it was
                    // recorded, and a host produced this value for it there.
                    let current = s.relay.log[..i].iter().rev().find(|p| {
                        p.kind.job() == job
                            && matches!(p.kind, Kind::Dispatched { .. } | Kind::Requeue { .. })
                    });
                    s.produced.contains(&(job, value))
                        && value.attempt == attempt
                        && current.is_some_and(|p| {
                            p.kind
                                == Kind::Dispatched {
                                    job,
                                    host: value.host,
                                    attempt,
                                }
                        })
                })
            }),
            Property::always("client sees only committed", |m: &Pool, s: &State| {
                (0..m.jobs).all(|j| match s.observed[j as usize] {
                    None => true,
                    Some(o) => s.terminals(j).next().and_then(|e| e.kind.outcome()) == Some(o),
                })
            }),
            Property::always("attempts only grow on hosts", |_, s: &State| !s.regressed),
            // Every attempt a host holds (runs, ran or was fenced with) is in
            // the log as a dispatch to that host; every attempt a host
            // accepts is its newest one for a while, so this covers them all.
            Property::always("every placement is recorded", |m: &Pool, s: &State| {
                (0..HOSTS).all(|host| {
                    (0..m.jobs).all(|job| {
                        let attempt = s.hosts[host as usize][job as usize].attempt;
                        attempt == 0
                            || s.relay
                                .log
                                .iter()
                                .any(|e| e.kind == Kind::Dispatched { job, host, attempt })
                    })
                })
            }),
            // Not claimed by the RFC (the finding below): a cancel only ever
            // ends an attempt that is no longer the job's current one.
            Property::always(FINDING, |m: &Pool, s: &State| {
                (0..HOSTS).all(|host| {
                    (0..m.jobs).all(|job| {
                        let r = s.hosts[host as usize][job as usize];
                        r.st != HSt::Canceled
                            || job_view(&s.relay.log, job).st
                                != (JSt::InFlight {
                                    host,
                                    attempt: r.attempt,
                                })
                    })
                })
            }),
            Property::always("one submit per key", |m: &Pool, s: &State| {
                (0..m.jobs).all(|j| {
                    s.relay
                        .log
                        .iter()
                        .filter(|e| e.kind == Kind::Submit { job: j })
                        .count()
                        <= 1
                })
            }),
            Property::eventually("no lost jobs", |m: &Pool, s: &State| {
                (0..m.jobs).all(|j| {
                    s.acked[j as usize]
                        && s.terminals(j).next().is_some()
                        && s.observed[j as usize].is_some()
                })
            }),
            Property::sometimes("all jobs done", |m: &Pool, s: &State| {
                (0..m.jobs).all(|j| matches!(s.observed[j as usize], Some(Outcome::Result(_))))
            }),
            Property::sometimes("takeover with a job in flight", |_, s: &State| {
                // A job dispatched in one epoch is completed in a later one.
                s.relay.log.iter().any(|c| {
                    let Kind::Complete { job, attempt, .. } = c.kind else {
                        return false;
                    };
                    s.relay.log.iter().any(|d| {
                        matches!(d.kind, Kind::Dispatched { job: dj, attempt: da, .. }
                            if dj == job && da == attempt)
                            && d.epoch < c.epoch
                    })
                })
            }),
            Property::sometimes("stale coordinator fenced", |_, s: &State| s.seen.fenced),
            Property::sometimes("duplicate execution suppressed", |m: &Pool, s: &State| {
                s.seen.suppressed && (0..m.jobs).all(|j| s.terminals(j).next().is_some())
            }),
        ];
        match self.only {
            Some(name) => all.into_iter().filter(|p| p.name == name).collect(),
            // The finding is checked on its own (see the tests).
            None => all.into_iter().filter(|p| p.name != FINDING).collect(),
        }
    }
}

fn run(model: Pool, threads: usize) -> impl Checker<Pool> {
    run_capped(model, threads, None)
}

fn run_capped(model: Pool, threads: usize, cap: Option<usize>) -> impl Checker<Pool> {
    let started = Instant::now();
    let mut builder = model
        .checker()
        .threads(threads)
        .symmetry_fn(State::representative);
    if let Some(n) = cap {
        builder = builder.target_state_count(n);
    }
    // A cap for exploring larger configurations without running out of memory.
    if let Some(n) = std::env::var("POOL_MODEL_MAX_STATES")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        builder = builder.target_state_count(n);
    }
    let checker = builder.spawn_dfs().join();
    eprintln!(
        "pool model: {} unique states ({} generated), max depth {}, {:.1?}",
        checker.unique_state_count(),
        checker.state_count(),
        checker.max_depth(),
        started.elapsed()
    );
    checker
}

fn trace(checker: &impl Checker<Pool>, name: &'static str) -> String {
    let path = checker.assert_any_discovery(name);
    path.into_actions()
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().min(4))
}

fn check_all(model: Pool) {
    let checker = run(model, threads());
    for name in [
        "takeover with a job in flight",
        "stale coordinator fenced",
        "duplicate execution suppressed",
        "all jobs done",
    ] {
        eprintln!("example of {name:?}: {}", trace(&checker, name));
    }
    checker.assert_properties();
}

/// The protocol as designed: every `always` and `eventually` property holds
/// over the whole bounded space (1 job, 2 attempts, 2 epochs: one takeover
/// with a stale old holder), and every `sometimes` one is reachable.
#[test]
fn pool_protocol_is_safe() {
    check_all(Pool::small());
}

/// Three epochs (a second takeover, a holder that crashes and comes back).
/// Run with `cargo test --release -p cua-spaces-cluster --test pool_model
/// -- --ignored`.
#[test]
#[ignore = "large: run in release"]
fn pool_protocol_is_safe_three_epochs() {
    check_all(Pool {
        max_epoch: 3,
        ..Pool::small()
    });
}

/// Two jobs and their interleavings: the space is too large to exhaust
/// (over 77M states), so this explores the first 300M generated states
/// depth-first and checks only that no `always` property is violated there
/// (a partial check; `sometimes`/`eventually` need the whole space).
#[test]
#[ignore = "large: run in release"]
fn pool_protocol_is_safe_two_jobs_partial() {
    let checker = run_capped(
        Pool {
            jobs: 2,
            ..Pool::small()
        },
        threads(),
        Some(300_000_000),
    );
    for name in [
        "single writer per epoch",
        "no double results",
        "results are real",
        "client sees only committed",
        "attempts only grow on hosts",
        "every placement is recorded",
        "one submit per key",
    ] {
        checker.assert_no_discovery(name);
    }
}

fn find(mutations: Mutations, only: &'static str) -> String {
    let checker = run(
        Pool {
            mutations,
            only: Some(only),
            ..Pool::small()
        },
        threads(),
    );
    let t = trace(&checker, only);
    eprintln!("counterexample to {only:?}: {t}");
    t
}

/// (a) Without the relay's epoch check a paused old holder appends after
/// the next epoch was granted.
#[test]
fn mutation_no_epoch_check_breaks_single_writer() {
    find(
        Mutations {
            skip_epoch_check: true,
            ..Mutations::default()
        },
        "single writer per epoch",
    );
}

/// (b) Without the relay's second-terminal refusal and the coordinator's
/// current-attempt check, a duplicate result is recorded too.
#[test]
fn mutation_no_terminal_guard_breaks_no_double_results() {
    find(
        Mutations {
            allow_second_result: true,
            ..Mutations::default()
        },
        "no double results",
    );
}

/// (c) Sending a dispatch before the relay stored it lets a host run an
/// attempt the log does not know of.
#[test]
fn mutation_send_before_persist_breaks_recorded_placements() {
    find(
        Mutations {
            send_before_persist: true,
            ..Mutations::default()
        },
        "every placement is recorded",
    );
}

/// Finding: a stale holder (its lease ran out, it has not tried to append
/// yet) reads a host's feed, sees the new holder's attempt running, takes
/// it for a superseded one (`on_record`: not current, still running) and
/// cancels it. Cancels are not fenced by the epoch, so the new holder's
/// current attempt dies and runs again. Completion stays exactly once (no
/// safety property breaks), but work is lost and, where a cancel counts as
/// an attempt, a job can end `error` without ever failing.
#[test]
fn finding_stale_holder_cancels_current_attempt() {
    find(Mutations::default(), FINDING);
}

/// The proposed fix (`fix_stale_cancel`) removes it: no cancel ever ends a
/// current attempt. Full space, so run in release with `--ignored`.
#[test]
#[ignore = "large: run in release"]
fn fix_stale_cancel_keeps_current_attempts() {
    let checker = run(
        Pool {
            mutations: Mutations {
                fix_stale_cancel: true,
                ..Mutations::default()
            },
            only: Some(FINDING),
            ..Pool::small()
        },
        threads(),
    );
    checker.assert_no_discovery(FINDING);
}
