// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The replica side: apply the owner's records in order, catch up from a
//! snapshot, and put right a copy that was on the wrong side of a takeover.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use super::era::{Eras, Pos};
use super::link::Link;
use super::migrate::{self, Unlogged};
use super::node::{Inner, save_eras};
use super::oplog::{Op, Record};
use super::wire::{Cond, Hello, Msg, Pending, Rep, Req, Src};
use crate::backend::{Backend, ObjectMeta};
use crate::fs::ApplySource;
use crate::{Error, Result};

impl Inner {
    /// Where the bytes of `meta` can come from without the network, if
    /// anywhere on this machine.
    async fn local_source(&self, meta: &ObjectMeta) -> Option<ApplySource> {
        if meta.size == 0 {
            return Some(ApplySource::Bytes(vec![]));
        }
        if let Ok(Some(h)) = self.store.head(&meta.key).await
            && h.etag == meta.etag
            && let Ok(path) = self.store.version_file(&meta.key, &h.version)
            && path.is_file()
        {
            return Some(ApplySource::File { path, link: true });
        }
        let pending: Vec<Pending> = {
            let st = self.st.lock().await;
            st.outbox
                .items()
                .iter()
                .filter(|p| {
                    p.meta
                        .as_ref()
                        .is_some_and(|m| m.etag == meta.etag && m.size == meta.size)
                })
                .cloned()
                .collect()
        };
        for p in pending {
            let path = match &p.src {
                Src::Spool { key, version } => self.spool.version_file(key, version),
                Src::Data { key, version } => self.store.version_file(key, version),
            };
            if let Ok(path) = path
                && path.is_file()
            {
                return Some(ApplySource::File { path, link: true });
            }
        }
        None
    }

    /// The bytes of `meta`, from this machine if it has them, else pulled
    /// from `from_peer`.
    pub(super) async fn acquire(
        self: &Arc<Self>,
        from_peer: &str,
        meta: &ObjectMeta,
        inline: Option<Vec<u8>>,
    ) -> Result<ApplySource> {
        if let Some(b) = inline {
            return Ok(ApplySource::Bytes(b));
        }
        if let Some(s) = self.local_source(meta).await {
            return Ok(s);
        }
        let dl = self
            .data_link(from_peer)
            .await
            .ok_or_else(|| Error::Backend("no data link to the owner".into()))?;
        let seed = self.store.head(&meta.key).await.ok().flatten();
        let path = self
            .pull(
                &dl,
                Src::Data {
                    key: meta.key.clone(),
                    version: meta.version.clone(),
                },
                meta,
                seed.as_ref(),
            )
            .await?;
        Ok(ApplySource::File { path, link: false })
    }

    pub(super) async fn handle_append(
        self: &Arc<Self>,
        link: &Arc<Link>,
        epoch: u64,
        records: Vec<super::wire::RecordW>,
        data: Vec<u8>,
    ) -> Msg {
        let t0 = std::time::Instant::now();
        let _g = self.gate.lock().await;
        tracing::debug!(n = records.len(), gate_wait = ?t0.elapsed(), "replica: append received");
        {
            let mut st = self.st.lock().await;
            if link.peer != st.eras.owner() || epoch < st.eras.epoch() {
                st.fenced_writes += records.len() as u64;
                return Msg::Fenced {
                    eras: st.eras.recs.clone(),
                };
            }
            if epoch > st.eras.epoch() {
                return Msg::Behind {
                    applied: st.log.tail(),
                };
            }
        }
        // Runs of small puts that came with their bytes are applied as one
        // batch (one disk barrier, one run of log records).
        let mut records = records.into_iter().peekable();
        while let Some(rw) = records.next() {
            if let (Op::Put { .. }, Some(_)) = (&rw.op, rw.body) {
                let mut run = vec![rw];
                while run.len() < 256
                    && records
                        .peek()
                        .is_some_and(|n| matches!(n.op, Op::Put { .. }) && n.body.is_some())
                {
                    run.push(records.next().expect("peeked"));
                }
                let tail = self.st.lock().await.log.tail();
                let mut expect = tail;
                let mut items = vec![];
                let mut recs = vec![];
                let mut gap = false;
                for r in run {
                    if r.pos <= tail {
                        continue;
                    }
                    let contiguous = if r.pos.epoch == expect.epoch {
                        r.pos.seq == expect.seq + 1
                    } else {
                        r.pos.epoch > expect.epoch && r.pos.seq == 1
                    };
                    if !contiguous {
                        gap = true;
                        break;
                    }
                    expect = r.pos;
                    let (o, l) = r.body.expect("a body");
                    let Some(bytes) = data.get(o as usize..(o + l) as usize) else {
                        gap = true;
                        break;
                    };
                    if let Op::Put { meta, .. } = &r.op {
                        items.push((meta.clone(), bytes.to_vec()));
                    }
                    recs.push(Record {
                        pos: r.pos,
                        op: r.op,
                        at_ms: r.at_ms,
                    });
                }
                if !items.is_empty() {
                    if let Err(e) = self.store.apply_many(items).await {
                        if e.to_string().contains("No space")
                            || e.to_string().contains("disk is full")
                        {
                            self.st.lock().await.healthy = false;
                        }
                        return Msg::Error {
                            kind: e.tag().into(),
                            message: e.to_string(),
                        };
                    }
                    let last = recs.last().map(|r| r.pos);
                    let mut st = self.st.lock().await;
                    st.healthy = true;
                    if let Err(e) = st.log.append_batch(recs) {
                        return Msg::Error {
                            kind: e.tag().into(),
                            message: e.to_string(),
                        };
                    }
                    if let Some(p) = last {
                        self.applied_tx.send_replace(p);
                    }
                }
                if gap {
                    let t = self.st.lock().await.log.tail();
                    return Msg::Behind { applied: t };
                }
                continue;
            }
            let tail = self.st.lock().await.log.tail();
            if rw.pos <= tail {
                tracing::debug!(me = %self.me, pos = %rw.pos, %tail, key = %rw.op.key(), "replica: skipped (not after tail)");
                continue;
            }
            let contiguous = if rw.pos.epoch == tail.epoch {
                rw.pos.seq == tail.seq + 1
            } else {
                rw.pos.epoch > tail.epoch && rw.pos.seq == 1
            };
            if !contiguous {
                return Msg::Behind { applied: tail };
            }
            let inline = rw
                .body
                .and_then(|(o, l)| data.get(o as usize..(o + l) as usize).map(<[u8]>::to_vec));
            let r: Result<()> = async {
                match &rw.op {
                    Op::Put { meta, .. } => {
                        let src = self.acquire(&link.peer, meta, inline).await?;
                        self.store.apply_put(meta.clone(), src).await?;
                    }
                    Op::Delete { key, version, .. } => {
                        self.store.apply_delete(key, version).await?;
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = r {
                let full =
                    e.to_string().contains("disk is full") || e.to_string().contains("No space");
                if full {
                    self.st.lock().await.healthy = false;
                }
                return Msg::Error {
                    kind: e.tag().into(),
                    message: e.to_string(),
                };
            }
            let mut st = self.st.lock().await;
            st.healthy = true;
            tracing::debug!(me = %self.me, pos = %rw.pos, key = %rw.op.key(), "replica: applied");
            if let Err(e) = st.log.append(Record {
                pos: rw.pos,
                op: rw.op,
                at_ms: rw.at_ms,
            }) {
                return Msg::Error {
                    kind: e.tag().into(),
                    message: e.to_string(),
                };
            }
            self.applied_tx.send_replace(rw.pos);
        }
        let st = self.st.lock().await;
        Msg::Ack {
            applied: st.log.tail(),
        }
    }

    pub(super) async fn handle_snapshot(self: &Arc<Self>, link: &Arc<Link>, at: Pos) -> Msg {
        let _g = self.gate.lock().await;
        match self.pull_snapshot(link, at).await {
            Ok(()) => Msg::Ack { applied: at },
            Err(e) => Msg::Error {
                kind: e.tag().into(),
                message: e.to_string(),
            },
        }
    }

    async fn pull_snapshot(self: &Arc<Self>, link: &Arc<Link>, at: Pos) -> Result<()> {
        let peer = link.peer.clone();
        let dl = self
            .data_link(&peer)
            .await
            .ok_or_else(|| Error::Backend("no data link to the owner".into()))?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut after = String::new();
        loop {
            let (m, _) = dl
                .call(
                    Msg::Req(Req::Heads {
                        after_key: after.clone(),
                        max: 512,
                    }),
                    &[],
                    self.cfg.data_timeout,
                )
                .await
                .map_err(|e| Error::Backend(e.to_string()))?;
            let (items, done) = match m {
                Msg::Rep(Rep::Heads { items, done, .. }) => (items, done),
                Msg::Rep(Rep::Fail { kind, message }) => {
                    return Err(Rep::into_error(&kind, message));
                }
                _ => return Err(Error::Backend("unexpected reply".into())),
            };
            for meta in &items {
                seen.insert(meta.key.clone());
                if let Some(local) = self.store.head(&meta.key).await?
                    && local.version == meta.version
                {
                    continue;
                }
                if self.st.lock().await.outbox.has_key(&meta.key) {
                    continue;
                }
                let src = self.acquire(&peer, meta, None).await?;
                self.store.apply_put(meta.clone(), src).await?;
            }
            if let Some(last) = items.last() {
                after = last.key.clone();
            }
            if done || items.is_empty() {
                break;
            }
        }
        // Heads that are not the owner's and not waiting to be sent are stale.
        for local in self.store.list("").await? {
            if !seen.contains(&local.key) && !self.st.lock().await.outbox.has_key(&local.key) {
                self.store.set_head(&local.key, None).await?;
            }
        }
        let mut st = self.st.lock().await;
        st.log.reset_to(at)?;
        drop(st);
        self.applied_tx.send_replace(at);
        Ok(())
    }

    /// This machine holds records past a later era's base (it was the owner
    /// while another took over, or it was ahead): move those heads back,
    /// keep every version, and queue the changes for the new owner.
    pub(super) async fn reconcile_eras(self: &Arc<Self>) {
        let _g = self.gate.lock().await;
        if let Err(e) = self.reconcile_locked().await {
            tracing::warn!(error = %e, "replica: could not hand over this machine's divergent changes yet");
        }
    }

    async fn reconcile_locked(self: &Arc<Self>) -> Result<()> {
        let tail_records = {
            let mut st = self.st.lock().await;
            let Some(keep) = st.eras.divergence(st.log.tail()) else {
                return Ok(());
            };
            tracing::info!(%keep, "replica: this machine was ahead of the new owner; handing over its tail");
            st.log.cut_after(keep)?
        };
        tracing::debug!(n = tail_records.len(), first = ?tail_records.last().map(|r| r.op.key().to_string()), "replica: tail cut");
        // Newest first: rewind each head to what it was before.
        for rec in &tail_records {
            match &rec.op {
                Op::Put { meta, prev } => {
                    if self
                        .store
                        .head(&meta.key)
                        .await?
                        .is_some_and(|h| h.version == meta.version)
                    {
                        self.store.set_head(&meta.key, prev.clone()).await?;
                    }
                }
                Op::Delete { key, prev, .. } => {
                    if self.store.head(key).await?.is_none() {
                        self.store.set_head(key, prev.clone()).await?;
                    }
                }
            }
        }
        // Oldest first: they go to the new owner as changes against what
        // each key was before.
        let mut st = self.st.lock().await;
        for rec in tail_records.iter().rev() {
            let (key, meta, prev) = match &rec.op {
                Op::Put { meta, prev } => (meta.key.clone(), Some(meta.clone()), prev.clone()),
                Op::Delete { key, prev, .. } => (key.clone(), None, prev.clone()),
            };
            let src = Src::Data {
                key: key.clone(),
                version: meta.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
            };
            st.outbox.add(Pending {
                id: crate::new_id(),
                key,
                meta,
                base: prev.map(|p| p.version),
                cond: Cond::None,
                machine: self.me.clone(),
                name: self.name.clone(),
                ts_ms: self.wall_ms(),
                src,
                body: None,
            })?;
        }
        let tail = st.log.tail();
        for p in st.peers.values_mut() {
            if p.acked > tail {
                p.acked = tail;
            }
        }
        Ok(())
    }

    /// Two volumes that started apart met; this machine is the less
    /// reliable one and joins the other's: everything it has goes to the new
    /// owner as changes against nothing (identical files merge, different
    /// ones become conflict copies).
    pub(super) async fn join_lineage(self: &Arc<Self>, h: Hello) {
        let _g = self.gate.lock().await;
        let mut st = self.st.lock().await;
        if st.eras.lineage == h.lineage {
            return;
        }
        tracing::info!(other = %h.machine, "replica: joining another machine's volume");
        let heads = match self.store.list("").await {
            Ok(h) => h,
            Err(_) => return,
        };
        for meta in heads {
            if st.outbox.has_key(&meta.key) {
                continue;
            }
            let _ = st.outbox.add(Pending {
                id: crate::new_id(),
                key: meta.key.clone(),
                meta: Some(meta.clone()),
                base: None,
                cond: Cond::None,
                machine: self.me.clone(),
                name: self.name.clone(),
                ts_ms: self.wall_ms(),
                src: Src::Data {
                    key: meta.key.clone(),
                    version: meta.version.clone(),
                },
                body: None,
            });
        }
        st.eras = Eras {
            lineage: h.lineage.clone(),
            recs: h.eras.clone(),
        };
        let _ = save_eras(&self.dir, &st.eras);
        let _ = st.log.reset_to(Pos::ZERO);
        self.applied_tx.send_replace(Pos::ZERO);
        for p in st.peers.values_mut() {
            p.acked = Pos::ZERO;
        }
    }

    #[allow(dead_code)]
    pub(super) fn incoming_dir(&self) -> PathBuf {
        self.dir.join("incoming")
    }
}

impl Inner {
    /// Changes made to the store while replication was not looking (an older
    /// build on the same home) go into the log (this machine owns the
    /// Volume) or to the owner as changes (it does not).
    pub(super) async fn adopt_unlogged(&self) -> Result<()> {
        let unlogged = {
            let st = self.st.lock().await;
            migrate::find_unlogged(&self.dir, &self.store, &st.log).await?
        };
        if unlogged.is_empty() {
            return Ok(());
        }
        tracing::info!(
            n = unlogged.len(),
            "replica: found changes made while replication was off; handing them over"
        );
        let mut st = self.st.lock().await;
        let owner = st.eras.owner() == self.me;
        for u in unlogged {
            if owner {
                let op = match u {
                    Unlogged::Put { meta, prev } => super::oplog::Op::Put { meta, prev },
                    Unlogged::Delete { key, version, prev } => {
                        super::oplog::Op::Delete { key, version, prev }
                    }
                };
                self.commit_locked(&mut st, op)?;
            } else {
                let (key, meta, base, src) = match u {
                    Unlogged::Put { meta, prev } => {
                        let src = Src::Data {
                            key: meta.key.clone(),
                            version: meta.version.clone(),
                        };
                        (meta.key.clone(), Some(meta), prev.map(|p| p.version), src)
                    }
                    Unlogged::Delete { key, prev, .. } => {
                        let src = Src::Data {
                            key: key.clone(),
                            version: String::new(),
                        };
                        (key, None, prev.map(|p| p.version), src)
                    }
                };
                st.outbox.add(Pending {
                    id: crate::new_id(),
                    key,
                    meta,
                    base,
                    cond: Cond::None,
                    machine: self.me.clone(),
                    name: self.name.clone(),
                    ts_ms: self.wall_ms(),
                    src,
                    body: None,
                })?;
            }
        }
        Ok(())
    }

    /// Records what the heads are now (see [`migrate`]).
    pub(super) async fn save_snapshot(&self) {
        let pos = self.st.lock().await.log.tail();
        if let Err(e) = migrate::save_snap(&self.dir, &self.store, pos).await {
            tracing::warn!(error = %e, "replica: could not save the heads snapshot");
        }
    }
}
