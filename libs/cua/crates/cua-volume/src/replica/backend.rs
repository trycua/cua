// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The `Backend` the drive sees: one volume across your machines.
//!
//! Machines that hold a copy answer reads from it; a machine that does not
//! (a client) asks one that does, and the block cache sits in front of it.
//! Writes go to the owner, which orders them, logs them and ships them;
//! unconditional writes made while the owner is away are saved and sent
//! when it is back.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::node::Inner;
use super::owner::Body;
use super::roles::Role;
use super::status::Durability;
use super::wire::{Msg, Rep, Req};
use crate::backend::{Backend, Condition, ObjectMeta, VersionInfo};
use crate::{Error, Result};

pub struct ReplicatedBackend {
    inner: Arc<Inner>,
}

impl ReplicatedBackend {
    pub(super) fn new(inner: Arc<Inner>) -> ReplicatedBackend {
        ReplicatedBackend { inner }
    }

    /// Writes and says how many machines hold the result.
    pub async fn put_durable(
        &self,
        key: &str,
        bytes: Vec<u8>,
        cond: Condition,
    ) -> Result<(ObjectMeta, Durability)> {
        self.inner.write_put(key, Body::Bytes(bytes), cond).await
    }

    /// The machines to ask for a read, best first: the owner, then the
    /// other machines that hold a copy.
    async fn read_targets(&self) -> Vec<String> {
        let st = self.inner.st.lock().await;
        let layout = self.inner.layout_locked(&st);
        let owner = st.eras.owner().to_string();
        let mut v = vec![];
        if owner != self.inner.me {
            v.push(owner.clone());
        }
        for r in layout.replicas {
            if r != self.inner.me && r != owner {
                v.push(r);
            }
        }
        v
    }

    async fn ask(&self, req: Req, big: bool) -> Result<(Rep, Vec<u8>)> {
        let targets = self.read_targets().await;
        let mut last = Error::Backend("no machine that holds the volume can be reached".into());
        for t in targets {
            let link = if big {
                self.inner.data_link(&t).await
            } else {
                self.inner.ctl_link(&t)
            };
            let Some(link) = link else { continue };
            let to = if big {
                self.inner.cfg.data_timeout
            } else {
                self.inner.cfg.call_timeout
            };
            let r = if big {
                link.call_strict(Msg::Req(req.clone()), &[], to).await
            } else {
                link.call(Msg::Req(req.clone()), &[], to).await
            };
            match r {
                Ok((Msg::Rep(Rep::Fail { kind, message }), _)) => {
                    return Err(Rep::into_error(&kind, message));
                }
                Ok((Msg::Rep(r), d)) => return Ok((r, d)),
                Ok(_) => last = Error::Backend("unexpected reply".into()),
                Err(e) => last = Error::Backend(e.to_string()),
            }
        }
        Err(last)
    }

    fn holds_copy(&self) -> bool {
        !self.inner.is_client.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Backend for ReplicatedBackend {
    fn kind(&self) -> &'static str {
        "mesh"
    }

    async fn put(&self, key: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta> {
        Ok(self.inner.write_put(key, Body::Bytes(bytes), cond).await?.0)
    }

    async fn put_file(&self, key: &str, file: &Path, cond: Condition) -> Result<ObjectMeta> {
        Ok(self
            .inner
            .write_put(key, Body::File(file.to_path_buf()), cond)
            .await?
            .0)
    }

    async fn put_many(&self, items: Vec<(String, Vec<u8>)>) -> Result<Vec<ObjectMeta>> {
        self.inner.write_many(items).await
    }

    async fn delete(&self, key: &str, cond: Condition) -> Result<()> {
        self.inner.write_delete(key, cond).await.map(|_| ())
    }

    async fn get(&self, key: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        if self.holds_copy() {
            match self.inner.store.get(key, version).await {
                Err(Error::NotFound(_)) if version.is_some() => {}
                r => return r,
            }
        }
        match self
            .ask(
                Req::Get {
                    key: key.to_string(),
                    version: version.map(str::to_string),
                },
                true,
            )
            .await?
        {
            (Rep::Bytes { meta }, bytes) => Ok((bytes, meta)),
            _ => Err(Error::Backend("unexpected reply".into())),
        }
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        if self.holds_copy() {
            return self.inner.store.head(key).await;
        }
        match self
            .ask(
                Req::Head {
                    key: key.to_string(),
                },
                false,
            )
            .await?
        {
            (Rep::Head { meta }, _) => Ok(meta),
            _ => Err(Error::Backend("unexpected reply".into())),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        if self.holds_copy() {
            return self.inner.store.list(prefix).await;
        }
        match self
            .ask(
                Req::List {
                    prefix: prefix.to_string(),
                },
                true,
            )
            .await?
        {
            (Rep::List { items }, _) => Ok(items),
            _ => Err(Error::Backend("unexpected reply".into())),
        }
    }

    async fn list_dir(&self, folder: &str) -> Result<(Vec<ObjectMeta>, Vec<String>)> {
        if self.holds_copy() {
            return self.inner.store.list_dir(folder).await;
        }
        match self
            .ask(
                Req::ListDir {
                    folder: folder.to_string(),
                },
                false,
            )
            .await?
        {
            (Rep::ListDir { files, folders }, _) => Ok((files, folders)),
            _ => Err(Error::Backend("unexpected reply".into())),
        }
    }

    async fn versions(&self, key: &str) -> Result<Vec<VersionInfo>> {
        if self.holds_copy() {
            return self.inner.store.versions(key).await;
        }
        match self
            .ask(
                Req::Versions {
                    key: key.to_string(),
                },
                false,
            )
            .await?
        {
            (Rep::Versions { items }, _) => Ok(items),
            _ => Err(Error::Backend("unexpected reply".into())),
        }
    }

    async fn get_range(&self, key: &str, version: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if self.holds_copy() {
            match self.inner.store.get_range(key, version, offset, len).await {
                Err(Error::NotFound(_)) => {}
                r => return r,
            }
        }
        match self
            .ask(
                Req::GetRange {
                    key: key.to_string(),
                    version: version.to_string(),
                    offset,
                    len,
                },
                true,
            )
            .await?
        {
            (Rep::Range, bytes) => Ok(bytes),
            _ => Err(Error::Backend("unexpected reply".into())),
        }
    }

    fn remote(&self) -> bool {
        !self.holds_copy()
    }

    fn identity(&self) -> String {
        format!("mesh:{}", self.inner.store.root().display())
    }
}

#[allow(dead_code)]
fn _role(_: Role) {}
