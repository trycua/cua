// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The replication runtime of one machine: who it talks to, what it knows,
//! and the loop that keeps the volume converged. The pieces live in
//! sibling modules (`owner`, `apply`, `election`, `backend`); this file has
//! the state, the lifecycle, the links, and the tick.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, watch};

use super::detector::{Detector, Health, Timing};
use super::era::{EraRec, Eras, Pos};
use super::link::{Counters, Handler, Link, Stream};
use super::oplog::OpLog;
use super::outbox::Outbox;
use super::roles::{self, Facts, Layout, Member, Role};
use super::ship::Shipping;
use super::status::{PeerView, PendingView, ReplicaStatus};
use super::transport::Transport;
use super::wire::{self, Beat, Chan, Frame, Hello, Msg};
use super::words::Summary;
use super::{MachineId, backend::ReplicatedBackend};
use crate::backend::Backend as _;
use crate::fs::FsBackend;
use crate::{Error, Result, now_ms};

/// Timing and size knobs. Defaults are for real networks; [`Config::fast`]
/// scales time down for simulations.
#[derive(Clone, Debug)]
pub struct Config {
    pub timing: Timing,
    /// How long a write waits for a second copy before it is reported as
    /// held on one machine (the write itself never fails for this).
    pub ack_wait: Duration,
    /// A control request that gets no answer in this long fails.
    pub call_timeout: Duration,
    /// A bulk request (a block) that gets no answer in this long fails.
    pub data_timeout: Duration,
    /// A more reliable machine must have been up and caught up this long
    /// before the volume moves back to it.
    pub handback_after: Duration,
    /// Bodies up to this size travel inside the control message.
    pub inline_max: usize,
    /// A write by a non-owner above this size is spooled and pulled by the
    /// owner in blocks instead of pushed in one request.
    pub push_max: usize,
    /// Bytes per transfer block.
    pub block: u64,
    /// Blocks in flight per transfer.
    pub window: usize,
    /// Records per log message.
    pub batch: usize,
    /// Log records kept.
    pub log_keep: usize,
    pub dial_backoff_min: Duration,
    pub dial_backoff_max: Duration,
    /// A machine not heard from in this long is not counted for takeover
    /// quorums.
    pub active_window: Duration,
    /// After a takeover attempt, wait this long before trying again.
    pub takeover_cooldown: Duration,
    /// A machine that does not need to order writes takes over from an
    /// owner only after it has been down this long.
    pub idle_takeover: Duration,
    /// How often the heads are recorded (see `migrate`).
    pub snapshot_every: Duration,
    /// Added to this node's wall clock (simulating skew).
    pub wall_skew_ms: i64,
}

impl Config {
    pub fn standard() -> Config {
        Config {
            timing: Timing::standard(),
            ack_wait: Duration::from_secs(2),
            call_timeout: Duration::from_secs(30),
            data_timeout: Duration::from_secs(60),
            handback_after: Duration::from_secs(120),
            inline_max: 16 * 1024,
            push_max: 1 << 20,
            block: 1 << 20,
            window: 8,
            batch: 256,
            log_keep: 200_000,
            dial_backoff_min: Duration::from_millis(250),
            dial_backoff_max: Duration::from_secs(15),
            active_window: Duration::from_secs(14 * 24 * 3600),
            takeover_cooldown: Duration::from_secs(10),
            idle_takeover: Duration::from_secs(300),
            snapshot_every: Duration::from_secs(600),
            wall_skew_ms: 0,
        }
    }

    /// Time scaled down by about 40 (heartbeat 50 ms): for simulations.
    pub fn fast() -> Config {
        Config {
            timing: Timing {
                interval: Duration::from_millis(50),
                floor: Duration::from_millis(250),
                confirm: Duration::from_millis(400),
                recover: 3,
            },
            ack_wait: Duration::from_millis(400),
            call_timeout: Duration::from_secs(4),
            data_timeout: Duration::from_secs(6),
            handback_after: Duration::from_millis(1500),
            dial_backoff_min: Duration::from_millis(20),
            dial_backoff_max: Duration::from_millis(250),
            takeover_cooldown: Duration::from_millis(300),
            idle_takeover: Duration::from_millis(2500),
            snapshot_every: Duration::from_secs(5),
            ..Config::standard()
        }
    }
}

/// One machine of the group as the supervisor sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineView {
    pub id: MachineId,
    pub name: String,
    pub is_me: bool,
    /// Reachable now (this machine counts as up unless it is going to sleep).
    pub alive: bool,
    /// Confirmed down: silent through the whole confirmation window, said
    /// goodbye, or refused connections. Not merely slow.
    pub down: bool,
    /// Up without a break for at least the hand-back time.
    pub stable: bool,
    /// Holds a full copy of the volume.
    pub replica: bool,
    pub facts: Facts,
}

/// What a machine's power state does to the volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    /// The machine is about to sleep (lid closed, idle sleep): tell peers.
    WillSleep,
    /// It woke.
    DidWake,
    /// The network went away.
    NetworkLost,
    /// The network is back (or changed address): dial again now.
    NetworkUp,
}

pub(super) struct PeerState {
    pub det: Detector,
    pub facts: Facts,
    pub applied: Pos,
    pub tail: Pos,
    /// Owner's view: the replica has applied through this.
    pub acked: Pos,
    pub pending: u64,
    pub problem: Option<String>,
    /// When the detector last turned alive and stayed so.
    pub alive_since: Option<Instant>,
    pub last_health: Health,
    pub next_dial: Instant,
    pub backoff: Duration,
    pub dialing: bool,
    /// A promise given to a takeover candidate: (epoch, until).
    pub promised: Option<(u64, Instant)>,
    pub heard_ms: u64,
    /// Since when the detector says it is down.
    pub down_since: Option<Instant>,
    /// It said goodbye on purpose (sleep, shutdown): no flapping to fear.
    pub intentional: bool,
    /// Dials in a row the machine refused (or hung up on before hello).
    pub refusals: u32,
    /// When that run of refusals began.
    pub first_refusal: Option<Instant>,
    /// How long its acknowledgements take (a moving average, ms): a machine
    /// that takes longer than half the wait is not waited for.
    pub ack_rtt_ms: f64,
    /// Writes in a row that waited for its acknowledgement in vain: a
    /// machine that does not answer is not waited for until it does.
    pub missed_acks: u32,
}

pub(super) struct State {
    pub members: BTreeMap<MachineId, Member>,
    pub eras: Eras,
    pub log: OpLog,
    pub outbox: Outbox,
    pub peers: HashMap<MachineId, PeerState>,
    pub volume_bytes: u64,
    pub last_takeover_try: Option<Instant>,
    pub takeovers: u64,
    pub handbacks: u64,
    pub fenced_writes: u64,
    pub conflict_copies: u64,
    /// Our own recent writes by position, for the read-your-writes wait.
    pub healthy: bool,
    /// When a local operation last needed to order a write itself.
    pub authority_wanted: Option<Instant>,
    /// When each recent record was written here (to time acknowledgements).
    pub written_at: std::collections::BTreeMap<Pos, Instant>,
}

#[derive(Default)]
pub(super) struct PeerLinks {
    pub ctl: Option<Arc<Link>>,
    pub data: Option<Arc<Link>>,
}

pub(super) struct Inner {
    pub me: MachineId,
    pub name: String,
    pub cfg: Config,
    pub store: FsBackend,
    pub spool: FsBackend,
    pub dir: PathBuf,
    pub transport: Arc<dyn Transport>,
    pub counters: Arc<Counters>,
    pub st: tokio::sync::Mutex<State>,
    /// One applier of records, snapshots and rebases at a time.
    pub gate: tokio::sync::Mutex<()>,
    pub links: Mutex<HashMap<MachineId, PeerLinks>>,
    pub facts: Mutex<Facts>,
    pub wake: Notify,
    /// Wakes the tick loop early (a link dropped, authority is wanted, a
    /// machine said goodbye).
    pub tick_wake: Notify,
    pub acks: watch::Sender<u64>,
    pub applied_tx: watch::Sender<Pos>,
    pub closed: AtomicBool,
    pub frozen: watch::Sender<bool>,
    pub is_client: AtomicBool,
    pub started: AtomicBool,
    pub hashes: Mutex<HashMap<String, Arc<Vec<String>>>>,
    pub seq_ack_counter: AtomicU64,
    pub self_away: AtomicBool,
    pub last_tick: Mutex<Instant>,
    pub submitting: AtomicBool,
    pub shipping: Shipping,
    pub takeover_busy: AtomicBool,
    pub handoff_busy: AtomicBool,
}

/// One machine's replicated volume.
#[derive(Clone)]
pub struct ReplicaNode {
    pub(super) inner: Arc<Inner>,
}

impl std::fmt::Debug for ReplicaNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReplicaNode({})", self.inner.me)
    }
}

/// A dial that failed because the machine said no, not because nothing
/// answered.
fn is_refusal(e: &Error) -> bool {
    let m = e.to_string();
    m.contains("refused") || m.contains("did not say hello") || m.contains("Connection reset")
}

fn load_eras(dir: &Path) -> Option<Eras> {
    serde_json::from_slice(&std::fs::read(dir.join("era.json")).ok()?).ok()
}

pub(super) fn save_eras(dir: &Path, e: &Eras) -> Result<()> {
    let p = dir.join("era.json");
    cua_home::guard_write(&p)?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(e)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

impl ReplicaNode {
    /// Opens this machine's replica state under `state_dir/replica` around
    /// `store` (the volume's local copy). `members` is the machines list,
    /// this machine included.
    pub async fn open(
        cfg: Config,
        me: &str,
        name: &str,
        state_dir: &Path,
        store: FsBackend,
        transport: Arc<dyn Transport>,
        members: Vec<Member>,
    ) -> Result<ReplicaNode> {
        let dir = state_dir.join("replica");
        std::fs::create_dir_all(&dir)?;
        // A directory a newer Cua Spaces prepared is refused, not guessed at.
        super::migrate::check_format(&dir)?;
        let mut log = OpLog::open(&dir, cfg.log_keep)?;
        let eras = match load_eras(&dir) {
            Some(e) => e,
            None => {
                let e = Eras::genesis(me, now_ms());
                // The first time replication touches a Volume that has
                // files: keep a copy of its heads before anything moves.
                let has_files = !store.list("").await?.is_empty();
                if has_files {
                    super::migrate::backup_heads(&dir, store.root())?;
                }
                save_eras(&dir, &e)?;
                // A volume that already holds files starts its log after
                // them: a machine that joins gets a snapshot, not a replay
                // of history this log never recorded.
                if has_files {
                    log.reset_to(Pos { epoch: 1, seq: 0 })?;
                }
                e
            }
        };
        let outbox = Outbox::open(&dir)?;
        let spool = FsBackend::new(dir.join("spool"));
        let (acks, _) = watch::channel(0u64);
        let (applied_tx, _) = watch::channel(log.tail());
        let (frozen, _) = watch::channel(false);
        let mut m: BTreeMap<MachineId, Member> =
            members.into_iter().map(|x| (x.id.clone(), x)).collect();
        m.entry(me.to_string()).or_insert_with(|| Member {
            id: me.to_string(),
            name: name.to_string(),
            ..Default::default()
        });
        let inner = Arc::new(Inner {
            me: me.to_string(),
            name: name.to_string(),
            cfg,
            store,
            spool,
            dir,
            transport,
            counters: Arc::new(Counters::default()),
            st: tokio::sync::Mutex::new(State {
                members: m,
                eras,
                log,
                outbox,
                peers: HashMap::new(),
                volume_bytes: 0,
                last_takeover_try: None,
                takeovers: 0,
                handbacks: 0,
                fenced_writes: 0,
                conflict_copies: 0,
                healthy: true,
                authority_wanted: None,
                written_at: Default::default(),
            }),
            gate: tokio::sync::Mutex::new(()),
            links: Mutex::new(HashMap::new()),
            facts: Mutex::new(Facts::default()),
            wake: Notify::new(),
            tick_wake: Notify::new(),
            acks,
            applied_tx,
            closed: AtomicBool::new(false),
            frozen,
            is_client: AtomicBool::new(false),
            started: AtomicBool::new(false),
            hashes: Mutex::new(HashMap::new()),
            seq_ack_counter: AtomicU64::new(0),
            self_away: AtomicBool::new(false),
            last_tick: Mutex::new(Instant::now()),
            submitting: AtomicBool::new(false),
            shipping: Mutex::new(HashMap::new()),
            takeover_busy: AtomicBool::new(false),
            handoff_busy: AtomicBool::new(false),
        });
        inner.recover_intent().await?;
        inner.adopt_unlogged().await?;
        {
            let mut st = inner.st.lock().await;
            inner.refresh_roles(&mut st);
        }
        Ok(ReplicaNode { inner })
    }

    /// The `Backend` the drive uses.
    pub fn backend(&self) -> Arc<ReplicatedBackend> {
        Arc::new(ReplicatedBackend::new(self.inner.clone()))
    }

    /// Starts the tick loop and the shippers.
    pub fn start(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let i = self.inner.clone();
        tokio::spawn(async move { i.tick_loop().await });
        let i = self.inner.clone();
        tokio::spawn(async move { i.ship_all_loop().await });
    }

    /// Says goodbye to every peer and stops.
    pub async fn shutdown(&self) {
        let first = !self.inner.closed.load(Ordering::SeqCst);
        self.inner.say_bye_and_flush("shutdown").await;
        // Only the first stop records the heads: a second one (after something
        // else wrote to the store) would hide those writes.
        if first {
            self.inner.save_snapshot().await;
        }
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.wake.notify_waiters();
        let links: Vec<Arc<Link>> = {
            let mut l = self.inner.links.lock().expect("links");
            let v = l
                .values()
                .flat_map(|p| [p.ctl.clone(), p.data.clone()])
                .flatten()
                .collect();
            l.clear();
            v
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        for l in links {
            l.close();
        }
    }

    /// A stream arrived for this machine.
    pub async fn accept(&self, stream: Stream) {
        let i = self.inner.clone();
        tokio::spawn(async move {
            let _ = i.accept_stream(stream).await;
        });
    }

    /// The machines list changed.
    pub async fn set_members(&self, members: Vec<Member>) {
        let mut st = self.inner.st.lock().await;
        for m in members {
            let cur = st.members.entry(m.id.clone()).or_default();
            cur.id = m.id.clone();
            cur.name = m.name;
            if m.facts != Facts::default() {
                cur.facts = m.facts;
            }
        }
        self.inner.refresh_roles(&mut st);
    }

    /// This machine's reliability facts changed.
    pub fn set_facts(&self, f: Facts) {
        *self.inner.facts.lock().expect("facts") = f;
    }

    /// A power or network event from the operating system.
    pub async fn notify_power(&self, ev: PowerEvent) {
        match ev {
            PowerEvent::WillSleep => {
                self.inner.say_bye_and_flush("sleep").await;
                self.inner.self_away.store(true, Ordering::SeqCst);
            }
            PowerEvent::NetworkLost => {
                self.inner.say_bye_and_flush("network_lost").await;
                self.inner.self_away.store(true, Ordering::SeqCst);
                self.inner.drop_links();
            }
            PowerEvent::DidWake | PowerEvent::NetworkUp => {
                self.inner.self_away.store(false, Ordering::SeqCst);
                let now = Instant::now();
                let mut st = self.inner.st.lock().await;
                for p in st.peers.values_mut() {
                    p.det.resume(now);
                    p.next_dial = now;
                    p.backoff = self.inner.cfg.dial_backoff_min;
                }
                drop(st);
                self.inner.drop_links();
                *self.inner.last_tick.lock().expect("tick") = now;
            }
        }
    }

    /// Freezes this node (a stopped process: nothing is read, written or
    /// timed until it thaws). Simulations only; the real test uses SIGSTOP.
    pub fn freeze(&self, on: bool) {
        self.inner.frozen.send_replace(on);
        if !on {
            self.inner.wake.notify_waiters();
        }
    }

    pub fn machine(&self) -> &str {
        &self.inner.me
    }

    /// This machine's name in the machines list.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Every machine of the group with what is known about it now.
    pub async fn machines(&self) -> Vec<MachineView> {
        let st = self.inner.st.lock().await;
        let layout = self.inner.layout_locked(&st);
        let handback = self.inner.cfg.handback_after;
        let mut out = vec![];
        for (id, m) in &st.members {
            let facts = self.inner.member_facts(&st, id);
            if *id == self.inner.me {
                out.push(MachineView {
                    id: id.clone(),
                    name: self.inner.name.clone(),
                    is_me: true,
                    alive: !self.inner.self_away.load(Ordering::SeqCst),
                    down: false,
                    stable: true,
                    replica: layout.role_of(id) == Role::Replica,
                    facts,
                });
                continue;
            }
            let p = st.peers.get(id);
            let alive = p.is_some_and(|p| p.last_health == Health::Alive)
                && self.inner.ctl_link(id).is_some();
            let stable =
                alive && p.is_some_and(|p| p.alive_since.is_some_and(|t| t.elapsed() >= handback));
            out.push(MachineView {
                id: id.clone(),
                name: if m.name.is_empty() {
                    id.clone()
                } else {
                    m.name.clone()
                },
                is_me: false,
                alive,
                down: p.is_none_or(|p| {
                    p.last_health.is_down() && p.det.down_and_silent(Instant::now())
                }),
                stable,
                replica: layout.role_of(id) == Role::Replica,
                facts,
            });
        }
        out
    }

    /// Whether this machine owns the volume now.
    pub async fn is_owner(&self) -> bool {
        self.inner.st.lock().await.eras.owner() == self.inner.me
    }

    /// Bytes the links carried (sent, received).
    pub fn wire_bytes(&self) -> (u64, u64) {
        (
            self.inner.counters.sent.load(Ordering::Relaxed),
            self.inner.counters.received.load(Ordering::Relaxed),
        )
    }

    /// What this machine's log and outbox know about `key` (for a failure's
    /// report).
    pub async fn debug_key(&self, key: &str) -> String {
        let st = self.inner.st.lock().await;
        let mut out = String::new();
        for r in st.log.range(Pos::ZERO, st.log.tail()) {
            if r.op.key() == key {
                let what = match &r.op {
                    super::oplog::Op::Put { meta, .. } => format!("put {}", meta.version),
                    super::oplog::Op::Delete { version, .. } => format!("delete {version}"),
                };
                out.push_str(&format!(" log@{}:{what}", r.pos));
            }
        }
        for p in st.outbox.items().iter().filter(|p| p.key == key) {
            out.push_str(&format!(
                " pending({})",
                p.meta
                    .as_ref()
                    .map_or("delete".into(), |m| m.version.clone())
            ));
        }
        out
    }

    pub async fn status(&self) -> ReplicaStatus {
        self.inner.status().await
    }

    /// The facts for the one plain paragraph (see `words`).
    pub async fn summary(&self, run_on: Option<&str>) -> Summary {
        self.inner.summary(run_on).await
    }
}

impl Inner {
    pub(super) fn wall_ms(&self) -> u64 {
        (now_ms() as i64 + self.cfg.wall_skew_ms).max(0) as u64
    }

    pub(super) fn my_facts(&self) -> Facts {
        let mut f = self.facts.lock().expect("facts").clone();
        if f.free_disk == 0 {
            f.free_disk = self.store.free_bytes().unwrap_or(0);
        }
        f
    }

    pub(super) async fn wait_unfrozen(&self) {
        let mut rx = self.frozen.subscribe();
        while *rx.borrow() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    pub(super) fn handler(self: &Arc<Self>) -> Handler {
        let me = self.clone();
        Arc::new(move |link, frame, data| {
            let me = me.clone();
            Box::pin(async move {
                me.wait_unfrozen().await;
                me.handle(link, frame, data).await;
            })
        })
    }

    /// Recomputes who should hold copies and whether this machine is a
    /// client (state lock held).
    pub(super) fn refresh_roles(&self, st: &mut State) {
        let l = self.layout_locked(st);
        self.is_client
            .store(l.role_of(&self.me) == Role::Client, Ordering::SeqCst);
    }

    pub(super) fn member_facts(&self, st: &State, id: &str) -> Facts {
        if id == self.me {
            return self.my_facts();
        }
        st.peers
            .get(id)
            .map(|p| p.facts.clone())
            .filter(|f| *f != Facts::default())
            .or_else(|| st.members.get(id).map(|m| m.facts.clone()))
            .unwrap_or_default()
    }

    /// Members that count (heard from recently, or never heard but listed).
    pub(super) fn active_members(&self, st: &State) -> Vec<MachineId> {
        let now = self.wall_ms();
        let win = self.cfg.active_window.as_millis() as u64;
        st.members
            .values()
            .filter(|m| {
                m.id == self.me
                    || st
                        .peers
                        .get(&m.id)
                        .map_or(m.last_seen_ms, |p| p.heard_ms.max(m.last_seen_ms))
                        == 0
                    || now.saturating_sub(
                        st.peers
                            .get(&m.id)
                            .map_or(m.last_seen_ms, |p| p.heard_ms.max(m.last_seen_ms)),
                    ) < win
            })
            .map(|m| m.id.clone())
            .collect()
    }

    pub(super) fn layout_locked(&self, st: &State) -> Layout {
        let active = self.active_members(st);
        let members: Vec<Member> = active
            .iter()
            .filter_map(|id| st.members.get(id))
            .map(|m| Member {
                facts: self.member_facts(st, &m.id),
                ..m.clone()
            })
            .collect();
        roles::layout(
            &members,
            roles::replica_count(members.len()),
            st.volume_bytes,
        )
    }

    fn beat(&self, st: &State) -> Beat {
        let eras: Vec<EraRec> = st.eras.recs.iter().rev().take(4).rev().cloned().collect();
        Beat {
            epoch: st.eras.epoch(),
            owner: st.eras.owner().to_string(),
            lineage: st.eras.lineage.clone(),
            eras,
            problem: if st.healthy {
                None
            } else {
                Some("this machine cannot store more of the volume".into())
            },
            applied: st.log.tail(),
            tail: st.log.tail(),
            facts: self.my_facts(),
            pending: st.outbox.len() as u64,
        }
    }

    fn hello(&self, st: &State, chan: Chan) -> Hello {
        Hello {
            machine: self.me.clone(),
            name: self.name.clone(),
            chan,
            lineage: st.eras.lineage.clone(),
            eras: st.eras.recs.iter().rev().take(8).rev().cloned().collect(),
            applied: st.log.tail(),
            tail: st.log.tail(),
            facts: self.my_facts(),
            v: wire::VERSION,
        }
    }

    pub(super) fn peer_state<'a>(&self, st: &'a mut State, id: &str) -> &'a mut PeerState {
        let timing = self.cfg.timing;
        let min = self.cfg.dial_backoff_min;
        st.peers.entry(id.to_string()).or_insert_with(|| PeerState {
            det: Detector::new(timing, Instant::now()),
            facts: Facts::default(),
            applied: Pos::ZERO,
            tail: Pos::ZERO,
            acked: Pos::ZERO,
            pending: 0,
            problem: None,
            alive_since: None,
            last_health: Health::Alive,
            next_dial: Instant::now(),
            backoff: min,
            dialing: false,
            promised: None,
            heard_ms: 0,
            down_since: None,
            intentional: false,
            missed_acks: 0,
            refusals: 0,
            first_refusal: None,
            ack_rtt_ms: 0.0,
        })
    }

    pub(super) fn say_bye(&self, reason: &str) -> Vec<Arc<Link>> {
        let links = self.links.lock().expect("links");
        let mut sent = vec![];
        for p in links.values() {
            if let Some(l) = &p.ctl {
                l.send(
                    Msg::Bye {
                        reason: reason.into(),
                    },
                    &[],
                );
                sent.push(l.clone());
            }
        }
        sent
    }

    /// Says goodbye and waits (a moment) for it to leave: a machine about
    /// to sleep has only a moment.
    pub(super) async fn say_bye_and_flush(&self, reason: &str) {
        for l in self.say_bye(reason) {
            l.flush(Duration::from_millis(500)).await;
        }
    }

    pub(super) fn drop_links(&self) {
        let links: Vec<Arc<Link>> = {
            let mut l = self.links.lock().expect("links");
            let v = l
                .values()
                .flat_map(|p| [p.ctl.clone(), p.data.clone()])
                .flatten()
                .collect();
            l.clear();
            v
        };
        for l in links {
            l.close();
        }
    }

    pub(super) fn ctl_link(&self, peer: &str) -> Option<Arc<Link>> {
        self.links
            .lock()
            .expect("links")
            .get(peer)
            .and_then(|p| p.ctl.clone())
            .filter(|l| !l.is_closed())
    }

    pub(super) fn data_link_now(&self, peer: &str) -> Option<Arc<Link>> {
        self.links
            .lock()
            .expect("links")
            .get(peer)
            .and_then(|p| p.data.clone())
            .filter(|l| !l.is_closed())
    }

    /// The data link to `peer`, dialing it if this machine has none.
    pub(super) async fn data_link(self: &Arc<Self>, peer: &str) -> Option<Arc<Link>> {
        if let Some(l) = self.data_link_now(peer) {
            return Some(l);
        }
        self.dial_chan(peer, Chan::Data).await.ok()
    }

    fn register(&self, link: &Arc<Link>) {
        let mut links = self.links.lock().expect("links");
        let e = links.entry(link.peer.clone()).or_default();
        match link.chan {
            Chan::Control => e.ctl = Some(link.clone()),
            Chan::Data => e.data = Some(link.clone()),
        }
    }

    fn unregister(&self, link: &Arc<Link>) {
        let mut links = self.links.lock().expect("links");
        if let Some(e) = links.get_mut(&link.peer) {
            let slot = match link.chan {
                Chan::Control => &mut e.ctl,
                Chan::Data => &mut e.data,
            };
            if slot.as_ref().is_some_and(|l| Arc::ptr_eq(l, link)) {
                *slot = None;
            }
        }
        drop(links);
        // A link that dropped is redialed now, not at the next tick.
        self.tick_wake.notify_one();
    }

    async fn handshake(
        self: &Arc<Self>,
        mut stream: Stream,
        chan: Chan,
        initiated: bool,
        expect: Option<&str>,
    ) -> Result<Arc<Link>> {
        use tokio::io::AsyncWriteExt as _;
        let hello = {
            let st = self.st.lock().await;
            self.hello(&st, chan)
        };
        let frame = Frame {
            id: 0,
            re: None,
            msg: Msg::Hello(hello),
        };
        let bytes = wire::encode(&frame, &[])?;
        let to = self.cfg.call_timeout;
        let exchange = async {
            stream.write_all(&bytes).await?;
            stream.flush().await?;
            wire::read(&mut stream).await
        };
        let got = tokio::time::timeout(to, exchange)
            .await
            .map_err(|_| Error::Backend("handshake timed out".into()))??;
        let Some((
            Frame {
                msg: Msg::Hello(h), ..
            },
            _,
        )) = got
        else {
            return Err(Error::Backend("the machine did not say hello".into()));
        };
        if let Some(e) = expect
            && e != h.machine
        {
            return Err(Error::Backend(format!(
                "dialed {e} and {} answered",
                h.machine
            )));
        }
        {
            let st = self.st.lock().await;
            if !st.members.contains_key(&h.machine) {
                return Err(Error::Forbidden(format!(
                    "{} is not one of this account's machines",
                    h.machine
                )));
            }
        }
        let me = self.clone();
        let me2 = self.clone();
        let link = Link::spawn(
            stream,
            h.machine.clone(),
            h.name.clone(),
            chan,
            initiated,
            self.counters.clone(),
            self.handler(),
            move |l| {
                me2.unregister(&l);
            },
        );
        *link.hello.lock().expect("hello") = Some(h.clone());
        me.register(&link);
        if chan == Chan::Control {
            me.peer_up(&link, h).await;
        }
        Ok(link)
    }

    async fn accept_stream(self: &Arc<Self>, stream: Stream) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        // The chan is in the peer's hello; read it from a peek of our own
        // handshake: both sides send first, so the chan we answer with is
        // the one the dialer chose. Read first, then answer.
        use tokio::io::AsyncWriteExt as _;
        let mut stream = stream;
        let first = tokio::time::timeout(self.cfg.call_timeout, wire::read(&mut stream))
            .await
            .map_err(|_| Error::Backend("handshake timed out".into()))??;
        let Some((
            Frame {
                msg: Msg::Hello(h), ..
            },
            _,
        )) = first
        else {
            return Err(Error::Backend("no hello".into()));
        };
        {
            let st = self.st.lock().await;
            if !st.members.contains_key(&h.machine) {
                return Err(Error::Forbidden(format!(
                    "{} is not one of this account's machines",
                    h.machine
                )));
            }
        }
        let mine = {
            let st = self.st.lock().await;
            self.hello(&st, h.chan)
        };
        let bytes = wire::encode(
            &Frame {
                id: 0,
                re: None,
                msg: Msg::Hello(mine),
            },
            &[],
        )?;
        stream.write_all(&bytes).await?;
        stream.flush().await?;
        let me2 = self.clone();
        let link = Link::spawn(
            stream,
            h.machine.clone(),
            h.name.clone(),
            h.chan,
            false,
            self.counters.clone(),
            self.handler(),
            move |l| me2.unregister(&l),
        );
        *link.hello.lock().expect("hello") = Some(h.clone());
        self.register(&link);
        if h.chan == Chan::Control {
            self.peer_up(&link, h).await;
        }
        Ok(())
    }

    pub(super) async fn dial_chan(self: &Arc<Self>, peer: &str, chan: Chan) -> Result<Arc<Link>> {
        let stream = self.transport.dial(peer).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                Error::Backend(format!("connection refused: {e}"))
            } else {
                Error::from(e)
            }
        })?;
        self.handshake(stream, chan, true, Some(peer)).await
    }

    /// A control link came up (either direction).
    async fn peer_up(self: &Arc<Self>, _link: &Arc<Link>, h: Hello) {
        let now = Instant::now();
        let mut reconcile = false;
        let mut join = false;
        {
            let mut st = self.st.lock().await;
            if let Some(m) = st.members.get_mut(&h.machine) {
                m.name = h.name.clone();
                m.last_seen_ms = self.wall_ms();
            }
            if h.lineage != st.eras.lineage {
                // Two volumes that started apart: the less reliable machine
                // joins the other's.
                let mine = self.my_facts();
                let theirs = h.facts.clone();
                join = (mine.rank, &self.me) < (theirs.rank, &h.machine);
                // Equal rank: the lower id joins (a deterministic pick).
                let _ = mine;
            } else {
                let advanced = st.eras.adopt_from(&h.eras);
                if advanced {
                    let _ = save_eras(&self.dir, &st.eras);
                    reconcile = true;
                }
            }
            let owner_now = st.eras.owner() == self.me;
            let other_lineage = h.lineage != st.eras.lineage;
            let divergence = st.eras.divergence(h.applied);
            let p = self.peer_state(&mut st, &h.machine);
            p.det.arrival(now);
            p.facts = h.facts.clone();
            p.heard_ms = self.wall_ms();
            p.applied = h.applied;
            p.tail = h.tail;
            // What a machine of another volume has applied says nothing
            // about this one.
            p.acked = if other_lineage {
                Pos::ZERO
            } else {
                divergence.unwrap_or(h.applied)
            };
            p.missed_acks = 0;
            p.backoff = self.cfg.dial_backoff_min;
            p.dialing = false;
            let _ = owner_now;
            if reconcile || st.eras.divergence(st.log.tail()).is_some() {
                reconcile = true;
            }
            self.refresh_roles(&mut st);
        }
        if join {
            let me = self.clone();
            let h2 = h.clone();
            tokio::spawn(async move { me.join_lineage(h2).await });
        } else if reconcile {
            let me = self.clone();
            tokio::spawn(async move { me.reconcile_eras().await });
        }
        self.wake.notify_one();
    }

    /// Frame dispatch.
    async fn handle(self: &Arc<Self>, link: Arc<Link>, frame: Frame, data: Vec<u8>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let id = frame.id;
        match frame.msg {
            Msg::Ping(b) => {
                self.on_beat(&link.peer, b).await;
                let mine = {
                    let st = self.st.lock().await;
                    self.beat(&st)
                };
                link.reply(id, Msg::Pong(mine), &[]);
            }
            Msg::Bye { reason } => self.on_bye(&link.peer, &reason).await,
            Msg::Append { epoch, records } => {
                let reply = self.handle_append(&link, epoch, records, data).await;
                link.reply(id, reply, &[]);
            }
            Msg::Snapshot { at } => {
                let reply = self.handle_snapshot(&link, at).await;
                link.reply(id, reply, &[]);
            }
            Msg::Prepare { epoch, base } => {
                let reply = self.handle_prepare(&link.peer, epoch, base).await;
                link.reply(id, reply, &[]);
            }
            Msg::Era { recs } => {
                self.handle_era(&link.peer, recs).await;
                link.reply(id, Msg::Ack { applied: Pos::ZERO }, &[]);
            }
            Msg::Handoff { epoch, to, base } => {
                let reply = self.handle_handoff(&link.peer, epoch, to, base).await;
                link.reply(id, reply, &[]);
            }
            Msg::Submit { ops } => {
                let reply = self.handle_submit(&link, ops, data).await;
                link.reply(id, reply, &[]);
            }
            Msg::Req(r) => {
                let (rep, out) = self.handle_req(&link, r, data).await;
                link.reply(id, Msg::Rep(rep), &out);
            }
            Msg::Hashes {
                src,
                block,
                from,
                count,
            } => {
                let reply = self.handle_hashes(src, block, from, count).await;
                link.reply(id, reply, &[]);
            }
            Msg::Block { src, block, idx } => match self.handle_block(src, block, idx).await {
                Ok(bytes) => {
                    link.reply(id, Msg::BlockIs, &bytes);
                }
                Err(e) => {
                    link.reply(
                        id,
                        Msg::Error {
                            kind: e.tag().into(),
                            message: e.to_string(),
                        },
                        &[],
                    );
                }
            },
            // Replies and handshakes are never dispatched.
            _ => {}
        }
    }

    pub(super) async fn on_beat(self: &Arc<Self>, peer: &str, b: Beat) {
        let now = Instant::now();
        let mut reconcile = false;
        let mut join: Option<Hello> = None;
        {
            let mut st = self.st.lock().await;
            if !b.lineage.is_empty() && b.lineage != st.eras.lineage {
                // The peer belongs to another volume now (it joined one while
                // we were connected): the less reliable of the two joins.
                let mine = self.my_facts();
                if (mine.rank, &self.me) < (b.facts.rank, &peer.to_string()) {
                    join = Some(Hello {
                        machine: peer.to_string(),
                        name: st
                            .members
                            .get(peer)
                            .map(|m| m.name.clone())
                            .unwrap_or_default(),
                        chan: Chan::Control,
                        lineage: b.lineage.clone(),
                        eras: b.eras.clone(),
                        applied: b.applied,
                        tail: b.tail,
                        facts: b.facts.clone(),
                        v: wire::VERSION,
                    });
                }
            } else if b.epoch > st.eras.epoch() && st.eras.adopt_from(&b.eras) {
                let _ = save_eras(&self.dir, &st.eras);
                reconcile = true;
            }
            if let Some(m) = st.members.get_mut(peer) {
                m.last_seen_ms = self.wall_ms();
            }
            let wall = self.wall_ms();
            let p = self.peer_state(&mut st, peer);
            p.det.arrival(now);
            p.facts = b.facts;
            p.applied = b.applied;
            p.tail = b.tail;
            p.pending = b.pending;
            p.problem = b.problem;
            p.heard_ms = wall;
            // The beat is the machine's own account of how far it got
            // (it moves back when the machine puts itself right after a
            // takeover, or starts over after joining another volume).
            if b.applied > p.acked {
                p.missed_acks = 0;
            }
            p.acked = b.applied;
        }
        if let Some(h) = join {
            let me = self.clone();
            tokio::spawn(async move { me.join_lineage(h).await });
        } else if reconcile {
            let me = self.clone();
            tokio::spawn(async move { me.reconcile_eras().await });
        }
        self.acks
            .send_replace(self.seq_ack_counter.fetch_add(1, Ordering::Relaxed));
    }

    pub(super) async fn on_bye(&self, peer: &str, reason: &str) {
        let now = Instant::now();
        let mut st = self.st.lock().await;
        let p = self.peer_state(&mut st, peer);
        p.det.bye(now);
        p.intentional = matches!(reason, "sleep" | "shutdown");
        tracing::info!(peer, reason, "replica: a machine said goodbye");
        drop(st);
        self.tick_wake.notify_one();
        self.wake.notify_one();
    }
}

impl Inner {
    /// The tick: heartbeats, health, dialing, takeover, hand-back, outbox.
    pub(super) async fn tick_loop(self: Arc<Self>) {
        let period = (self.cfg.timing.interval / 2).max(Duration::from_millis(10));
        let mut last_ping = Instant::now() - self.cfg.timing.interval;
        let mut last_size = Instant::now() - Duration::from_secs(3600);
        let mut last_snap = Instant::now();
        let mut snapped = Pos::ZERO;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(period) => {}
                _ = self.tick_wake.notified() => {}
            }
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            self.wait_unfrozen().await;
            let now = Instant::now();
            // Were we paused (lid closed, process stopped)? Then every
            // peer looks silent through no fault of theirs.
            let gap = {
                let mut lt = self.last_tick.lock().expect("tick");
                let g = now.saturating_duration_since(*lt);
                *lt = now;
                g
            };
            if gap > (self.cfg.timing.interval * 4).max(period * 6) {
                let mut st = self.st.lock().await;
                for p in st.peers.values_mut() {
                    p.det.resume(now);
                }
                tracing::info!(?gap, "replica: this machine was paused; forgiving silence");
            }
            if self.self_away.load(Ordering::SeqCst) {
                continue;
            }
            if now.saturating_duration_since(last_ping) >= self.cfg.timing.interval {
                last_ping = now;
                self.ping_all().await;
            }
            if now.saturating_duration_since(last_size) >= Duration::from_secs(60) {
                last_size = now;
                self.measure_volume().await;
            }
            // A hard stop leaves the last snapshot, so keep it fresh.
            if now.saturating_duration_since(last_snap) >= self.cfg.snapshot_every {
                last_snap = now;
                let tail = self.st.lock().await.log.tail();
                if tail != snapped {
                    snapped = tail;
                    self.save_snapshot().await;
                }
            }
            self.poll_health(now).await;
            self.dial_missing().await;
            // None of these may hold the tick: each can wait on the network
            // (a vote, an offer, the outbox), and a tick that waits is a
            // machine that stops noticing its peers.
            if !self.takeover_busy.swap(true, Ordering::SeqCst) {
                let me = self.clone();
                tokio::spawn(async move {
                    me.maybe_takeover().await;
                    me.takeover_busy.store(false, Ordering::SeqCst);
                });
            }
            if !self.handoff_busy.swap(true, Ordering::SeqCst) {
                let me = self.clone();
                tokio::spawn(async move {
                    me.maybe_handoff().await;
                    me.handoff_busy.store(false, Ordering::SeqCst);
                });
            }
            let me = self.clone();
            tokio::spawn(async move { me.submit_outbox().await });
        }
    }

    async fn measure_volume(&self) {
        if let Ok(all) = crate::backend::Backend::list(&self.store, "").await {
            let total: u64 = all.iter().map(|m| m.size).sum();
            let mut st = self.st.lock().await;
            st.volume_bytes = total;
            self.refresh_roles(&mut st);
        }
    }

    async fn ping_all(self: &Arc<Self>) {
        let peers: Vec<(MachineId, Arc<Link>)> = {
            let l = self.links.lock().expect("links");
            l.iter()
                .filter_map(|(k, v)| {
                    v.ctl
                        .clone()
                        .filter(|c| !c.is_closed())
                        .map(|c| (k.clone(), c))
                })
                .collect()
        };
        for (peer, link) in peers {
            let me = self.clone();
            tokio::spawn(async move {
                let beat = {
                    let st = me.st.lock().await;
                    me.beat(&st)
                };
                let to = me.cfg.timing.floor.max(me.cfg.timing.interval * 3);
                if let Ok((Msg::Pong(b), _)) = link.call(Msg::Ping(beat), &[], to).await {
                    me.on_beat(&peer, b).await;
                }
            });
        }
    }

    async fn poll_health(&self, now: Instant) {
        let mut st = self.st.lock().await;
        let ids: Vec<MachineId> = st.peers.keys().cloned().collect();
        let mut changed = false;
        for id in ids {
            let h = {
                let p = st.peers.get_mut(&id).expect("peer");
                let h = p.det.poll(now);
                if h != p.last_health {
                    tracing::info!(me = %self.me, peer = %id, from = p.last_health.word(), to = h.word(), "replica: peer health");
                    if h.is_down() && p.down_since.is_none() {
                        p.down_since = Some(now);
                    }
                    if h == Health::Alive {
                        p.down_since = None;
                        p.intentional = false;
                    }
                    if h == Health::Alive && p.last_health.is_down() {
                        p.alive_since = Some(now);
                    }
                    if h != Health::Alive {
                        p.alive_since = None;
                    }
                    p.last_health = h;
                    changed = true;
                }
                if h == Health::Alive && p.alive_since.is_none() {
                    p.alive_since = Some(now);
                }
                h
            };
            // A silent link may be half open: drop it so a new one is dialed.
            // A link that has only just come up has not had time to be
            // silent: it is the way back, not the problem.
            if matches!(h, Health::Suspect | Health::Dead) {
                let threshold = st
                    .peers
                    .get(&id)
                    .map_or(Duration::ZERO, |p| p.det.threshold());
                let l = {
                    let links = self.links.lock().expect("links");
                    links.get(&id).and_then(|p| p.ctl.clone())
                };
                if let Some(l) = l
                    && l.created.elapsed() > threshold
                {
                    l.close();
                    self.unregister(&l);
                }
            }
        }
        if changed {
            self.acks
                .send_replace(self.seq_ack_counter.fetch_add(1, Ordering::Relaxed));
            self.wake.notify_one();
        }
    }

    async fn dial_missing(self: &Arc<Self>) {
        let want: Vec<MachineId> = {
            let st = self.st.lock().await;
            st.members
                .keys()
                .filter(|k| **k != self.me)
                .cloned()
                .collect()
        };
        let now = Instant::now();
        for id in want {
            if self.ctl_link(&id).is_some() {
                continue;
            }
            let go = {
                let mut st = self.st.lock().await;
                let p = self.peer_state(&mut st, &id);
                if p.dialing || now < p.next_dial {
                    false
                } else {
                    p.dialing = true;
                    true
                }
            };
            if !go {
                continue;
            }
            let me = self.clone();
            tokio::spawn(async move {
                let r = me.dial_chan(&id, Chan::Control).await;
                let mut st = me.st.lock().await;
                let max = me.cfg.dial_backoff_max;
                let p = me.peer_state(&mut st, &id);
                p.dialing = false;
                if let Err(e) = &r {
                    // Right after a link drops the first few redials come
                    // fast (is the machine there?); only then does the wait
                    // grow.
                    let refusal = is_refusal(e);
                    if !(refusal && p.refusals < 3) {
                        p.backoff = (p.backoff * 2).min(max);
                    }
                    let jitter = Duration::from_millis(rand::random::<u64>() % 40);
                    p.next_dial = Instant::now() + p.backoff + jitter;
                    // Look again when the backoff ends, not at the next tick.
                    let (me2, after) = (me.clone(), p.backoff + jitter);
                    tokio::spawn(async move {
                        tokio::time::sleep(after).await;
                        me2.tick_wake.notify_one();
                    });
                    // Someone answered to say no (or hung up before hello):
                    // after three in a row, nobody is running the node.
                    if refusal {
                        p.refusals += 1;
                        let began = *p.first_refusal.get_or_insert_with(Instant::now);
                        // Three refusals in a row, over at least the silence
                        // floor: a link that flaps for a few seconds gives
                        // the same refusals and must not count as a death.
                        if p.refusals >= 3
                            && began.elapsed() >= me.cfg.timing.floor
                            && !p.last_health.is_down()
                        {
                            tracing::info!(peer = %id, "replica: the machine refuses connections; counting it as down");
                            p.det.refused(Instant::now());
                            p.last_health = Health::Dead;
                            p.down_since = Some(Instant::now());
                            drop(st);
                            me.wake.notify_one();
                        }
                    } else {
                        p.refusals = 0;
                        p.first_refusal = None;
                    }
                } else {
                    p.refusals = 0;
                    p.first_refusal = None;
                }
            });
        }
    }

    pub(super) async fn status(&self) -> ReplicaStatus {
        let st = self.st.lock().await;
        let layout = self.layout_locked(&st);
        let owner = st.eras.owner().to_string();
        let tail = st.log.tail();
        let name_of = |id: &str| {
            st.members
                .get(id)
                .map(|m| m.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.to_string())
        };
        let mut peers = vec![];
        let mut copies_now = 1usize;
        let owner_tail = if owner == self.me {
            tail
        } else {
            st.peers.get(&owner).map_or(tail, |p| p.tail)
        };
        for (id, m) in &st.members {
            if *id == self.me {
                continue;
            }
            let p = st.peers.get(id);
            let health = p.map_or(Health::Dead, |p| p.last_health);
            let applied = p.map_or(Pos::ZERO, |p| {
                if owner == self.me { p.acked } else { p.applied }
            });
            let role = layout.role_of(id);
            let lag = if applied.epoch == owner_tail.epoch {
                owner_tail.seq.saturating_sub(applied.seq)
            } else if applied < owner_tail {
                owner_tail.seq.max(1)
            } else {
                0
            };
            if role == Role::Replica && !health.is_down() && lag == 0 && applied >= owner_tail {
                copies_now += 1;
            }
            peers.push(PeerView {
                id: id.clone(),
                name: m.name.clone(),
                role,
                health: health.word().into(),
                connected: self.ctl_link(id).is_some(),
                owner: *id == owner,
                applied,
                lag,
                problem: p.and_then(|p| p.problem.clone()),
            });
        }
        let wanted = layout.replicas.len().max(1);
        let unreplicated = if owner == self.me {
            let mut acks: Vec<Pos> = layout
                .replicas
                .iter()
                .filter(|r| **r != self.me)
                .map(|r| st.peers.get(r).map_or(Pos::ZERO, |p| p.acked))
                .collect();
            acks.sort();
            if wanted <= 1 {
                0
            } else {
                let best = acks.last().copied().unwrap_or(Pos::ZERO);
                if best.epoch == tail.epoch {
                    tail.seq.saturating_sub(best.seq)
                } else {
                    tail.seq
                }
            }
        } else {
            0
        };
        ReplicaStatus {
            me: self.me.clone(),
            name: self.name.clone(),
            role: layout.role_of(&self.me),
            owner: owner.clone(),
            owner_name: name_of(&owner),
            i_own: owner == self.me,
            epoch: st.eras.epoch(),
            applied: tail,
            tail,
            peers,
            pending: st
                .outbox
                .items()
                .iter()
                .map(|p| PendingView {
                    key: p.key.clone(),
                    size: p.meta.as_ref().map_or(0, |m| m.size),
                    since_ms: p.ts_ms,
                    deleted: p.meta.is_none(),
                })
                .collect(),
            pending_bytes: st.outbox.bytes(),
            copies_now,
            copies_wanted: wanted,
            unreplicated,
            volume_bytes: st.volume_bytes,
            bytes_sent: self.counters.sent.load(Ordering::Relaxed),
            bytes_received: self.counters.received.load(Ordering::Relaxed),
            takeovers: st.takeovers,
            handbacks: st.handbacks,
            fenced_writes: st.fenced_writes,
            conflict_copies: st.conflict_copies,
        }
    }

    pub(super) async fn summary(&self, run_on: Option<&str>) -> Summary {
        let s = self.status().await;
        let st = self.st.lock().await;
        let facts = self.my_facts();
        let layout = self.layout_locked(&st);
        let mut holders_up = vec![];
        let mut holders_away = vec![];
        let mut hold = |name: String, up: bool| {
            if up {
                holders_up.push(name)
            } else {
                holders_away.push(name)
            }
        };
        if layout.role_of(&self.me) == Role::Replica {
            hold(self.name.clone(), true);
        }
        for p in &s.peers {
            if p.role == Role::Replica {
                hold(p.name.clone(), p.health == "up" || p.health == "slow");
            }
        }
        let owner_name = s.owner_name.clone();
        let pending_for = if s.pending.is_empty() {
            None
        } else {
            Some(owner_name)
        };
        Summary {
            this_machine: self.name.clone(),
            run_on: run_on.map(str::to_string),
            machines: st.members.len(),
            holders_up,
            holders_away,
            pending_here: s.pending.len() as u64,
            pending_for,
            this_sleeps: facts.sleep_prone,
            sleep_blocked: facts.sleep_prevented,
        }
    }
}
