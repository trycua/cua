// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable Spaces: a long-lived Space the cluster keeps running, addressed
//! by a stable name instead of a machine.
//!
//! - **Placement.** The coordinator creates it on the best host that
//!   reaches its Spaces directly (as `place_space` does), and keeps it
//!   there.
//! - **Ownership.** One client (an agent, a person) holds it at a time
//!   under a lease: `acquire` returns a fencing token (it only grows) and
//!   the Space's address and token; `renew` extends the lease; `release`
//!   ends it. A lease that runs out can be acquired by someone else with a
//!   higher token, so the previous holder's token stops working here, and
//!   it can present the token to anything it writes to (the fencing
//!   pattern) so a stale holder's writes are refused there too.
//! - **Rescheduling.** When its host is down (no answer for
//!   `node_down_after`), or its Space is gone from a host that answers, the
//!   coordinator creates it again on another host: the next generation,
//!   `restored: "from-scratch"` (a fresh Space of the same image; restoring
//!   from a checkpoint needs a host API to checkpoint a Space, which is not
//!   there yet). The lease holder finds the new address with its token.
//!   The copy on the host that went down is deleted when that host answers
//!   again.
//!
//! State lives in the coordinator's database (`durable` table), so a
//! coordinator restart changes nothing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{CoordError, Coordinator, Health, SpaceRequest, sanitize_name};
use cua_cluster::link::{LinkError, NodeLink};
use cua_cluster::model::now_ms;

/// What a durable Space runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableSpec {
    /// Image (empty: the canonical Linux image).
    #[serde(default)]
    pub image: String,
    /// `container`, `vm`, `microvm` or empty.
    #[serde(default)]
    pub kind: String,
    /// A runtime or empty.
    #[serde(default)]
    pub runtime: String,
    /// vCPUs (0: 2).
    #[serde(default)]
    pub cpus: u32,
    /// Memory in MiB (0: 4096).
    #[serde(default)]
    pub memory_mb: u64,
    /// No desktop (`SpaceCreate::headless`): kept by every copy.
    #[serde(default)]
    pub headless: bool,
}

/// `pending` (no host has it yet), `running`, or `deleted` (its copies
/// are being removed).
pub mod state {
    /// Waiting for a host.
    pub const PENDING: &str = "pending";
    /// On a host.
    pub const RUNNING: &str = "running";
    /// Deleted; copies on hosts that were down are removed when they
    /// answer.
    pub const DELETED: &str = "deleted";
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Row {
    name: String,
    spec: DurableSpec,
    state: String,
    node: String,
    local_space: String,
    id: String,
    token: String,
    kind: String,
    generation: u32,
    restored: String,
    holder: String,
    lease_token: u64,
    lease_expires_ms: i64,
    fence: u64,
    error: String,
    created_ms: i64,
    updated_ms: i64,
    /// Copies to delete when their host answers: (host, `local:<name>`).
    stale: Vec<(String, String)>,
}

impl Row {
    /// What deletes this copy on its host: its relay machine (`relay:<id>`:
    /// the Space and its relay registration), or the Space on the host.
    fn copy_ref(&self) -> String {
        if self.id.starts_with("relay:") {
            self.id.clone()
        } else {
            self.local_space.clone()
        }
    }

    fn leased(&self, now: i64) -> bool {
        !self.holder.is_empty() && self.lease_expires_ms > now
    }

    fn view(&self) -> DurableView {
        let now = now_ms();
        DurableView {
            name: self.name.clone(),
            spec: self.spec.clone(),
            state: self.state.clone(),
            node: self.node.clone(),
            kind: self.kind.clone(),
            generation: self.generation,
            restored: self.restored.clone(),
            holder: self.leased(now).then(|| self.holder.clone()),
            lease_expires_in_secs: self
                .leased(now)
                .then(|| (self.lease_expires_ms - now) / 1000),
            error: self.error.clone(),
            created_ms: self.created_ms,
            updated_ms: self.updated_ms,
            space: self.id.clone(),
            token: self.token.clone(),
        }
    }

    fn connection(&self) -> DurableConnection {
        DurableConnection {
            name: self.name.clone(),
            state: self.state.clone(),
            lease_token: self.lease_token,
            lease_expires_in_secs: ((self.lease_expires_ms - now_ms()) / 1000).max(0),
            generation: self.generation,
            restored: self.restored.clone(),
            node: self.node.clone(),
            id: self.id.clone(),
            token: self.token.clone(),
        }
    }
}

/// A durable Space as anyone with the admin token sees it (no secrets).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableView {
    /// Its stable name.
    pub name: String,
    /// What it runs.
    pub spec: DurableSpec,
    /// `pending`, `running` or `deleted`.
    pub state: String,
    /// The host it is on.
    pub node: String,
    /// Its kind.
    pub kind: String,
    /// One more each time it is created again (1: the first copy).
    pub generation: u32,
    /// `""` (the first copy) or `from-scratch` (created again on another
    /// host without its state).
    pub restored: String,
    /// Who holds its lease, while one does.
    pub holder: Option<String>,
    /// Seconds left on that lease.
    pub lease_expires_in_secs: Option<i64>,
    /// Why it is pending.
    pub error: String,
    /// Created (Unix ms).
    pub created_ms: i64,
    /// Last changed.
    pub updated_ms: i64,
    /// The Space it is now: `relay:<machine>` (on a machine the account
    /// reaches through the relay) or `direct:<host>:<port>` (on one added
    /// by address); empty while it waits for a machine.
    #[serde(default)]
    pub space: String,
    /// The token of a `direct:` Space (empty for a relay one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
}

/// What the lease holder gets: the fencing token and how to reach the
/// Space now.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableConnection {
    /// Its name.
    pub name: String,
    /// `pending` (no address yet) or `running`.
    pub state: String,
    /// The fencing token: present it to renew and release, and to anything
    /// the holder writes to.
    pub lease_token: u64,
    /// Seconds left on the lease.
    pub lease_expires_in_secs: i64,
    /// Which copy this is.
    pub generation: u32,
    /// `""` or `from-scratch`.
    pub restored: String,
    /// The host.
    pub node: String,
    /// `direct:<host address>:<port>`: add it with `cua spaces add` (or
    /// the SDK) and the token below.
    pub id: String,
    /// The Space's cua-spacesd token.
    pub token: String,
}

pub(crate) fn init(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS durable(
           name TEXT PRIMARY KEY, spec TEXT NOT NULL, state TEXT NOT NULL,
           node TEXT NOT NULL DEFAULT '', local_space TEXT NOT NULL DEFAULT '',
           id TEXT NOT NULL DEFAULT '', token TEXT NOT NULL DEFAULT '',
           kind TEXT NOT NULL DEFAULT '', generation INTEGER NOT NULL DEFAULT 0,
           restored TEXT NOT NULL DEFAULT '', holder TEXT NOT NULL DEFAULT '',
           lease_token INTEGER NOT NULL DEFAULT 0, lease_expires_ms INTEGER NOT NULL DEFAULT 0,
           fence INTEGER NOT NULL DEFAULT 0, error TEXT NOT NULL DEFAULT '',
           created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
           stale TEXT NOT NULL DEFAULT '[]');",
    )
}

const COLS: &str = "name,spec,state,node,local_space,id,token,kind,generation,restored,holder,lease_token,lease_expires_ms,fence,error,created_ms,updated_ms,stale";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let spec: String = r.get(1)?;
    let stale: String = r.get(17)?;
    Ok(Row {
        name: r.get(0)?,
        spec: serde_json::from_str(&spec).unwrap_or_default(),
        state: r.get(2)?,
        node: r.get(3)?,
        local_space: r.get(4)?,
        id: r.get(5)?,
        token: r.get(6)?,
        kind: r.get(7)?,
        generation: r.get::<_, i64>(8)? as u32,
        restored: r.get(9)?,
        holder: r.get(10)?,
        lease_token: r.get::<_, i64>(11)? as u64,
        lease_expires_ms: r.get(12)?,
        fence: r.get::<_, i64>(13)? as u64,
        error: r.get(14)?,
        created_ms: r.get(15)?,
        updated_ms: r.get(16)?,
        stale: serde_json::from_str(&stale).unwrap_or_default(),
    })
}

fn load(c: &Connection, name: &str) -> Result<Option<Row>, String> {
    c.query_row(
        &format!("SELECT {COLS} FROM durable WHERE name=?1"),
        [name],
        from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn list(c: &Connection) -> Result<Vec<Row>, String> {
    let mut st = c
        .prepare(&format!("SELECT {COLS} FROM durable ORDER BY name"))
        .map_err(|e| e.to_string())?;
    st.query_map([], from_row)
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
}

fn save(c: &Connection, r: &Row) -> Result<(), String> {
    c.execute(
        &format!("INSERT OR REPLACE INTO durable({COLS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)"),
        params![
            r.name,
            serde_json::to_string(&r.spec).unwrap_or_default(),
            r.state,
            r.node,
            r.local_space,
            r.id,
            r.token,
            r.kind,
            r.generation as i64,
            r.restored,
            r.holder,
            r.lease_token as i64,
            r.lease_expires_ms,
            r.fence as i64,
            r.error,
            r.created_ms,
            r.updated_ms,
            serde_json::to_string(&r.stale).unwrap_or_else(|_| "[]".into()),
        ],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// [`store::Op::DurableSave`](super::store::Op::DurableSave): writes the
/// row; whether it changed anything.
pub(crate) fn save_json(c: &Connection, row: &serde_json::Value) -> Result<bool, String> {
    let r: Row = serde_json::from_value(row.clone()).map_err(|e| format!("durable row: {e}"))?;
    let before = load(c, &r.name)?;
    save(c, &r)?;
    Ok(before.as_ref() != Some(&r))
}

/// [`store::Op::DurableRemove`](super::store::Op::DurableRemove).
pub(crate) fn remove_named(c: &Connection, name: &str) -> Result<bool, String> {
    c.execute("DELETE FROM durable WHERE name=?1", [name])
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
}

/// How often a running durable Space's host is asked whether it still has
/// it.
const CHECK_EVERY: Duration = Duration::from_secs(15);

impl Coordinator {
    async fn durable_lock(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let m = {
            let mut g = self.durable_locks.lock().expect("durable locks");
            g.entry(name.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        m.lock_owned().await
    }

    async fn durable_load(&self, name: &str) -> Result<Option<Row>, CoordError> {
        let n = name.to_string();
        self.store
            .call(move |c| load(c, &n))
            .await
            .map_err(CoordError::Store)
    }

    async fn durable_save(&self, mut r: Row) -> Result<Row, CoordError> {
        r.updated_ms = now_ms();
        let row = serde_json::to_value(&r).map_err(|e| CoordError::Store(e.to_string()))?;
        self.persist_ops(vec![super::store::Op::DurableSave { row }])
            .await
            .map_err(CoordError::Store)?;
        Ok(r)
    }

    async fn durable_remove(&self, name: &str) -> Result<(), CoordError> {
        self.persist_ops(vec![super::store::Op::DurableRemove {
            name: name.to_string(),
        }])
        .await
        .map(|_| ())
        .map_err(CoordError::Store)
    }

    fn node_way(&self, name: &str) -> Option<(Health, Arc<dyn NodeLink>)> {
        let g = self.sched.lock().expect("sched");
        g.nodes.get(name).map(|n| (n.health, n.link.clone()))
    }

    /// Creates (or returns) the durable Space `name` and places it.
    pub async fn durable_create(
        &self,
        name: &str,
        spec: DurableSpec,
    ) -> Result<DurableView, CoordError> {
        self.durable_create_with(name, spec, true).await
    }

    /// Creates (or returns) the durable Space `name`, `pending` until the
    /// durable loop places it (within seconds).
    pub async fn durable_create_later(
        &self,
        name: &str,
        spec: DurableSpec,
    ) -> Result<DurableView, CoordError> {
        self.durable_create_with(name, spec, false).await
    }

    async fn durable_create_with(
        &self,
        name: &str,
        spec: DurableSpec,
        place: bool,
    ) -> Result<DurableView, CoordError> {
        let clean = sanitize_name(name);
        if clean != name.trim() || clean.len() > 48 {
            return Err(CoordError::Invalid(format!(
                "a durable Space is named with lower-case letters, digits, `-` and `.` (48 at \
                 most), like {clean:?}"
            )));
        }
        let _g = self.durable_lock(&clean).await;
        if let Some(r) = self.durable_load(&clean).await?
            && r.state != state::DELETED
        {
            if r.spec != spec {
                return Err(CoordError::Invalid(format!(
                    "{clean} already exists with another image or size; delete it first"
                )));
            }
            return Ok(r.view());
        }
        let now = now_ms();
        let row = Row {
            name: clean.clone(),
            spec,
            state: state::PENDING.into(),
            created_ms: now,
            ..Default::default()
        };
        let row = self.durable_save(row).await?;
        if !place {
            return Ok(row.view());
        }
        let row = self.durable_place(row, false).await?;
        Ok(row.view())
    }

    /// Places `row` on a host (another one than its current when
    /// `elsewhere`), as the next generation.
    async fn durable_place(&self, mut row: Row, elsewhere: bool) -> Result<Row, CoordError> {
        let exclude: Vec<String> = if elsewhere && !row.node.is_empty() {
            vec![row.node.clone()]
        } else {
            vec![]
        };
        let req = SpaceRequest {
            image: row.spec.image.clone(),
            kind: row.spec.kind.clone(),
            runtime: row.spec.runtime.clone(),
            // Its own name on the host (a copy an earlier try left there,
            // or the one a machine that comes back still has, never
            // collides); clients show and find it by the durable name.
            name: format!("{}-{:04x}", row.name, rand::random::<u16>()),
            cpus: row.spec.cpus,
            memory_mb: row.spec.memory_mb,
            headless: row.spec.headless,
        };
        match self.place_space_excluding(req, &exclude).await {
            Ok(p) => {
                let from = row.node.clone();
                row.node = p.node.clone();
                row.local_space = p.local_space;
                row.id = p.id;
                row.token = p.token;
                row.kind = p.kind;
                row.generation += 1;
                row.restored = if row.generation > 1 {
                    "from-scratch".into()
                } else {
                    String::new()
                };
                row.state = state::RUNNING.into();
                row.error.clear();
                let detail = if row.generation > 1 {
                    format!(
                        "generation {} on {} (was on {}), restored from scratch",
                        row.generation,
                        row.node,
                        if from.is_empty() { "-" } else { &from }
                    )
                } else {
                    format!("on {}", row.node)
                };
                self.audit("durable", "coordinator", &row.name, &detail)
                    .await;
                self.durable_save(row).await
            }
            Err(e) => {
                // Recorded when it changes, not on every try.
                let error = e.to_string();
                if row.state == state::PENDING && row.error == error {
                    return Ok(row);
                }
                row.state = state::PENDING.into();
                row.error = error;
                self.durable_save(row).await
            }
        }
    }

    /// Every durable Space.
    pub async fn durable_list(&self) -> Result<Vec<DurableView>, CoordError> {
        let rows = self
            .store
            .call(|c| list(c))
            .await
            .map_err(CoordError::Store)?;
        Ok(rows
            .into_iter()
            .filter(|r| r.state != state::DELETED)
            .map(|r| r.view())
            .collect())
    }

    /// One durable Space.
    pub async fn durable_get(&self, name: &str) -> Result<DurableView, CoordError> {
        self.durable_load(name)
            .await?
            .filter(|r| r.state != state::DELETED)
            .map(|r| r.view())
            .ok_or_else(|| CoordError::NotFound(format!("no durable Space {name}")))
    }

    /// Asks the Space's host now whether it still has `name`, and places the
    /// Space again if the host answers and does not list it. The durable loop
    /// does this every [`CHECK_EVERY`]; a session calls it before each drive,
    /// so a driver is not started against an address that is already known to
    /// be dead (it would fail at once and use up an attempt).
    pub(crate) async fn durable_recheck(&self, name: &str) {
        let _g = self.durable_lock(name).await;
        let Ok(Some(row)) = self.durable_load(name).await else {
            return;
        };
        if row.state != state::RUNNING {
            return;
        }
        if let Some((Health::Up, link)) = self.node_way(&row.node)
            && let Ok(host) = link.host().await
            && !host.spaces.iter().any(|s| s.local_space == row.local_space)
        {
            tracing::warn!(space = %name, node = %row.node, "durable Space is gone from its host: creating it again");
            let _ = self.durable_place(row, false).await;
        }
    }

    /// Takes the lease of `name` for `holder` (or extends `holder`'s own):
    /// a new fencing token and how to reach the Space.
    pub async fn durable_acquire(
        &self,
        name: &str,
        holder: &str,
        ttl: Duration,
    ) -> Result<DurableConnection, CoordError> {
        if holder.trim().is_empty() {
            return Err(CoordError::Invalid("a lease needs a holder".into()));
        }
        let _g = self.durable_lock(name).await;
        let mut r = self
            .durable_load(name)
            .await?
            .filter(|r| r.state != state::DELETED)
            .ok_or_else(|| CoordError::NotFound(format!("no durable Space {name}")))?;
        let now = now_ms();
        if r.leased(now) && r.holder != holder.trim() {
            return Err(CoordError::Denied(format!(
                "{name} is held by {} for {} s more",
                r.holder,
                (r.lease_expires_ms - now) / 1000
            )));
        }
        r.fence += 1;
        r.lease_token = r.fence;
        r.holder = holder.trim().to_string();
        r.lease_expires_ms = now + ttl.as_millis() as i64;
        let r = self.durable_save(r).await?;
        self.audit(
            "lease",
            &r.holder,
            name,
            &format!("token {} for {} s", r.lease_token, ttl.as_secs()),
        )
        .await;
        Ok(r.connection())
    }

    async fn durable_with_lease(
        &self,
        name: &str,
        token: u64,
    ) -> Result<(tokio::sync::OwnedMutexGuard<()>, Row), CoordError> {
        let g = self.durable_lock(name).await;
        let r = self
            .durable_load(name)
            .await?
            .filter(|r| r.state != state::DELETED)
            .ok_or_else(|| CoordError::NotFound(format!("no durable Space {name}")))?;
        if r.lease_token != token || !r.leased(now_ms()) {
            return Err(CoordError::Denied(format!(
                "the lease of {name} with token {token} is no longer held (it ran out or was taken); acquire it again"
            )));
        }
        Ok((g, r))
    }

    /// Extends the lease `token` of `name`.
    pub async fn durable_renew(
        &self,
        name: &str,
        token: u64,
        ttl: Duration,
    ) -> Result<DurableConnection, CoordError> {
        let (_g, mut r) = self.durable_with_lease(name, token).await?;
        r.lease_expires_ms = now_ms() + ttl.as_millis() as i64;
        Ok(self.durable_save(r).await?.connection())
    }

    /// How the holder of lease `token` reaches `name` now.
    pub async fn durable_connection(
        &self,
        name: &str,
        token: u64,
    ) -> Result<DurableConnection, CoordError> {
        let (_g, r) = self.durable_with_lease(name, token).await?;
        Ok(r.connection())
    }

    /// Ends the lease `token` of `name`.
    pub async fn durable_release(&self, name: &str, token: u64) -> Result<(), CoordError> {
        let (_g, mut r) = self.durable_with_lease(name, token).await?;
        let holder = std::mem::take(&mut r.holder);
        r.lease_expires_ms = 0;
        self.durable_save(r).await?;
        self.audit("release", &holder, name, &format!("token {token}"))
            .await;
        Ok(())
    }

    /// Deletes `name`: its Space now, or (its host is down) when its host
    /// answers again.
    pub async fn durable_delete(&self, name: &str) -> Result<(), CoordError> {
        let _g = self.durable_lock(name).await;
        let mut r = self
            .durable_load(name)
            .await?
            .ok_or_else(|| CoordError::NotFound(format!("no durable Space {name}")))?;
        if !r.local_space.is_empty() {
            r.stale.push((r.node.clone(), r.copy_ref()));
        }
        r.state = state::DELETED.into();
        r.holder.clear();
        r.lease_expires_ms = 0;
        let r = self.durable_clean(r).await;
        if r.stale.is_empty() {
            self.durable_remove(name).await?;
        } else {
            self.durable_save(r).await?;
        }
        Ok(())
    }

    /// Deletes the copies left on hosts that answer now (not tried on a
    /// host known to be down; one not heard from yet is tried).
    async fn durable_clean(&self, mut r: Row) -> Row {
        let mut left = Vec::new();
        for (node, space) in std::mem::take(&mut r.stale) {
            match self.node_way(&node) {
                Some((Health::Up | Health::Unknown, link)) => match link.delete_space(&space).await
                {
                    Ok(_) | Err(LinkError::NotFound(_)) => {}
                    Err(e) => {
                        tracing::debug!(%node, %space, "durable copy not deleted yet: {e}");
                        left.push((node, space));
                    }
                },
                // A removed host: nothing to reach.
                None => {}
                Some(_) => left.push((node, space)),
            }
        }
        r.stale = left;
        r
    }

    /// Keeps every durable Space placed: places pending ones, moves the
    /// ones whose host is down, recreates one its host lost, and deletes
    /// leftover copies.
    pub(crate) async fn durable_loop(self: Arc<Self>) {
        let mut checked: HashMap<String, i64> = HashMap::new();
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            let rows = match self.store.call(|c| list(c)).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("durable Spaces: {e}");
                    continue;
                }
            };
            for row in rows {
                let name = row.name.clone();
                let _g = self.durable_lock(&name).await;
                // Fresh under the lock.
                let Ok(Some(row)) = self.durable_load(&name).await else {
                    continue;
                };
                let row = if row.stale.is_empty() {
                    row
                } else {
                    let before = row.stale.len();
                    let r = self.durable_clean(row).await;
                    if r.stale.len() != before {
                        match self.durable_save(r).await {
                            Ok(r) => r,
                            Err(_) => continue,
                        }
                    } else {
                        r
                    }
                };
                match row.state.as_str() {
                    state::DELETED => {
                        if row.stale.is_empty() {
                            let _ = self.durable_remove(&name).await;
                        }
                    }
                    state::PENDING => {
                        let elsewhere = row.generation > 0;
                        let _ = self.durable_place(row, elsewhere).await;
                    }
                    _ => {
                        let way = self.node_way(&row.node);
                        let gone = matches!(&way, None | Some((Health::Down, _)));
                        if gone {
                            tracing::warn!(space = %name, node = %row.node, "durable Space's host is down: creating it again elsewhere");
                            let mut r = row;
                            r.stale.push((r.node.clone(), r.copy_ref()));
                            let _ = self.durable_place(r, true).await;
                            continue;
                        }
                        let now = now_ms();
                        let last = checked.get(&name).copied().unwrap_or(0);
                        if now - last < CHECK_EVERY.as_millis() as i64 {
                            continue;
                        }
                        checked.insert(name.clone(), now);
                        if let Some((Health::Up, link)) = way
                            && let Ok(host) = link.host().await
                            && !host.spaces.iter().any(|s| s.local_space == row.local_space)
                        {
                            tracing::warn!(space = %name, node = %row.node, "durable Space is gone from its host: creating it again");
                            let _ = self.durable_place(row, false).await;
                        }
                    }
                }
            }
        }
    }
}
