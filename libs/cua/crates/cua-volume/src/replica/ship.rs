// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The owner side of replication: ship the log to every machine that holds
//! a copy, resuming from what each has applied.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::node::{Inner, save_eras};
use super::oplog::Op;
use super::roles::Role;
use super::wire::{Msg, RecordW};
use crate::backend::Backend;

pub(super) type Shipping = std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>;

impl Inner {
    pub(super) async fn ship_all_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            self.wait_unfrozen().await;
            if self.self_away.load(Ordering::SeqCst) {
                continue;
            }
            let targets: Vec<String> = {
                let st = self.st.lock().await;
                if st.eras.owner() != self.me {
                    continue;
                }
                let layout = self.layout_locked(&st);
                layout
                    .replicas
                    .iter()
                    .filter(|r| **r != self.me && layout.role_of(r) == Role::Replica)
                    .cloned()
                    .collect()
            };
            for peer in targets {
                if self.ctl_link(&peer).is_none() {
                    continue;
                }
                let flag = {
                    let mut m = self.shipping.lock().expect("shipping");
                    m.entry(peer.clone()).or_default().clone()
                };
                if flag.swap(true, Ordering::SeqCst) {
                    continue;
                }
                let me = self.clone();
                tokio::spawn(async move {
                    me.ship_until_idle(&peer).await;
                    flag.store(false, Ordering::SeqCst);
                });
            }
        }
    }

    async fn ship_until_idle(self: &Arc<Self>, peer: &str) {
        let mut last_acked = None;
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            let Some(link) = self.ctl_link(peer) else {
                return;
            };
            let (epoch, acked, recs, tail) = {
                let st = self.st.lock().await;
                if st.eras.owner() != self.me {
                    return;
                }
                let acked = st
                    .peers
                    .get(peer)
                    .map_or(super::era::Pos::ZERO, |p| p.acked);
                (
                    st.eras.epoch(),
                    acked,
                    st.log.after(acked, self.cfg.batch),
                    st.log.tail(),
                )
            };
            if last_acked == Some(acked) {
                // No progress since the last round: wait for news.
                return;
            }
            let Some(recs) = recs else {
                // The log no longer reaches this replica: it pulls a snapshot.
                let r = link
                    .call(Msg::Snapshot { at: tail }, &[], self.cfg.data_timeout * 30)
                    .await;
                match r {
                    Ok((Msg::Ack { applied }, _)) => self.set_acked(peer, applied).await,
                    Ok((Msg::Error { message, .. }, _)) => {
                        tracing::warn!(peer, %message, "replica: a snapshot could not be applied");
                        return;
                    }
                    _ => return,
                }
                last_acked = Some(acked);
                continue;
            };
            if recs.is_empty() {
                return;
            }
            let mut data: Vec<u8> = vec![];
            let mut out = Vec::with_capacity(recs.len());
            for r in recs {
                let mut body = None;
                if let Op::Put { meta, .. } = &r.op
                    && meta.size <= self.cfg.inline_max as u64
                    && data.len() + (meta.size as usize) < (1 << 20)
                    && let Ok((bytes, _)) = self.store.get(&meta.key, Some(&meta.version)).await
                {
                    body = Some((data.len() as u64, bytes.len() as u64));
                    data.extend_from_slice(&bytes);
                }
                out.push(RecordW {
                    pos: r.pos,
                    op: r.op,
                    at_ms: r.at_ms,
                    body,
                });
            }
            let t0 = std::time::Instant::now();
            let n_out = out.len();
            let reply = link
                .call(
                    Msg::Append {
                        epoch,
                        records: out,
                    },
                    &data,
                    self.cfg.call_timeout * 10,
                )
                .await;
            tracing::debug!(peer, n = n_out, took = ?t0.elapsed(), ok = reply.is_ok(), "replica: append sent");
            last_acked = Some(acked);
            match reply {
                Ok((Msg::Ack { applied }, _)) => {
                    self.set_acked(peer, applied).await;
                    if applied > acked {
                        last_acked = None;
                    }
                }
                Ok((Msg::Behind { applied }, _)) => {
                    {
                        let mut st = self.st.lock().await;
                        if let Some(p) = st.peers.get_mut(peer) {
                            p.acked = applied;
                        }
                    }
                    if applied != acked {
                        last_acked = None;
                    }
                }
                Ok((Msg::Fenced { eras }, _)) => {
                    let advanced = {
                        let mut st = self.st.lock().await;
                        let a = st.eras.adopt_from(&eras);
                        if a {
                            let _ = save_eras(&self.dir, &st.eras);
                        }
                        a
                    };
                    if advanced {
                        self.reconcile_eras().await;
                    }
                    return;
                }
                Ok((Msg::Error { kind, message }, _)) => {
                    let _ = kind;
                    {
                        let mut st = self.st.lock().await;
                        if let Some(p) = st.peers.get_mut(peer) {
                            p.problem = Some(message);
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    async fn set_acked(&self, peer: &str, applied: super::era::Pos) {
        {
            let mut st = self.st.lock().await;
            let sample = st
                .written_at
                .get(&applied)
                .map(|t| t.elapsed().as_millis() as f64);
            if let Some(p) = st.peers.get_mut(peer) {
                if let Some(ms) = sample {
                    p.ack_rtt_ms = if p.ack_rtt_ms == 0.0 {
                        ms
                    } else {
                        0.7 * p.ack_rtt_ms + 0.3 * ms
                    };
                }
                if applied > p.acked {
                    p.acked = applied;
                }
                p.applied = applied;
                p.problem = None;
                p.missed_acks = 0;
            }
        }
        self.acks
            .send_replace(self.seq_ack_counter.fetch_add(1, Ordering::Relaxed));
    }
}
