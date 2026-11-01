// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who owns the volume: takeover when the owner is gone, hand-back when a
//! more reliable machine returns.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::detector::Health;
use super::era::{EraRec, Pos};
use super::node::{Inner, save_eras};
use super::roles::{self, Role};
use super::wire::Msg;

impl Inner {
    /// A machine holding a copy takes the volume when its owner is down.
    pub(super) async fn maybe_takeover(self: &Arc<Self>) {
        struct Plan {
            epoch: u64,
            base: Pos,
            need: usize,
            owner: String,
        }
        let plan = {
            let mut st = self.st.lock().await;
            let owner = st.eras.owner().to_string();
            if owner == self.me {
                return;
            }
            if let Some(t) = st.last_takeover_try
                && t.elapsed() < self.cfg.takeover_cooldown
            {
                return;
            }
            let layout = self.layout_locked(&st);
            // A machine that keeps no copy never takes the volume.
            if layout.role_of(&self.me) != Role::Replica {
                return;
            }
            let Some(op) = st.peers.get(&owner) else {
                return;
            };
            if !op.last_health.is_down() || !op.det.down_and_silent(Instant::now()) {
                return;
            }
            // A machine that does not need to order writes leaves a flaky
            // owner alone (its changes wait in the outbox); one that does,
            // or one whose owner announced it was leaving, or has been gone
            // a long time, takes over.
            let wanted = st.authority_wanted.is_some_and(|t| {
                t.elapsed() < self.cfg.timing.confirm * 8 + Duration::from_secs(5)
            });
            let gone_long = op
                .down_since
                .is_some_and(|t| t.elapsed() >= self.cfg.idle_takeover);
            if !(wanted || op.intentional || gone_long) {
                return;
            }
            // The best replica that is up takes it; the others wait.
            let my = self.my_facts();
            for r in &layout.replicas {
                if *r == self.me || *r == owner {
                    continue;
                }
                let alive = st
                    .peers
                    .get(r)
                    .is_some_and(|p| p.last_health == Health::Alive);
                if !alive {
                    continue;
                }
                let theirs = self.member_facts(&st, r);
                if (theirs.rank, std::cmp::Reverse(r)) > (my.rank, std::cmp::Reverse(&self.me)) {
                    return;
                }
            }
            let n = self.active_members(&st).len();
            let need = if n >= 3 { n / 2 + 1 } else { 1 };
            st.last_takeover_try = Some(Instant::now());
            Plan {
                epoch: st.eras.epoch() + 1,
                base: st.log.tail(),
                need,
                owner,
            }
        };
        if plan.need > 1 {
            let peers: Vec<_> = {
                let st = self.st.lock().await;
                st.peers
                    .iter()
                    .filter(|(k, p)| **k != plan.owner && p.last_health == Health::Alive)
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            let mut set = tokio::task::JoinSet::new();
            for p in peers {
                if let Some(l) = self.ctl_link(&p) {
                    let (epoch, base) = (plan.epoch, plan.base);
                    let to = self.cfg.call_timeout.min(Duration::from_secs(5));
                    set.spawn(async move {
                        matches!(
                            l.call(Msg::Prepare { epoch, base }, &[], to).await,
                            Ok((Msg::Promise { ok: true, .. }, _))
                        )
                    });
                }
            }
            let mut yes = 1;
            while let Some(r) = set.join_next().await {
                if r.unwrap_or(false) {
                    yes += 1;
                }
            }
            if yes < plan.need {
                tracing::info!(
                    yes,
                    need = plan.need,
                    "replica: not enough machines agree to take over"
                );
                return;
            }
        }
        let recs = {
            let mut st = self.st.lock().await;
            if st.eras.epoch() >= plan.epoch || st.eras.owner() != plan.owner {
                return;
            }
            let base = st.log.tail();
            st.eras.adopt(EraRec {
                epoch: plan.epoch,
                owner: self.me.clone(),
                base,
                at_ms: self.wall_ms(),
            });
            let _ = save_eras(&self.dir, &st.eras);
            st.takeovers += 1;
            tracing::info!(
                epoch = plan.epoch,
                "replica: this machine took over the volume"
            );
            st.eras
                .recs
                .iter()
                .rev()
                .take(4)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
        };
        self.announce(recs).await;
        self.wake.notify_one();
    }

    pub(super) async fn announce(self: &Arc<Self>, recs: Vec<EraRec>) {
        let links: Vec<_> = {
            let l = self.links.lock().expect("links");
            l.values().filter_map(|p| p.ctl.clone()).collect()
        };
        for l in links {
            let recs = recs.clone();
            let to = self.cfg.call_timeout.min(Duration::from_secs(5));
            tokio::spawn(async move {
                let _ = l.call(Msg::Era { recs }, &[], to).await;
            });
        }
    }

    pub(super) async fn handle_prepare(&self, from: &str, epoch: u64, _base: Pos) -> Msg {
        let mut st = self.st.lock().await;
        let no = |reason: &str| Msg::Promise {
            ok: false,
            reason: reason.into(),
        };
        if epoch <= st.eras.epoch() {
            return no("that epoch has passed");
        }
        let owner = st.eras.owner().to_string();
        if owner == from {
            return no("the candidate is the owner");
        }
        if owner != self.me
            && !st
                .peers
                .get(&owner)
                .is_some_and(|p| p.last_health.is_down() && p.det.down_and_silent(Instant::now()))
        {
            return no("the owner is up from here");
        }
        let now = Instant::now();
        for (k, p) in &st.peers {
            if k != from
                && let Some((e, until)) = p.promised
                && e >= epoch
                && until > now
            {
                return no("already promised another machine");
            }
        }
        let until = now + self.cfg.timing.confirm * 4;
        self.peer_state(&mut st, from).promised = Some((epoch, until));
        Msg::Promise {
            ok: true,
            reason: String::new(),
        }
    }

    pub(super) async fn handle_era(self: &Arc<Self>, _from: &str, recs: Vec<EraRec>) {
        let advanced = {
            let mut st = self.st.lock().await;
            let a = st.eras.adopt_from(&recs);
            if a {
                let _ = save_eras(&self.dir, &st.eras);
                self.refresh_roles(&mut st);
            }
            a
        };
        if advanced {
            let me = self.clone();
            tokio::spawn(async move { me.reconcile_eras().await });
            self.wake.notify_one();
        }
    }

    /// The owner gives the volume back to a clearly more reliable machine
    /// that has been up and caught up for a while.
    pub(super) async fn maybe_handoff(self: &Arc<Self>) {
        let mut st = self.st.lock().await;
        if st.eras.owner() != self.me {
            return;
        }
        if let Some(t) = st.last_takeover_try
            && t.elapsed() < self.cfg.takeover_cooldown
        {
            return;
        }
        let layout = self.layout_locked(&st);
        let target = layout.owner.clone();
        if target == self.me || target.is_empty() {
            return;
        }
        let mine = self.my_facts();
        let Some(p) = st.peers.get(&target) else {
            return;
        };
        let stable = p
            .alive_since
            .is_some_and(|t| t.elapsed() >= self.cfg.handback_after);
        let tail = st.log.tail();
        if p.last_health != Health::Alive
            || !stable
            || p.acked < tail
            || !roles::clearly_better(&p.facts, &mine)
        {
            return;
        }
        let Some(link) = self.ctl_link(&target) else {
            return;
        };
        let epoch = st.eras.epoch() + 1;
        st.last_takeover_try = Some(Instant::now());
        // Nothing is written while the offer is out (the lock is held), so
        // the base is exact.
        let r = link
            .call(
                Msg::Handoff {
                    epoch,
                    to: target.clone(),
                    base: tail,
                },
                &[],
                self.cfg.call_timeout.min(Duration::from_secs(5)),
            )
            .await;
        if !matches!(r, Ok((Msg::HandoffReply { ok: true, .. }, _))) {
            return;
        }
        st.eras.adopt(EraRec {
            epoch,
            owner: target.clone(),
            base: tail,
            at_ms: self.wall_ms(),
        });
        let _ = save_eras(&self.dir, &st.eras);
        st.handbacks += 1;
        tracing::info!(to = %target, epoch, "replica: the volume went back to a more reliable machine");
        let recs: Vec<EraRec> = st.eras.recs.iter().rev().take(4).rev().cloned().collect();
        drop(st);
        self.announce(recs).await;
        self.wake.notify_one();
    }

    pub(super) async fn handle_handoff(
        &self,
        from: &str,
        epoch: u64,
        to: String,
        base: Pos,
    ) -> Msg {
        let mut st = self.st.lock().await;
        let no = |reason: &str| Msg::HandoffReply {
            ok: false,
            reason: reason.into(),
        };
        if to != self.me {
            return no("not for this machine");
        }
        if from != st.eras.owner() || epoch != st.eras.epoch() + 1 {
            return no("the offer is stale");
        }
        if st.log.tail() != base {
            return no("this machine is not caught up");
        }
        st.eras.adopt(EraRec {
            epoch,
            owner: self.me.clone(),
            base,
            at_ms: self.wall_ms(),
        });
        let _ = save_eras(&self.dir, &st.eras);
        st.handbacks += 1;
        self.wake.notify_one();
        Msg::HandoffReply {
            ok: true,
            reason: String::new(),
        }
    }
}
