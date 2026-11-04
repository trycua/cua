// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The object store under the drive: S3-shaped, versioned, with
//! conditional writes. The local backend ([`crate::fs::FsBackend`]) and the
//! S3-compatible one implement the same trait, so everything above them
//! (access checks, sync, leases, audit) is backend-agnostic.

use serde::{Deserialize, Serialize};

use crate::Result;

/// A write or delete precondition. `etag` values are opaque: compare only
/// against an etag the same backend returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Condition {
    #[default]
    None,
    /// Only if nothing is at the key (create-only; a work-queue claim).
    IfNoneMatch,
    /// Only if the current object has this etag.
    IfMatch(String),
}

/// One object's current state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub key: String,
    pub size: u64,
    /// Opaque content tag used by preconditions.
    pub etag: String,
    /// The backend's version id of this content.
    pub version: String,
    /// Unix ms of the write.
    pub modified_ms: u64,
}

/// One entry in an object's history (newest first).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub size: u64,
    pub modified_ms: u64,
    /// A delete marker.
    pub deleted: bool,
    /// The current version.
    pub latest: bool,
}

/// A versioned object store.
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    /// `fs`, `s3`.
    fn kind(&self) -> &'static str;
    /// Writes a new version of `key`.
    async fn put(&self, key: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta>;
    /// Reads the current version (or `version`).
    async fn get(&self, key: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)>;
    /// The current version's metadata, or `None`.
    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>>;
    /// Every current object under `prefix` (recursive), sorted by key.
    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>>;
    /// Deletes `key` (a delete marker: history is kept).
    async fn delete(&self, key: &str, cond: Condition) -> Result<()>;
    /// The history of `key`, newest first.
    async fn versions(&self, key: &str) -> Result<Vec<VersionInfo>>;
    /// Bytes `[offset, offset + len)` of `key` at `version` (a version this
    /// backend returned; readers pin one so a concurrent write never mixes
    /// two contents). Short at the end of the object. The default reads the
    /// whole version; real backends read only the range.
    async fn get_range(&self, key: &str, version: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let (bytes, _) = self.get(key, Some(version)).await?;
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(len as usize).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }
    /// Writes a new version of `key` from a local file, without holding it
    /// in memory where the backend can (S3: a multipart upload).
    async fn put_file(
        &self,
        key: &str,
        file: &std::path::Path,
        cond: Condition,
    ) -> Result<ObjectMeta> {
        let bytes = tokio::fs::read(file).await?;
        self.put(key, bytes, cond).await
    }
    /// One level of `folder` (`""` or ending in `/`): the objects directly
    /// in it and the names of its sub-folders (each ending in `/`), sorted.
    async fn list_dir(&self, folder: &str) -> Result<(Vec<ObjectMeta>, Vec<String>)> {
        let mut files = vec![];
        let mut folders = std::collections::BTreeSet::new();
        for m in self.list(folder).await? {
            let rest = &m.key[folder.len()..];
            match rest.find('/') {
                Some(i) => {
                    folders.insert(format!("{folder}{}/", &rest[..i]));
                }
                None => files.push(m),
            }
        }
        Ok((files, folders.into_iter().collect()))
    }
    /// Current objects under `prefix` whose key sorts after `start_after`,
    /// at most `max`, sorted (a change feed's cursor read).
    async fn list_after(
        &self,
        prefix: &str,
        start_after: &str,
        max: usize,
    ) -> Result<Vec<ObjectMeta>> {
        let mut v: Vec<ObjectMeta> = self
            .list(prefix)
            .await?
            .into_iter()
            .filter(|m| m.key.as_str() > start_after)
            .collect();
        v.truncate(max);
        Ok(v)
    }
    /// Copies the current version of `from` to a new version of `to`.
    async fn copy(&self, from: &str, to: &str) -> Result<ObjectMeta> {
        let (bytes, _) = self.get(from, None).await?;
        self.put(to, bytes, Condition::None).await
    }
    /// Whether reads cross a network (the block cache only sits in front of
    /// remote backends).
    fn remote(&self) -> bool {
        false
    }
    /// A stable id of where the bytes live (cache keys include it, so a
    /// switched backend never serves another store's blocks).
    fn identity(&self) -> String {
        self.kind().to_string()
    }
    /// Writes many objects unconditionally (a sync's changed files). The
    /// default writes them one by one; a backend may do better.
    async fn put_many(&self, items: Vec<(String, Vec<u8>)>) -> Result<Vec<ObjectMeta>> {
        let mut out = Vec::with_capacity(items.len());
        for (k, b) in items {
            out.push(self.put(&k, b, Condition::None).await?);
        }
        Ok(out)
    }
}

/// A backend that is configured but cannot run here (for example `s3` in a
/// build without the `s3` feature). Every call fails with the reason, so a
/// misconfiguration is never silently served from another store.
pub struct Unavailable(pub String);

#[async_trait::async_trait]
impl Backend for Unavailable {
    fn kind(&self) -> &'static str {
        "unavailable"
    }
    async fn put(&self, _: &str, _: Vec<u8>, _: Condition) -> Result<ObjectMeta> {
        Err(crate::Error::Backend(self.0.clone()))
    }
    async fn get(&self, _: &str, _: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        Err(crate::Error::Backend(self.0.clone()))
    }
    async fn head(&self, _: &str) -> Result<Option<ObjectMeta>> {
        Err(crate::Error::Backend(self.0.clone()))
    }
    async fn list(&self, _: &str) -> Result<Vec<ObjectMeta>> {
        Err(crate::Error::Backend(self.0.clone()))
    }
    async fn delete(&self, _: &str, _: Condition) -> Result<()> {
        Err(crate::Error::Backend(self.0.clone()))
    }
    async fn versions(&self, _: &str) -> Result<Vec<VersionInfo>> {
        Err(crate::Error::Backend(self.0.clone()))
    }
}
