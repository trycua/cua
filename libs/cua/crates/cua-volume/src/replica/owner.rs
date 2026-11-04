// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Writes: at the owner (serialized, logged, shipped), forwarded from other
//! machines, and saved for later when the owner is away.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::era::Pos;
use super::link::{CallError, Link};
use super::merge::{self, Resolution};
use super::node::{Inner, State};
use super::oplog::{Op, Record};
use super::status::Durability;
use super::wire::{Chan, Cond, Msg, Outcome, Pending, Rep, Req, Src};
use crate::backend::{Backend, Condition, ObjectMeta};
use crate::fs::{ApplySource, FsBackend};
use crate::{Error, Result};

/// What the owner says when asked to write and it is not the owner (the
/// caller looks up the new owner and retries).
pub(super) const NOT_OWNER: &str = "this machine does not keep the volume now";

pub(super) fn is_not_owner(e: &Error) -> bool {
    matches!(e, Error::Backend(m) if m == NOT_OWNER)
}

/// The bytes of a write.
pub(super) enum Body {
    Bytes(Vec<u8>),
    File(PathBuf),
}

impl Body {
    pub fn size(&self) -> u64 {
        match self {
            Body::Bytes(b) => b.len() as u64,
            Body::File(p) => std::fs::metadata(p).map_or(0, |m| m.len()),
        }
    }
}

/// Why a forward did not happen.
pub(super) enum Fwd {
    /// No answer from the owner.
    Unreachable(String),
    /// The owner answered with an error.
    Remote(Error),
}

fn unreachable_msg(owner: &str) -> String {
    format!("{owner}, which keeps this volume in order, cannot be reached right now")
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Intent {
    keys: Vec<(String, bool)>,
}

impl Inner {
    fn intent_path(&self) -> PathBuf {
        self.dir.join("intent.json")
    }

    fn write_intent(&self, keys: &[(&str, bool)]) -> Result<()> {
        let i = Intent {
            keys: keys.iter().map(|(k, d)| (k.to_string(), *d)).collect(),
        };
        let p = self.intent_path();
        cua_home::guard_write(&p)?;
        std::fs::write(p, serde_json::to_vec(&i)?)?;
        Ok(())
    }

    fn clear_intent(&self) {
        let _ = std::fs::remove_file(self.intent_path());
    }

    /// A crash between the store write and the log append leaves a head
    /// the log does not know. Make it known.
    pub(super) async fn recover_intent(&self) -> Result<()> {
        let Ok(b) = std::fs::read(self.intent_path()) else {
            return Ok(());
        };
        let i: Intent = serde_json::from_slice(&b).unwrap_or_default();
        let mut st = self.st.lock().await;
        if st.eras.owner() == self.me {
            for (key, deleted) in i.keys {
                let head = self.store.head(&key).await?;
                let known = st.log.range(Pos::ZERO, st.log.tail());
                let last = known.iter().rev().find(|r| r.op.key() == key);
                match (deleted, head) {
                    (false, Some(h)) => {
                        let logged = matches!(last.map(|r| &r.op), Some(Op::Put { meta, .. }) if meta.version == h.version);
                        if !logged {
                            self.commit_locked(
                                &mut st,
                                Op::Put {
                                    meta: h,
                                    prev: None,
                                },
                            )?;
                        }
                    }
                    (true, None) => {
                        let logged = matches!(last.map(|r| &r.op), Some(Op::Delete { .. }));
                        if !logged
                            && let Ok(vs) = self.store.versions(&key).await
                            && let Some(v) = vs.iter().find(|v| v.deleted)
                        {
                            self.commit_locked(
                                &mut st,
                                Op::Delete {
                                    key: key.clone(),
                                    version: v.version.clone(),
                                    prev: None,
                                },
                            )?;
                        }
                    }
                    _ => {}
                }
            }
        }
        self.clear_intent();
        Ok(())
    }

    pub(super) fn next_pos(&self, st: &State) -> Pos {
        let epoch = st.eras.epoch();
        let t = st.log.tail();
        Pos {
            epoch,
            seq: if t.epoch == epoch { t.seq + 1 } else { 1 },
        }
    }

    /// Appends `op` to the log at the next position (state lock held).
    pub(super) fn commit_locked(&self, st: &mut State, op: Op) -> Result<Pos> {
        let pos = self.next_pos(st);
        st.log.append(Record {
            pos,
            op,
            at_ms: self.wall_ms(),
        })?;
        st.written_at.insert(pos, Instant::now());
        while st.written_at.len() > 4096 {
            st.written_at.pop_first();
        }
        self.applied_tx.send_replace(pos);
        Ok(pos)
    }

    /// A write at the owner.
    pub(super) async fn owner_put(
        &self,
        key: &str,
        body: Body,
        cond: Condition,
    ) -> Result<(ObjectMeta, Pos)> {
        match self.owner_put_try(key, body, cond).await {
            Ok(r) => r,
            Err(_) => Err(Error::Backend(NOT_OWNER.into())),
        }
    }

    /// A write at the owner; hands the body back when this machine is not
    /// the owner (any more).
    pub(super) async fn owner_put_try(
        &self,
        key: &str,
        body: Body,
        cond: Condition,
    ) -> std::result::Result<Result<(ObjectMeta, Pos)>, Body> {
        let mut st = self.st.lock().await;
        if st.eras.owner() != self.me {
            return Err(body);
        }
        let r = self.owner_put_locked(&mut st, key, body, cond).await;
        drop(st);
        self.wake.notify_one();
        Ok(r)
    }

    async fn owner_put_locked(
        &self,
        st: &mut State,
        key: &str,
        body: Body,
        cond: Condition,
    ) -> Result<(ObjectMeta, Pos)> {
        let prev = self.store.head(key).await?;
        self.write_intent(&[(key, false)])?;
        let meta = match body {
            Body::Bytes(b) => self.store.put(key, b, cond).await,
            Body::File(p) => self.store.put_file(key, &p, cond).await,
        };
        let meta = match meta {
            Ok(m) => m,
            Err(e) => {
                self.clear_intent();
                return Err(e);
            }
        };
        let pos = self.commit_locked(
            st,
            Op::Put {
                meta: meta.clone(),
                prev,
            },
        )?;
        self.clear_intent();
        Ok((meta, pos))
    }

    pub(super) async fn owner_delete(&self, key: &str, cond: Condition) -> Result<Pos> {
        let mut st = self.st.lock().await;
        if st.eras.owner() != self.me {
            return Err(Error::Backend(NOT_OWNER.into()));
        }
        let r = self.owner_delete_locked(&mut st, key, cond).await;
        drop(st);
        self.wake.notify_one();
        r
    }

    async fn owner_delete_locked(&self, st: &mut State, key: &str, cond: Condition) -> Result<Pos> {
        let prev = self.store.head(key).await?;
        self.write_intent(&[(key, true)])?;
        let version = match self.store.delete_versioned(key, cond).await {
            Ok(v) => v,
            Err(e) => {
                self.clear_intent();
                return Err(e);
            }
        };
        let pos = self.commit_locked(
            st,
            Op::Delete {
                key: key.to_string(),
                version,
                prev,
            },
        )?;
        self.clear_intent();
        Ok(pos)
    }

    /// Copies holding `pos`, copies wanted, and copies that could still
    /// hold it (machines holding it, plus holders that are up and have not
    /// acknowledged yet) (state lock held).
    fn copies_at(&self, st: &State, pos: Pos) -> (usize, usize, usize) {
        let layout = self.layout_locked(st);
        let want = layout.replicas.len().max(1);
        let mut copies = 1;
        let mut possible = 1;
        for r in &layout.replicas {
            if *r == self.me {
                continue;
            }
            if let Some(p) = st.peers.get(r)
                && !p.last_health.is_down()
            {
                if p.acked >= pos {
                    copies += 1;
                    possible += 1;
                } else if self.ctl_link(r).is_some()
                    && p.missed_acks < 2
                    && p.ack_rtt_ms * 2.0 < self.cfg.ack_wait.as_millis() as f64
                {
                    possible += 1;
                }
            }
        }
        (copies, want, possible)
    }

    /// Waits up to `ack_wait` for enough copies of `pos`; the write never
    /// fails because a machine is away.
    pub(super) async fn wait_durable(&self, pos: Pos) -> (usize, bool) {
        let deadline = Instant::now() + self.cfg.ack_wait;
        let mut rx = self.acks.subscribe();
        loop {
            let (copies, want, possible) = {
                let st = self.st.lock().await;
                self.copies_at(&st, pos)
            };
            if want >= 2 && copies >= want {
                return (copies, true);
            }
            // One machine, or the other holder is known to be away: there is
            // nobody to wait for, so a write never waits (a closed laptop
            // costs the studio nothing).
            if want < 2 || possible < want {
                return (copies, false);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // Whoever did not answer in time is not waited for again
                // until it answers.
                let mut st = self.st.lock().await;
                let layout = self.layout_locked(&st);
                for r in layout.replicas.iter().filter(|r| **r != self.me) {
                    if let Some(p) = st.peers.get_mut(r)
                        && p.acked < pos
                    {
                        p.missed_acks = p.missed_acks.saturating_add(1);
                    }
                }
                return (copies, false);
            }
            tokio::select! {
                _ = rx.changed() => {}
                _ = tokio::time::sleep(left) => {}
            }
        }
    }

    pub(super) fn durability(copies: usize, durable: bool) -> Durability {
        if durable {
            Durability::Replicated(copies)
        } else {
            Durability::Local(copies)
        }
    }

    /// A forwarded `Backend` call, at the owner or at any machine holding a
    /// copy (reads).
    pub(super) async fn handle_req(
        self: &Arc<Self>,
        _link: &Arc<Link>,
        req: Req,
        data: Vec<u8>,
    ) -> (Rep, Vec<u8>) {
        let fail = |e: Error| (Rep::fail(&e), vec![]);
        match req {
            Req::Put { key, cond } => {
                match self.owner_put(&key, Body::Bytes(data), cond.into()).await {
                    Ok((meta, pos)) => {
                        let (copies, durable) = self.wait_durable(pos).await;
                        (
                            Rep::Meta {
                                meta,
                                pos,
                                copies: copies as u32,
                                durable,
                            },
                            vec![],
                        )
                    }
                    Err(e) => fail(e),
                }
            }
            Req::Delete { key, cond } => match self.owner_delete(&key, cond.into()).await {
                Ok(pos) => {
                    let (copies, durable) = self.wait_durable(pos).await;
                    (
                        Rep::Done {
                            pos,
                            copies: copies as u32,
                            durable,
                        },
                        vec![],
                    )
                }
                Err(e) => fail(e),
            },
            Req::Head { key } => match self.store.head(&key).await {
                Ok(meta) => (Rep::Head { meta }, vec![]),
                Err(e) => fail(e),
            },
            Req::Get { key, version } => match self.store.get(&key, version.as_deref()).await {
                Ok((bytes, meta)) => (Rep::Bytes { meta }, bytes),
                Err(e) => fail(e),
            },
            Req::GetRange {
                key,
                version,
                offset,
                len,
            } => match self.store.get_range(&key, &version, offset, len).await {
                Ok(bytes) => (Rep::Range, bytes),
                Err(e) => fail(e),
            },
            Req::List { prefix } => match self.store.list(&prefix).await {
                Ok(items) => (Rep::List { items }, vec![]),
                Err(e) => fail(e),
            },
            Req::ListDir { folder } => match self.store.list_dir(&folder).await {
                Ok((files, folders)) => (Rep::ListDir { files, folders }, vec![]),
                Err(e) => fail(e),
            },
            Req::Versions { key } => match self.store.versions(&key).await {
                Ok(items) => (Rep::Versions { items }, vec![]),
                Err(e) => fail(e),
            },
            Req::Heads { after_key, max } => {
                let at = self.st.lock().await.log.tail();
                match self.store.list_after("", &after_key, max).await {
                    Ok(items) => {
                        let done = items.len() < max;
                        (Rep::Heads { items, done, at }, vec![])
                    }
                    Err(e) => fail(e),
                }
            }
        }
    }

    // ---- the owner takes pending changes --------------------------------

    pub(super) async fn handle_submit(
        self: &Arc<Self>,
        link: &Arc<Link>,
        ops: Vec<Pending>,
        data: Vec<u8>,
    ) -> Msg {
        if !self.st.lock().await.eras.owner().eq(&self.me) {
            return Msg::Error {
                kind: "not_owner".into(),
                message: NOT_OWNER.into(),
            };
        }
        let mut remap: HashMap<String, String> = HashMap::new();
        let mut outcomes = Vec::with_capacity(ops.len());
        for op in &ops {
            let o = self.apply_pending(Some(link), op, &data, &mut remap).await;
            outcomes.push(o);
        }
        self.wake.notify_one();
        Msg::Submitted { outcomes }
    }

    async fn fetch_body(
        self: &Arc<Self>,
        link: Option<&Arc<Link>>,
        op: &Pending,
        data: &[u8],
    ) -> Result<Body> {
        let meta = op.meta.as_ref().expect("a put");
        // The owner's own outbox: the bytes are on this machine.
        let Some(link) = link else {
            let path = match &op.src {
                Src::Spool { key, version } => self.spool.version_file(key, version)?,
                Src::Data { key, version } => self.store.version_file(key, version)?,
            };
            return Ok(Body::File(path));
        };
        if let Some((off, len)) = op.body {
            let (o, l) = (off as usize, len as usize);
            let bytes = data
                .get(o..o + l)
                .ok_or_else(|| Error::Backend("a body outside its frame".into()))?;
            return Ok(Body::Bytes(bytes.to_vec()));
        }
        let dl = match self.data_link(&link.peer).await {
            Some(l) => l,
            None => return Err(Error::Backend("no data link to the writer".into())),
        };
        let seed = self.store.head(&op.key).await.ok().flatten();
        let path = self.pull(&dl, op.src.clone(), meta, seed.as_ref()).await?;
        Ok(Body::File(path))
    }

    async fn apply_pending(
        self: &Arc<Self>,
        link: Option<&Arc<Link>>,
        op: &Pending,
        data: &[u8],
        remap: &mut HashMap<String, String>,
    ) -> Outcome {
        let base = op
            .base
            .as_ref()
            .map(|b| remap.get(b).cloned().unwrap_or_else(|| b.clone()));
        let Some(meta) = op.meta.as_ref() else {
            // A delete.
            let mut st = self.st.lock().await;
            if st.eras.owner() != self.me {
                return Outcome::Failed {
                    message: NOT_OWNER.into(),
                };
            }
            let head = match self.store.head(&op.key).await {
                Ok(h) => h,
                Err(e) => {
                    return Outcome::Failed {
                        message: e.to_string(),
                    };
                }
            };
            return match head {
                None => Outcome::Merged,
                Some(h) if Some(&h.version) == base.as_ref() => {
                    match self
                        .owner_delete_locked(&mut st, &op.key, Condition::None)
                        .await
                    {
                        Ok(pos) => Outcome::Deleted { pos },
                        Err(e) => Outcome::Failed {
                            message: e.to_string(),
                        },
                    }
                }
                // Someone wrote after the deleter last looked: the write stays.
                Some(_) => Outcome::Kept,
            };
        };
        // The head may be this very change, made visible here while it
        // waited (a version of its own, never written to the log). The owner
        // must log it, not take it for content it already holds.
        if matches!(Condition::from(op.cond.clone()), Condition::None) {
            let mut st = self.st.lock().await;
            if st.eras.owner() == self.me
                && let Ok(Some(h)) = self.store.head(&op.key).await
                && h.version == meta.version
            {
                return match self.commit_locked(
                    &mut st,
                    Op::Put {
                        meta: h.clone(),
                        prev: None,
                    },
                ) {
                    Ok(pos) => {
                        drop(st);
                        self.wake.notify_one();
                        Outcome::Applied { meta: h, pos }
                    }
                    Err(e) => Outcome::Failed {
                        message: e.to_string(),
                    },
                };
            }
        }
        // Content first, outside the lock.
        let cond: Condition = op.cond.clone().into();
        let head0 = self.store.head(&op.key).await.ok().flatten();
        let have_same = head0.as_ref().is_some_and(|h| h.etag == meta.etag);
        let body = if have_same {
            None
        } else {
            match self.fetch_body(link, op, data).await {
                Ok(b) => Some(b),
                Err(e) => {
                    return Outcome::Failed {
                        message: e.to_string(),
                    };
                }
            }
        };
        let mut st = self.st.lock().await;
        if st.eras.owner() != self.me {
            return Outcome::Failed {
                message: NOT_OWNER.into(),
            };
        }
        let head = match self.store.head(&op.key).await {
            Ok(h) => h,
            Err(e) => {
                return Outcome::Failed {
                    message: e.to_string(),
                };
            }
        };
        if let Some(h) = &head
            && h.etag == meta.etag
            && matches!(cond, Condition::None)
        {
            remap.insert(meta.version.clone(), h.version.clone());
            return Outcome::Merged;
        }
        let Some(body) = body else {
            // The head had this content when we looked, and changed since.
            return Outcome::Failed {
                message: "the key changed while it was being merged; it will be tried again".into(),
            };
        };
        let clean = match (&head, &base) {
            (None, _) => true,
            (Some(h), Some(b)) => &h.version == b,
            (Some(_), None) => false,
        };
        if !matches!(cond, Condition::None) {
            // A conditional write is evaluated as written; no merging.
            return match self.owner_put_locked(&mut st, &op.key, body, cond).await {
                Ok((m, pos)) => {
                    remap.insert(meta.version.clone(), m.version.clone());
                    Outcome::Applied { meta: m, pos }
                }
                Err(Error::Precondition(m)) => Outcome::Precondition { message: m },
                Err(e) => Outcome::Failed {
                    message: e.to_string(),
                },
            };
        }
        // An order (fence, lease, run record) is judged by its order even
        // when the change chains onto a version the owner just kept.
        if clean
            && merge::is_ordered(&op.key)
            && let Some(h) = &head
            && h.size <= self.cfg.inline_max as u64 * 4
            && meta.size <= self.cfg.inline_max as u64 * 4
        {
            let head_bytes = self.store.get(&op.key, None).await.map(|(b, _)| b);
            let in_bytes = match &body {
                Body::Bytes(b) => Ok(b.clone()),
                Body::File(p) => std::fs::read(p).map_err(Error::from),
            };
            if let (Ok(hb), Ok(ib)) = (head_bytes, in_bytes)
                && merge::resolve(&op.key, &hb, &ib, &op.name, op.ts_ms) == Resolution::KeepHead
            {
                remap.insert(meta.version.clone(), h.version.clone());
                return Outcome::Merged;
            }
        }
        if clean {
            return match self
                .owner_put_locked(&mut st, &op.key, body, Condition::None)
                .await
            {
                Ok((m, pos)) => {
                    remap.insert(meta.version.clone(), m.version.clone());
                    Outcome::Applied { meta: m, pos }
                }
                Err(e) => Outcome::Failed {
                    message: e.to_string(),
                },
            };
        }
        // The key changed on the owner since the writer looked.
        let head = head.expect("not clean implies a head");
        let incoming_small = meta.size <= self.cfg.inline_max as u64 * 4
            && head.size <= self.cfg.inline_max as u64 * 4;
        let resolution = if incoming_small {
            let head_bytes = self.store.get(&op.key, None).await.map(|(b, _)| b);
            let in_bytes = match &body {
                Body::Bytes(b) => Ok(b.clone()),
                Body::File(p) => std::fs::read(p).map_err(Error::from),
            };
            match (head_bytes, in_bytes) {
                (Ok(h), Ok(i)) => merge::resolve(&op.key, &h, &i, &op.name, op.ts_ms),
                _ => Resolution::Copy(crate::feed::conflict_path(&op.key, &op.name, op.ts_ms)),
            }
        } else {
            Resolution::Copy(crate::feed::conflict_path(&op.key, &op.name, op.ts_ms))
        };
        match resolution {
            Resolution::KeepHead => {
                remap.insert(meta.version.clone(), head.version.clone());
                Outcome::Merged
            }
            Resolution::TakeIncoming => {
                match self
                    .owner_put_locked(&mut st, &op.key, body, Condition::None)
                    .await
                {
                    Ok((m, pos)) => {
                        remap.insert(meta.version.clone(), m.version.clone());
                        Outcome::Applied { meta: m, pos }
                    }
                    Err(e) => Outcome::Failed {
                        message: e.to_string(),
                    },
                }
            }
            Resolution::Copy(ckey) => {
                st.conflict_copies += 1;
                match self
                    .owner_put_locked(&mut st, &ckey, body, Condition::None)
                    .await
                {
                    Ok((m, pos)) => {
                        remap.insert(meta.version.clone(), head.version.clone());
                        Outcome::Copy { key: m.key, pos }
                    }
                    Err(e) => Outcome::Failed {
                        message: e.to_string(),
                    },
                }
            }
        }
    }

    // ---- writes from any machine ----------------------------------------

    /// Reaches the owner's control link, dialing once if there is none.
    async fn owner_ctl(self: &Arc<Self>, owner: &str) -> Option<Arc<Link>> {
        if let Some(l) = self.ctl_link(owner) {
            return Some(l);
        }
        // One quick try; the tick keeps trying in the background.
        match tokio::time::timeout(Duration::from_secs(3), self.dial_chan(owner, Chan::Control))
            .await
        {
            Ok(Ok(l)) => Some(l),
            _ => self.ctl_link(owner),
        }
    }

    pub(super) async fn write_put(
        self: &Arc<Self>,
        key: &str,
        body: Body,
        cond: Condition,
    ) -> Result<(ObjectMeta, Durability)> {
        let mut body = body;
        for attempt in 0..6u32 {
            let owner = {
                let st = self.st.lock().await;
                st.eras.owner().to_string()
            };
            if owner == self.me {
                match self
                    .owner_put_try(
                        key,
                        std::mem::replace(&mut body, Body::Bytes(vec![])),
                        cond.clone(),
                    )
                    .await
                {
                    Ok(Ok((meta, pos))) => {
                        let (copies, durable) = self.wait_durable(pos).await;
                        return Ok((meta, Self::durability(copies, durable)));
                    }
                    Ok(Err(e)) => return Err(e),
                    // The era moved between looking and writing: look again.
                    Err(b) => {
                        body = b;
                        continue;
                    }
                }
            }
            let t0 = Instant::now();
            match self.forward_put(&owner, key, &body, &cond).await {
                Ok((meta, pos, copies, durable)) => {
                    let t1 = t0.elapsed();
                    self.wait_applied(pos).await;
                    tracing::debug!(%key, ?t1, applied_wait = ?t0.elapsed() - t1, %pos, "replica: forwarded write");
                    return Ok((meta, Self::durability(copies as usize, durable)));
                }
                Err(Fwd::Remote(e)) if is_not_owner(&e) && attempt < 5 => {
                    tokio::time::sleep(self.cfg.timing.interval).await;
                    continue;
                }
                Err(Fwd::Remote(e)) => return Err(e),
                Err(Fwd::Unreachable(why)) => {
                    if matches!(cond, Condition::None) {
                        let meta = self.save_pending_put(key, body).await?;
                        return Ok((meta, Durability::Pending));
                    }
                    // An ordered write cannot wait in an outbox: this
                    // machine needs the authority, and takes it as soon as
                    // the owner is confirmed gone.
                    self.want_authority().await;
                    let name = self.owner_name(&owner).await;
                    let _ = why;
                    return Err(Error::Backend(unreachable_msg(&name)));
                }
            }
        }
        Err(Error::Backend(
            "the volume's owner keeps changing; try again".into(),
        ))
    }

    /// Records that this machine needs to order writes itself.
    pub(super) async fn want_authority(&self) {
        let mut st = self.st.lock().await;
        st.authority_wanted = Some(Instant::now());
        drop(st);
        self.tick_wake.notify_one();
    }

    pub(super) async fn owner_name(&self, id: &str) -> String {
        let st = self.st.lock().await;
        st.members
            .get(id)
            .map(|m| m.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string())
    }

    /// Waits (briefly) for our own copy to apply `pos`, so a read right
    /// after a forwarded write sees it.
    pub(super) async fn wait_applied(&self, pos: Pos) {
        if self.is_client.load(Ordering::SeqCst) {
            return;
        }
        let mut rx = self.applied_tx.subscribe();
        let deadline = Instant::now() + self.cfg.ack_wait * 2;
        while *rx.borrow() < pos {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            tokio::select! {
                r = rx.changed() => { if r.is_err() { return; } }
                _ = tokio::time::sleep(left) => return,
            }
        }
    }

    async fn forward_put(
        self: &Arc<Self>,
        owner: &str,
        key: &str,
        body: &Body,
        cond: &Condition,
    ) -> std::result::Result<(ObjectMeta, Pos, u32, bool), Fwd> {
        let size = body.size();
        if size > self.cfg.push_max as u64 {
            return self.forward_big(owner, key, body, cond).await;
        }
        let bytes = match body {
            Body::Bytes(b) => b.clone(),
            Body::File(p) => std::fs::read(p).map_err(|e| Fwd::Remote(e.into()))?,
        };
        let link = if bytes.len() <= self.cfg.inline_max {
            self.owner_ctl(owner).await
        } else {
            self.data_link(owner).await
        };
        let Some(link) = link else {
            return Err(Fwd::Unreachable("no link".into()));
        };
        let to = self.cfg.data_timeout;
        match link
            .call_strict(
                Msg::Req(Req::Put {
                    key: key.to_string(),
                    cond: Cond::from(cond.clone()),
                }),
                &bytes,
                to,
            )
            .await
        {
            Ok((
                Msg::Rep(Rep::Meta {
                    meta,
                    pos,
                    copies,
                    durable,
                }),
                _,
            )) => Ok((meta, pos, copies, durable)),
            Ok((Msg::Rep(Rep::Fail { kind, message }), _)) => {
                Err(Fwd::Remote(Rep::into_error(&kind, message)))
            }
            Ok(_) => Err(Fwd::Remote(Error::Backend("unexpected reply".into()))),
            Err(CallError::Closed | CallError::Timeout) => {
                Err(Fwd::Unreachable("no answer".into()))
            }
            Err(CallError::Io(e)) => Err(Fwd::Unreachable(e.to_string())),
        }
    }

    /// A large write by a non-owner: spool it, let the owner pull it in
    /// blocks (resumable), and wait for what it did.
    async fn forward_big(
        self: &Arc<Self>,
        owner: &str,
        key: &str,
        body: &Body,
        cond: &Condition,
    ) -> std::result::Result<(ObjectMeta, Pos, u32, bool), Fwd> {
        let Some(link) = self.owner_ctl(owner).await else {
            return Err(Fwd::Unreachable("no link".into()));
        };
        let meta = match body {
            Body::Bytes(b) => self.spool.put(key, b.clone(), Condition::None).await,
            Body::File(p) => self.spool.put_file(key, p, Condition::None).await,
        }
        .map_err(Fwd::Remote)?;
        let p = Pending {
            id: crate::new_id(),
            key: key.to_string(),
            meta: Some(meta.clone()),
            base: None,
            cond: Cond::from(cond.clone()),
            machine: self.me.clone(),
            name: self.name.clone(),
            ts_ms: self.wall_ms(),
            src: Src::Spool {
                key: key.to_string(),
                version: meta.version.clone(),
            },
            body: None,
        };
        let r = link
            .call(Msg::Submit { ops: vec![p] }, &[], self.cfg.data_timeout * 4)
            .await;
        let _ = self.spool.purge_version(key, &meta.version).await;
        match r {
            Ok((Msg::Submitted { outcomes }, _)) => match outcomes.into_iter().next() {
                Some(Outcome::Applied { meta, pos }) => Ok((meta, pos, 1, false)),
                Some(Outcome::Precondition { message }) => {
                    Err(Fwd::Remote(Error::Precondition(message)))
                }
                Some(Outcome::Failed { message }) => Err(Fwd::Remote(Error::Backend(message))),
                _ => Err(Fwd::Remote(Error::Backend("unexpected outcome".into()))),
            },
            Ok((Msg::Error { kind, message }, _)) => {
                Err(Fwd::Remote(Rep::into_error(&kind, message)))
            }
            Ok(_) => Err(Fwd::Remote(Error::Backend("unexpected reply".into()))),
            Err(_) => Err(Fwd::Unreachable("no answer".into())),
        }
    }

    /// Many unconditional writes: at the owner they are one batch (one disk
    /// barrier, one run of log records, one wait for the second copy).
    pub(super) async fn write_many(
        self: &Arc<Self>,
        items: Vec<(String, Vec<u8>)>,
    ) -> Result<Vec<ObjectMeta>> {
        let unique = items
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len()
            == items.len();
        if unique && !items.is_empty() {
            let mut st = self.st.lock().await;
            if st.eras.owner() == self.me {
                let mut prevs = Vec::with_capacity(items.len());
                for (k, _) in &items {
                    prevs.push(self.store.head(k).await?);
                }
                let keys: Vec<(&str, bool)> =
                    items.iter().map(|(k, _)| (k.as_str(), false)).collect();
                self.write_intent(&keys)?;
                let metas = match self.store.put_many(items).await {
                    Ok(m) => m,
                    Err(e) => {
                        self.clear_intent();
                        return Err(e);
                    }
                };
                let mut recs = Vec::with_capacity(metas.len());
                let mut pos = self.next_pos(&st);
                let at_ms = self.wall_ms();
                for (meta, prev) in metas.iter().zip(prevs) {
                    recs.push(Record {
                        pos,
                        op: Op::Put {
                            meta: meta.clone(),
                            prev,
                        },
                        at_ms,
                    });
                    pos.seq += 1;
                }
                let last = recs.last().map_or(Pos::ZERO, |r| r.pos);
                let now = Instant::now();
                for r in &recs {
                    st.written_at.insert(r.pos, now);
                }
                while st.written_at.len() > 4096 {
                    st.written_at.pop_first();
                }
                st.log.append_batch(recs)?;
                self.applied_tx.send_replace(last);
                self.clear_intent();
                drop(st);
                self.wake.notify_one();
                self.wait_durable(last).await;
                return Ok(metas);
            }
        }
        let mut out = Vec::with_capacity(items.len());
        for (k, b) in items {
            out.push(self.write_put(&k, Body::Bytes(b), Condition::None).await?.0);
        }
        Ok(out)
    }

    pub(super) async fn write_delete(
        self: &Arc<Self>,
        key: &str,
        cond: Condition,
    ) -> Result<Durability> {
        for attempt in 0..6u32 {
            let owner = self.st.lock().await.eras.owner().to_string();
            if owner == self.me {
                match self.owner_delete(key, cond.clone()).await {
                    Ok(pos) => {
                        let (copies, durable) = self.wait_durable(pos).await;
                        return Ok(Self::durability(copies, durable));
                    }
                    Err(e) if is_not_owner(&e) => continue,
                    Err(e) => return Err(e),
                }
            }
            let link = self.owner_ctl(&owner).await;
            let r = match link {
                Some(l) => {
                    l.call(
                        Msg::Req(Req::Delete {
                            key: key.to_string(),
                            cond: Cond::from(cond.clone()),
                        }),
                        &[],
                        self.cfg.call_timeout,
                    )
                    .await
                }
                None => Err(CallError::Closed),
            };
            match r {
                Ok((
                    Msg::Rep(Rep::Done {
                        pos,
                        copies,
                        durable,
                    }),
                    _,
                )) => {
                    self.wait_applied(pos).await;
                    return Ok(Self::durability(copies as usize, durable));
                }
                Ok((Msg::Rep(Rep::Fail { kind, message }), _)) => {
                    let e = Rep::into_error(&kind, message);
                    if is_not_owner(&e) && attempt < 5 {
                        tokio::time::sleep(self.cfg.timing.interval).await;
                        continue;
                    }
                    return Err(e);
                }
                Ok(_) => return Err(Error::Backend("unexpected reply".into())),
                Err(_) => {
                    if matches!(cond, Condition::None) {
                        self.save_pending_delete(key).await?;
                        return Ok(Durability::Pending);
                    }
                    self.want_authority().await;
                    let name = self.owner_name(&owner).await;
                    return Err(Error::Backend(unreachable_msg(&name)));
                }
            }
        }
        Err(Error::Backend(
            "the volume's owner keeps changing; try again".into(),
        ))
    }

    // ---- saved for later --------------------------------------------------

    async fn pending_base(&self, key: &str) -> Option<String> {
        {
            let st = self.st.lock().await;
            if let Some(p) = st.outbox.items().iter().rev().find(|p| p.key == key) {
                return p.meta.as_ref().map(|m| m.version.clone());
            }
        }
        if self.is_client.load(Ordering::SeqCst) {
            return None;
        }
        self.store.head(key).await.ok().flatten().map(|h| h.version)
    }

    pub(super) async fn save_pending_put(&self, key: &str, body: Body) -> Result<ObjectMeta> {
        let base = self.pending_base(key).await;
        let meta = match body {
            Body::Bytes(b) => self.spool.put(key, b, Condition::None).await?,
            Body::File(p) => self.spool.put_file(key, &p, Condition::None).await?,
        };
        let mut st = self.st.lock().await;
        st.outbox.add(Pending {
            id: crate::new_id(),
            key: key.to_string(),
            meta: Some(meta.clone()),
            base,
            cond: Cond::None,
            machine: self.me.clone(),
            name: self.name.clone(),
            ts_ms: self.wall_ms(),
            src: Src::Spool {
                key: key.to_string(),
                version: meta.version.clone(),
            },
            body: None,
        })?;
        if !self.is_client.load(Ordering::SeqCst) {
            let path = self.spool.version_file(key, &meta.version)?;
            self.store
                .apply_put(meta.clone(), ApplySource::File { path, link: true })
                .await?;
        }
        Ok(meta)
    }

    pub(super) async fn save_pending_delete(&self, key: &str) -> Result<()> {
        let base = self.pending_base(key).await;
        let mut st = self.st.lock().await;
        st.outbox.add(Pending {
            id: crate::new_id(),
            key: key.to_string(),
            meta: None,
            base,
            cond: Cond::None,
            machine: self.me.clone(),
            name: self.name.clone(),
            ts_ms: self.wall_ms(),
            src: Src::Data {
                key: key.to_string(),
                version: String::new(),
            },
            body: None,
        })?;
        if !self.is_client.load(Ordering::SeqCst) {
            self.store
                .apply_delete(key, &FsBackend::new_version())
                .await?;
        }
        Ok(())
    }

    /// Hands the outbox to the owner once this machine is caught up with it.
    pub(super) async fn submit_outbox(self: &Arc<Self>) {
        if self.drain_own_outbox().await {
            return;
        }
        let (owner, ops, caught_up) = {
            let st = self.st.lock().await;
            if st.outbox.is_empty() || st.eras.owner() == self.me {
                return;
            }
            let owner = st.eras.owner().to_string();
            let owner_tail = st.peers.get(&owner).map_or(Pos::ZERO, |p| p.tail);
            let client = self.is_client.load(Ordering::SeqCst);
            (
                owner,
                st.outbox
                    .items()
                    .iter()
                    .take(200)
                    .cloned()
                    .collect::<Vec<_>>(),
                client || st.log.tail() >= owner_tail,
            )
        };
        if !caught_up {
            return;
        }
        if self
            .submitting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let r = self.submit_ops(&owner, ops).await;
        self.submitting.store(false, Ordering::SeqCst);
        if let Err(e) = r {
            tracing::debug!(error = %e, "replica: the outbox waits");
        }
    }

    /// This machine owns the volume and holds changes it saved while it
    /// did not (it stepped down, then got the volume back): they are applied
    /// here, in order, by the same rules the owner applies anyone's. Returns
    /// whether this machine is the owner (nothing is sent to anyone).
    async fn drain_own_outbox(self: &Arc<Self>) -> bool {
        if self.st.lock().await.eras.owner() != self.me {
            return false;
        }
        let ops: Vec<Pending> = {
            let st = self.st.lock().await;
            st.outbox.items().iter().take(200).cloned().collect()
        };
        if ops.is_empty() {
            return true;
        }
        if self
            .submitting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return true;
        }
        let mut remap: HashMap<String, String> = HashMap::new();
        for op in &ops {
            let o = self.apply_pending(None, op, &[], &mut remap).await;
            tracing::debug!(key = %op.key, detail = ?o, "replica: applied own saved change");
            if let Outcome::Failed { message } = &o {
                tracing::warn!(key = %op.key, %message, "replica: could not apply a saved change here yet");
                continue;
            }
            let mut st = self.st.lock().await;
            let _ = st.outbox.remove(&op.id);
            if let (Outcome::Applied { meta: new, .. }, Some(old)) = (&o, &op.meta) {
                let _ = st.outbox.rebase(&old.version, &new.version);
            }
            drop(st);
            if let (Some(_), Src::Spool { key, version }) = (&op.meta, &op.src) {
                let _ = self.spool.purge_version(key, version).await;
            }
        }
        self.submitting.store(false, Ordering::SeqCst);
        self.wake.notify_one();
        true
    }

    async fn submit_ops(self: &Arc<Self>, owner: &str, mut ops: Vec<Pending>) -> Result<()> {
        let Some(link) = self.ctl_link(owner) else {
            return Err(Error::Backend("owner not connected".into()));
        };
        // Small bodies travel in the frame; large ones are pulled.
        let mut data: Vec<u8> = vec![];
        for op in &mut ops {
            op.body = None;
            if let Some(m) = &op.meta
                && m.size <= self.cfg.inline_max as u64
                && data.len() + (m.size as usize) < (1 << 20)
            {
                let r = match &op.src {
                    Src::Spool { key, version } => self.spool.get(key, Some(version)).await,
                    Src::Data { key, version } => self.store.get(key, Some(version)).await,
                };
                if let Ok((bytes, _)) = r {
                    op.body = Some((data.len() as u64, bytes.len() as u64));
                    data.extend_from_slice(&bytes);
                }
            }
        }
        let n = ops.len();
        let to = self.cfg.data_timeout * 4;
        let (reply, _) = link
            .call(Msg::Submit { ops: ops.clone() }, &data, to)
            .await
            .map_err(|e| Error::Backend(e.to_string()))?;
        let outcomes = match reply {
            Msg::Submitted { outcomes } => outcomes,
            Msg::Error { kind, message } => return Err(Rep::into_error(&kind, message)),
            _ => return Err(Error::Backend("unexpected reply".into())),
        };
        if outcomes.len() != n {
            return Err(Error::Backend("short answer".into()));
        }
        let mut done = vec![];
        let mut st = self.st.lock().await;
        for (op, o) in ops.iter().zip(outcomes) {
            tracing::debug!(key = %op.key, outcome = ?std::mem::discriminant(&o), detail = ?o, "replica: owner took a change");
            if let Outcome::Failed { message } = &o {
                tracing::warn!(key = %op.key, %message, "replica: the owner could not take a change yet");
                continue;
            }
            st.outbox.remove(&op.id)?;
            if let (Outcome::Applied { meta: new, .. }, Some(old)) = (&o, &op.meta) {
                st.outbox.rebase(&old.version, &new.version)?;
            }
            done.push(op.clone());
        }
        drop(st);
        for op in done {
            if let (Some(m), Src::Spool { key, version }) = (&op.meta, &op.src) {
                let _ = m;
                let _ = self.spool.purge_version(key, version).await;
            }
        }
        self.wake.notify_one();
        Ok(())
    }
}
