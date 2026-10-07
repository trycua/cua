// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`Drive`]: the backend plus grants, access requests and the audit log;
//! [`Session`]: one principal's checked view of it.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::acl::{self, Context, Grant, Mode, Principal};
use crate::audit::AuditLog;
use crate::backend::{Backend, Condition, ObjectMeta, VersionInfo};
use crate::fs::FsBackend;
use crate::path::{self, Area};
use crate::{Error, Result, new_id, now_ms, scan};

/// Names the drive keeps for itself (the sync manifest, the lease). No
/// session writes them; sync and lease code goes to the backend directly.
pub const INTERNAL_PREFIX: &str = ".cua-";

/// User presence (Touch ID or passphrase) for decisions that widen access.
pub trait Presence: Send + Sync {
    /// `Ok` only when the user confirmed `reason` just now.
    fn confirm(&self, reason: &str) -> std::result::Result<(), String>;
}

/// A presence that always refuses (no UI can ask).
pub struct NoPresence;

impl Presence for NoPresence {
    fn confirm(&self, _reason: &str) -> std::result::Result<(), String> {
        Err("no presence prompt is available here; approve it in the Cua app".into())
    }
}

/// An agent's (or Space's) request for more access, waiting for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRequest {
    pub id: String,
    pub principal: String,
    pub prefix: String,
    pub mode: Mode,
    /// The requester's words (shown as unverified).
    pub reason: String,
    pub created_ms: u64,
}

/// One row of a listing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The key (folders end in `/`).
    pub path: String,
    /// The last component.
    pub name: String,
    pub folder: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub modified_ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub etag: String,
    /// What this session may do there (`r` or `rw`).
    pub mode: Mode,
}

/// What changed in the drive (every successful write, restore, copy and
/// delete through a [`Session`]). The change feed listens for these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Put(ObjectMeta),
    Delete(String),
}

/// Told about every change, after it is stored.
pub trait ChangeSink: Send + Sync {
    fn changed(&self, change: Change);
}

struct Inner {
    backend: std::sync::RwLock<Arc<dyn Backend>>,
    state: PathBuf,
    audit: AuditLog,
    presence: Arc<dyn Presence>,
    sinks: std::sync::RwLock<Vec<Arc<dyn ChangeSink>>>,
}

/// The drive: one per account.
#[derive(Clone)]
pub struct Drive {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Drive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Drive")
            .field("backend", &self.backend().kind())
            .field("state", &self.inner.state)
            .finish()
    }
}

impl Drive {
    /// A drive over `backend`, keeping grants, requests and the audit log
    /// in `state`.
    pub fn new(backend: Arc<dyn Backend>, state: impl Into<PathBuf>) -> Drive {
        let state = state.into();
        Drive {
            inner: Arc::new(Inner {
                audit: AuditLog::new(state.join("audit.jsonl")),
                backend: std::sync::RwLock::new(backend),
                state,
                presence: Arc::new(NoPresence),
                sinks: std::sync::RwLock::new(vec![]),
            }),
        }
    }

    /// The local drive of a cua home: bytes in `<home>/volume/data`, state in
    /// `<home>/volume`.
    pub fn open_local(cua_home: &Path) -> Drive {
        let state = crate::state_dir(cua_home);
        Drive::new(Arc::new(FsBackend::new(state.join("data"))), state)
    }

    /// Uses `presence` for grants and approvals.
    pub fn with_presence(self, presence: Arc<dyn Presence>) -> Drive {
        let i = &self.inner;
        Drive {
            inner: Arc::new(Inner {
                backend: std::sync::RwLock::new(self.backend()),
                state: i.state.clone(),
                audit: i.audit.clone(),
                presence,
                sinks: std::sync::RwLock::new(i.sinks.read().unwrap().clone()),
            }),
        }
    }

    /// The backend serving the drive right now.
    pub fn backend(&self) -> Arc<dyn Backend> {
        self.inner.backend.read().unwrap().clone()
    }

    /// Switches the drive to `backend` live (a storage setting change).
    /// Grants, requests and the audit log stay; they live in the state
    /// directory, not in the store.
    pub fn set_backend(&self, backend: Arc<dyn Backend>) {
        let kind = backend.kind();
        *self.inner.backend.write().unwrap() = backend;
        self.log("user", "storage", "", &format!("backend={kind}"));
    }

    /// Adds a listener for every change made through a session.
    pub fn add_sink(&self, sink: Arc<dyn ChangeSink>) {
        self.inner.sinks.write().unwrap().push(sink);
    }

    fn notify(&self, change: Change) {
        for s in self.inner.sinks.read().unwrap().iter() {
            s.changed(change.clone());
        }
    }

    /// The presence prompt grants and approvals use.
    pub fn presence(&self) -> &Arc<dyn Presence> {
        &self.inner.presence
    }

    pub fn audit(&self) -> &AuditLog {
        &self.inner.audit
    }

    pub fn state_dir(&self) -> &Path {
        &self.inner.state
    }

    /// A checked view for `ctx`.
    pub fn session(&self, ctx: Context) -> Session {
        Session {
            drive: self.clone(),
            ctx,
        }
    }

    fn log(&self, principal: &str, action: &str, path: &str, detail: &str) {
        if let Err(e) = self.inner.audit.append(principal, action, path, detail) {
            // The audit log failing must be loud, but a read should not fail
            // because of it; writes check it themselves.
            eprintln!("cua-volume: audit append failed: {e}");
        }
    }

    fn with_json<T, R>(&self, file: &str, f: impl FnOnce(&mut Vec<T>) -> Result<R>) -> Result<R>
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let path = self.inner.state.join(file);
        fs::create_dir_all(&self.inner.state)?;
        cua_home::guard_write(&path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?;
        let mut items: Vec<T> = match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(e.into()),
        };
        let r = f(&mut items)?;
        cua_home::write_private(&path, &serde_json::to_vec_pretty(&items)?)?;
        Ok(r)
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, file: &str) -> Result<Vec<T>> {
        match fs::read(self.inner.state.join(file)) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    /// Every grant (live, expired and revoked).
    pub fn grants(&self) -> Result<Vec<Grant>> {
        self.read_json("grants.json")
    }

    /// Grants `principal` `mode` on `prefix` (the user, with presence).
    pub fn grant(
        &self,
        principal: &str,
        prefix: &str,
        mode: Mode,
        expires_ms: Option<u64>,
        note: &str,
    ) -> Result<Grant> {
        let (who, key) = acl::validate_grant(principal, prefix, mode)?;
        let reason = format!(
            "Let {} {} {}{}",
            who.id(),
            if mode == Mode::ReadWrite {
                "read and write"
            } else {
                "read"
            },
            if key.is_empty() {
                "the whole drive"
            } else {
                &key
            },
            if expires_ms.is_some() {
                " for a limited time"
            } else {
                ""
            }
        );
        self.inner
            .presence
            .confirm(&reason)
            .map_err(Error::NotConfirmed)?;
        let grant = Grant {
            id: new_id(),
            principal: who.id(),
            prefix: key.clone(),
            mode,
            created_ms: now_ms(),
            expires_ms,
            revoked: false,
            note: note.chars().take(200).collect(),
        };
        let g = grant.clone();
        self.with_json::<Grant, _>("grants.json", move |all| {
            all.push(g);
            Ok(())
        })?;
        self.log(
            "user",
            "grant",
            &key,
            &format!("{} {} id={}", grant.principal, mode.as_str(), grant.id),
        );
        Ok(grant)
    }

    /// Revokes a grant (no presence: it only narrows access).
    pub fn revoke(&self, id: &str) -> Result<Grant> {
        let g = self.with_json::<Grant, _>("grants.json", |all| {
            let g = all
                .iter_mut()
                .find(|g| g.id == id)
                .ok_or_else(|| Error::NotFound(format!("grant {id}")))?;
            g.revoked = true;
            Ok(g.clone())
        })?;
        self.log(
            "user",
            "revoke",
            &g.prefix,
            &format!("{} id={id}", g.principal),
        );
        Ok(g)
    }

    /// Access requests waiting for the user.
    pub fn requests(&self) -> Result<Vec<AccessRequest>> {
        self.read_json("requests.json")
    }

    /// Files a request for more access by `ctx` (never grants anything).
    pub fn request_access(
        &self,
        ctx: &Context,
        prefix: &str,
        mode: Mode,
        reason: &str,
    ) -> Result<AccessRequest> {
        let (who, key) = acl::validate_grant(&ctx.principal.id(), prefix, mode)?;
        let req = AccessRequest {
            id: new_id(),
            principal: who.id(),
            prefix: key.clone(),
            mode,
            reason: reason.chars().take(300).collect(),
            created_ms: now_ms(),
        };
        let r = req.clone();
        self.with_json::<AccessRequest, _>("requests.json", move |all| {
            if all.iter().filter(|x| x.principal == r.principal).count() >= 8 {
                return Err(Error::Invalid(
                    "this principal already has 8 requests waiting".into(),
                ));
            }
            all.push(r);
            Ok(())
        })?;
        self.log(
            &req.principal,
            "request",
            &key,
            &format!("{} id={}", mode.as_str(), req.id),
        );
        Ok(req)
    }

    /// Approves a request (the user, with presence): it becomes a grant.
    pub fn approve(&self, request_id: &str, expires_ms: Option<u64>) -> Result<Grant> {
        let req = self
            .requests()?
            .into_iter()
            .find(|r| r.id == request_id)
            .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?;
        let g = self.grant(
            &req.principal,
            &req.prefix,
            req.mode,
            expires_ms,
            &req.reason,
        )?;
        self.with_json::<AccessRequest, _>("requests.json", |all| {
            all.retain(|r| r.id != request_id);
            Ok(())
        })?;
        self.log(
            "user",
            "approve",
            &req.prefix,
            &format!("request={request_id} grant={}", g.id),
        );
        Ok(g)
    }

    /// Declines a request.
    pub fn deny(&self, request_id: &str) -> Result<()> {
        let req = self.with_json::<AccessRequest, _>("requests.json", |all| {
            let i = all
                .iter()
                .position(|r| r.id == request_id)
                .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?;
            Ok(all.remove(i))
        })?;
        self.log(
            "user",
            "deny",
            &req.prefix,
            &format!("request={request_id} {}", req.principal),
        );
        Ok(())
    }

    fn live_grants(&self) -> Vec<Grant> {
        let now = now_ms();
        self.grants()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| g.is_live(now))
            .collect()
    }
}

/// A blocking reader over a version's ranges (for the streaming scanner).
struct RangeReader {
    backend: Arc<dyn Backend>,
    key: String,
    version: String,
    size: u64,
    pos: u64,
    handle: tokio::runtime::Handle,
}

impl std::io::Read for RangeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.size - self.pos).min(8 << 20);
        let bytes = self
            .handle
            .block_on(
                self.backend
                    .get_range(&self.key, &self.version, self.pos, want),
            )
            .map_err(std::io::Error::other)?;
        let n = bytes.len();
        buf[..n].copy_from_slice(&bytes);
        self.pos += n as u64;
        Ok(n)
    }
}

/// One principal's checked view of the drive.
#[derive(Clone, Debug)]
pub struct Session {
    drive: Drive,
    ctx: Context,
}

/// Whether any component of `key` is one of the drive's own names (the
/// sync manifest, a lease, a folder marker, the change feed).
pub fn is_internal(key: &str) -> bool {
    key.split('/').any(|n| n.starts_with(INTERNAL_PREFIX))
}

/// The marker object that keeps an empty folder (made with
/// [`Session::mkdir`]) in a store that has no folders.
pub const FOLDER_MARKER: &str = ".cua-keep";

impl Session {
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    pub fn drive(&self) -> &Drive {
        &self.drive
    }

    fn who(&self) -> String {
        self.ctx.principal.id()
    }

    fn quiet(&self) -> bool {
        self.ctx.principal == Principal::User
    }

    /// The access this session has on `path`.
    pub fn mode(&self, path: &str) -> Result<Option<Mode>> {
        let key = path::normalize(path)?;
        Ok(acl::effective_mode(
            &self.ctx,
            &key,
            &self.drive.live_grants(),
            now_ms(),
        ))
    }

    fn require(&self, key: &str, want: Mode, what: &str) -> Result<()> {
        let have = acl::effective_mode(&self.ctx, key, &self.drive.live_grants(), now_ms());
        if have.is_some_and(|m| m.allows(want)) {
            return Ok(());
        }
        self.drive.log(&self.who(), "denied", key, what);
        Err(Error::Forbidden(format!(
            "{} may not {what} {key}{}",
            self.who(),
            match have {
                Some(Mode::Read) => " (read-only)",
                _ => "",
            }
        )))
    }

    /// The immediate children of folder `path` this session can see.
    pub async fn ls(&self, path: &str) -> Result<Vec<Entry>> {
        let folder = path::folder(path)?;
        let grants = self.drive.live_grants();
        let now = now_ms();
        if !acl::can_traverse(&self.ctx, &folder, &grants, now) {
            self.drive.log(&self.who(), "denied", &folder, "list");
            return Err(Error::Forbidden(format!(
                "{} may not list {folder}",
                self.who()
            )));
        }
        let mut files = vec![];
        let mut folders = std::collections::BTreeSet::new();
        let (objects, subfolders) = self.drive.backend().list_dir(&folder).await?;
        for f in subfolders {
            if !is_internal(&f) {
                folders.insert(f);
            }
        }
        for m in objects {
            if is_internal(&m.key) {
                continue;
            }
            let rest = &m.key[folder.len()..];
            if let Some(mode) = acl::effective_mode(&self.ctx, &m.key, &grants, now) {
                files.push(Entry {
                    name: rest.to_string(),
                    path: m.key.clone(),
                    folder: false,
                    size: m.size,
                    modified_ms: m.modified_ms,
                    etag: m.etag,
                    mode,
                });
            }
        }
        // The areas this session always has, even while empty.
        let mut own = vec![];
        if self.ctx.principal == Principal::User {
            own.extend([
                path::PUBLIC.to_string(),
                path::AGENTS.into(),
                path::SPACES.into(),
            ]);
        } else {
            own.push(path::PUBLIC.to_string());
            if let Principal::Agent(a) = &self.ctx.principal {
                own.push(format!("{}{a}/", path::AGENTS));
            }
            if let Some(s) = &self.ctx.space {
                own.push(path::space_folder(s));
            }
        }
        for o in own {
            if let Some(rest) = o.strip_prefix(&folder)
                && !rest.is_empty()
            {
                let child = rest.split('/').next().unwrap_or("");
                folders.insert(format!("{folder}{child}/"));
            }
        }
        let mut out = vec![];
        for f in folders {
            if !acl::can_traverse(&self.ctx, &f, &grants, now) {
                continue;
            }
            let mode = acl::effective_mode(&self.ctx, &f, &grants, now).unwrap_or(Mode::Read);
            out.push(Entry {
                name: f[folder.len()..].trim_end_matches('/').to_string(),
                path: f,
                folder: true,
                size: 0,
                modified_ms: 0,
                etag: String::new(),
                mode,
            });
        }
        out.extend(files);
        Ok(out)
    }

    /// Every readable file under folder `path` (recursive), without the
    /// drive's internal files.
    pub async fn walk(&self, path: &str) -> Result<Vec<ObjectMeta>> {
        let folder = path::folder(path)?;
        let grants = self.drive.live_grants();
        let now = now_ms();
        if !acl::can_traverse(&self.ctx, &folder, &grants, now) {
            self.drive.log(&self.who(), "denied", &folder, "list");
            return Err(Error::Forbidden(format!(
                "{} may not list {folder}",
                self.who()
            )));
        }
        Ok(self
            .drive
            .backend()
            .list(&folder)
            .await?
            .into_iter()
            .filter(|m| !is_internal(&m.key))
            .filter(|m| acl::effective_mode(&self.ctx, &m.key, &grants, now).is_some())
            .collect())
    }

    fn file_key(path: &str) -> Result<String> {
        let key = path::normalize(path)?;
        if key.is_empty() || key.ends_with('/') {
            return Err(Error::Invalid(format!("{path:?} is a folder, not a file")));
        }
        Ok(key)
    }

    /// Reads a file (or one of its versions).
    pub async fn read(&self, path: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        self.read_logged(path, version, true).await
    }

    /// A read checked like [`Session::read`], logged or not (a sync logs
    /// one summary line instead of one per file).
    pub(crate) async fn read_logged(
        &self,
        path: &str,
        version: Option<&str>,
        log: bool,
    ) -> Result<(Vec<u8>, ObjectMeta)> {
        let key = Self::file_key(path)?;
        self.require(&key, Mode::Read, "read")?;
        let r = self.drive.backend().get(&key, version).await?;
        if log && !self.quiet() {
            self.drive.log(&self.who(), "read", &key, "");
        }
        Ok(r)
    }

    fn scan_or_block(&self, key: &str, bytes: &[u8]) -> Result<()> {
        if !matches!(path::area(key), Area::Agent(_)) {
            return Ok(());
        }
        if let Some(f) = scan::scan(bytes).into_iter().next() {
            self.drive.log(
                &self.who(),
                "secret_blocked",
                key,
                &format!("{} on line {}", f.kind, f.line),
            );
            return Err(Error::SecretDetected {
                path: key.to_string(),
                kind: f.kind.into(),
                line: f.line,
            });
        }
        Ok(())
    }

    /// Writes a file. `cond` makes it create-only or compare-and-swap.
    pub async fn write(&self, path: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta> {
        self.write_logged(path, bytes, cond, true).await
    }

    /// A write checked like [`Session::write`] (access and the secret
    /// scanner always apply), logged or not.
    pub(crate) async fn write_logged(
        &self,
        path: &str,
        bytes: Vec<u8>,
        cond: Condition,
        log: bool,
    ) -> Result<ObjectMeta> {
        let key = Self::file_key(path)?;
        if is_internal(&key) {
            return Err(Error::Invalid(format!(
                "names starting with {INTERNAL_PREFIX} are reserved"
            )));
        }
        self.require(&key, Mode::ReadWrite, "write")?;
        self.scan_or_block(&key, &bytes)?;
        let m = self.drive.backend().put(&key, bytes, cond).await?;
        if log && !self.quiet() {
            self.drive
                .log(&self.who(), "write", &key, &format!("{} bytes", m.size));
        }
        self.drive.notify(Change::Put(m.clone()));
        Ok(m)
    }

    /// Checks a batch of writes like [`Session::write`] (access, reserved
    /// names, the secret scanner) and writes the ones that pass in one
    /// backend call. Returns the files the scanner kept out: `(path, kind)`.
    pub(crate) async fn write_many_quiet(
        &self,
        items: Vec<(String, Vec<u8>)>,
    ) -> Result<(Vec<ObjectMeta>, Vec<(String, String)>)> {
        let mut ok = vec![];
        let mut blocked = vec![];
        for (path, bytes) in items {
            let key = Self::file_key(&path)?;
            if is_internal(&key) {
                return Err(Error::Invalid(format!(
                    "names starting with {INTERNAL_PREFIX} are reserved"
                )));
            }
            self.require(&key, Mode::ReadWrite, "write")?;
            match self.scan_or_block(&key, &bytes) {
                Ok(()) => ok.push((key, bytes)),
                Err(Error::SecretDetected { kind, .. }) => blocked.push((path, kind)),
                Err(e) => return Err(e),
            }
        }
        let written = self.drive.backend().put_many(ok).await?;
        for m in &written {
            self.drive.notify(Change::Put(m.clone()));
        }
        Ok((written, blocked))
    }

    /// Deletes a file (a delete marker: its history stays).
    pub async fn delete(&self, path: &str, cond: Condition) -> Result<()> {
        let key = Self::file_key(path)?;
        if is_internal(&key) {
            return Err(Error::Invalid(format!(
                "names starting with {INTERNAL_PREFIX} are reserved"
            )));
        }
        self.require(&key, Mode::ReadWrite, "delete")?;
        self.drive.backend().delete(&key, cond).await?;
        self.drive.log(&self.who(), "delete", &key, "");
        self.drive.notify(Change::Delete(key));
        Ok(())
    }

    /// A file's history, newest first.
    pub async fn history(&self, path: &str) -> Result<Vec<VersionInfo>> {
        let key = Self::file_key(path)?;
        self.require(&key, Mode::Read, "read")?;
        self.drive.backend().versions(&key).await
    }

    /// Makes `version` the current content again (a new version).
    pub async fn restore(&self, path: &str, version: &str) -> Result<ObjectMeta> {
        let key = Self::file_key(path)?;
        self.require(&key, Mode::ReadWrite, "restore")?;
        let (bytes, _) = self.drive.backend().get(&key, Some(version)).await?;
        self.scan_or_block(&key, &bytes)?;
        let m = self
            .drive
            .backend()
            .put(&key, bytes, Condition::None)
            .await?;
        self.drive
            .log(&self.who(), "restore", &key, &format!("from={version}"));
        self.drive.notify(Change::Put(m.clone()));
        Ok(m)
    }

    /// Checks read access to a file and returns the metadata of the version
    /// a streaming reader should pin (the current one, or `version`). One
    /// audit line per open, not per block.
    pub async fn open(&self, path: &str, version: Option<&str>) -> Result<ObjectMeta> {
        let key = Self::file_key(path)?;
        if is_internal(&key) {
            return Err(Error::NotFound(key));
        }
        self.require(&key, Mode::Read, "read")?;
        let backend = self.drive.backend();
        let meta = match version {
            None => backend
                .head(&key)
                .await?
                .ok_or_else(|| Error::NotFound(key.clone()))?,
            Some(v) => {
                let info = backend
                    .versions(&key)
                    .await?
                    .into_iter()
                    .find(|x| x.version == v && !x.deleted)
                    .ok_or_else(|| Error::NotFound(format!("{key} version {v}")))?;
                ObjectMeta {
                    key: key.clone(),
                    size: info.size,
                    etag: String::new(),
                    version: info.version,
                    modified_ms: info.modified_ms,
                }
            }
        };
        if !self.quiet() {
            self.drive.log(&self.who(), "read", &key, "open");
        }
        Ok(meta)
    }

    /// Writes a file from a local path of any size (a multipart upload on
    /// S3), with the same checks as [`Session::write`]: access, reserved
    /// names, and the secret scanner (streamed) under `agents/`.
    pub async fn write_file(
        &self,
        path: &str,
        local: &Path,
        cond: Condition,
    ) -> Result<ObjectMeta> {
        let key = Self::file_key(path)?;
        if is_internal(&key) {
            return Err(Error::Invalid(format!(
                "names starting with {INTERNAL_PREFIX} are reserved"
            )));
        }
        self.require(&key, Mode::ReadWrite, "write")?;
        if matches!(path::area(&key), Area::Agent(_)) {
            let file = local.to_path_buf();
            let found = tokio::task::spawn_blocking(move || {
                scan::scan_reader(std::io::BufReader::new(fs::File::open(file)?))
            })
            .await
            .map_err(|e| Error::Backend(e.to_string()))??;
            if let Some(f) = found {
                self.drive.log(
                    &self.who(),
                    "secret_blocked",
                    &key,
                    &format!("{} on line {}", f.kind, f.line),
                );
                return Err(Error::SecretDetected {
                    path: key,
                    kind: f.kind.into(),
                    line: f.line,
                });
            }
        }
        let m = self.drive.backend().put_file(&key, local, cond).await?;
        if !self.quiet() {
            self.drive
                .log(&self.who(), "write", &key, &format!("{} bytes", m.size));
        }
        self.drive.notify(Change::Put(m.clone()));
        Ok(m)
    }

    /// Makes an (empty) folder visible: stores the folder marker.
    pub async fn mkdir(&self, path: &str) -> Result<String> {
        let folder = path::folder(path)?;
        if folder.is_empty() {
            return Err(Error::Invalid("the root always exists".into()));
        }
        if is_internal(&folder) {
            return Err(Error::Invalid(format!(
                "names starting with {INTERNAL_PREFIX} are reserved"
            )));
        }
        let marker = format!("{folder}{FOLDER_MARKER}");
        self.require(&marker, Mode::ReadWrite, "write")?;
        self.drive
            .backend()
            .put(&marker, vec![], Condition::None)
            .await?;
        if !self.quiet() {
            self.drive.log(&self.who(), "mkdir", &folder, "");
        }
        self.drive.notify(Change::Put(ObjectMeta {
            key: marker,
            size: 0,
            etag: String::new(),
            version: String::new(),
            modified_ms: now_ms(),
        }));
        Ok(folder)
    }

    /// Removes an empty folder's marker (a folder with files keeps
    /// existing while they do).
    pub async fn rmdir(&self, path: &str) -> Result<()> {
        let folder = path::folder(path)?;
        let (files, folders) = self.drive.backend().list_dir(&folder).await?;
        let visible = files.iter().filter(|m| !is_internal(&m.key)).count()
            + folders.iter().filter(|f| !is_internal(f)).count();
        if visible > 0 {
            return Err(Error::Precondition(format!("{folder} is not empty")));
        }
        let marker = format!("{folder}{FOLDER_MARKER}");
        self.require(&marker, Mode::ReadWrite, "delete")?;
        match self.drive.backend().delete(&marker, Condition::None).await {
            Ok(()) | Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        self.drive.notify(Change::Delete(marker));
        Ok(())
    }

    /// Copies a file inside the drive (a server-side copy on S3). The
    /// secret scanner runs on the source when the copy lands in an agent
    /// home it was not already in.
    pub async fn copy(&self, from: &str, to: &str) -> Result<ObjectMeta> {
        let src = Self::file_key(from)?;
        let dst = Self::file_key(to)?;
        if is_internal(&src) || is_internal(&dst) {
            return Err(Error::Invalid(format!(
                "names starting with {INTERNAL_PREFIX} are reserved"
            )));
        }
        self.require(&src, Mode::Read, "read")?;
        self.require(&dst, Mode::ReadWrite, "write")?;
        let backend = self.drive.backend();
        if let Area::Agent(a) = path::area(&dst)
            && path::area(&src) != Area::Agent(a)
        {
            let meta = backend
                .head(&src)
                .await?
                .ok_or_else(|| Error::NotFound(src.clone()))?;
            let reader = RangeReader {
                backend: backend.clone(),
                key: src.clone(),
                version: meta.version.clone(),
                size: meta.size,
                pos: 0,
                handle: tokio::runtime::Handle::current(),
            };
            let found = tokio::task::spawn_blocking(move || scan::scan_reader(reader))
                .await
                .map_err(|e| Error::Backend(e.to_string()))??;
            if let Some(f) = found {
                self.drive.log(
                    &self.who(),
                    "secret_blocked",
                    &dst,
                    &format!("{} on line {}", f.kind, f.line),
                );
                return Err(Error::SecretDetected {
                    path: dst,
                    kind: f.kind.into(),
                    line: f.line,
                });
            }
        }
        let m = backend.copy(&src, &dst).await?;
        if !self.quiet() {
            self.drive
                .log(&self.who(), "copy", &dst, &format!("from={src}"));
        }
        self.drive.notify(Change::Put(m.clone()));
        Ok(m)
    }

    /// Asks the user for more access.
    pub fn request_access(&self, prefix: &str, mode: Mode, reason: &str) -> Result<AccessRequest> {
        self.drive.request_access(&self.ctx, prefix, mode, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Yes(Mutex<Vec<String>>);
    impl Presence for Yes {
        fn confirm(&self, reason: &str) -> std::result::Result<(), String> {
            self.0.lock().unwrap().push(reason.into());
            Ok(())
        }
    }

    fn drive(dir: &Path) -> (Drive, Arc<Yes>) {
        let yes = Arc::new(Yes(Mutex::default()));
        (Drive::open_local(dir).with_presence(yes.clone()), yes)
    }

    #[tokio::test]
    async fn an_agent_cannot_read_another_agents_home_without_a_grant() {
        let dir = tempfile::tempdir().unwrap();
        let (d, yes) = drive(dir.path());
        let user = d.session(Context::user());
        user.write(
            "agents/writer/outputs/report.md",
            b"draft".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
        user.write("public/rules.md", b"be kind".to_vec(), Condition::None)
            .await
            .unwrap();
        let rs = d.session(Context::agent("researcher", Some("local:lab")));
        assert_eq!(
            rs.read("agents/writer/outputs/report.md", None)
                .await
                .unwrap_err()
                .tag(),
            "forbidden"
        );
        assert_eq!(
            rs.ls("agents/writer/").await.unwrap_err().tag(),
            "forbidden"
        );
        assert_eq!(
            rs.read("public/rules.md", None).await.unwrap().0,
            b"be kind"
        );
        assert_eq!(
            rs.write("public/rules.md", b"x".to_vec(), Condition::None)
                .await
                .unwrap_err()
                .tag(),
            "forbidden"
        );
        rs.write(
            "agents/researcher/memory/MEMORY.md",
            b"notes".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
        rs.write("spaces/local-lab/out.txt", b"o".to_vec(), Condition::None)
            .await
            .unwrap();
        assert_eq!(
            rs.write("spaces/cloud-x/out.txt", b"o".to_vec(), Condition::None)
                .await
                .unwrap_err()
                .tag(),
            "forbidden"
        );
        // The agent asks; the user approves with presence; access is exact.
        let req = rs
            .request_access("agents/writer/outputs/", Mode::Read, "cite the draft")
            .unwrap();
        assert_eq!(d.requests().unwrap().len(), 1);
        let g = d.approve(&req.id, None).unwrap();
        assert!(d.requests().unwrap().is_empty());
        assert_eq!(yes.0.lock().unwrap().len(), 1, "presence was asked");
        assert_eq!(
            rs.read("agents/writer/outputs/report.md", None)
                .await
                .unwrap()
                .0,
            b"draft"
        );
        assert_eq!(
            rs.write(
                "agents/writer/outputs/report.md",
                b"x".to_vec(),
                Condition::None
            )
            .await
            .unwrap_err()
            .tag(),
            "forbidden"
        );
        d.revoke(&g.id).unwrap();
        assert_eq!(
            rs.read("agents/writer/outputs/report.md", None)
                .await
                .unwrap_err()
                .tag(),
            "forbidden"
        );
        let (events, ok) = d.audit().tail(50).unwrap();
        assert!(ok.is_ok());
        let actions: Vec<&str> = events.iter().rev().map(|e| e.action.as_str()).collect();
        for want in ["denied", "request", "grant", "approve", "read", "revoke"] {
            assert!(actions.contains(&want), "{want} in {actions:?}");
        }
    }

    #[tokio::test]
    async fn grants_need_presence() {
        let dir = tempfile::tempdir().unwrap();
        let d = Drive::open_local(dir.path());
        assert_eq!(
            d.grant("agent:ada", "agents/bob/", Mode::Read, None, "")
                .unwrap_err()
                .tag(),
            "not_confirmed"
        );
        assert!(d.grants().unwrap().is_empty());
    }

    #[tokio::test]
    async fn secrets_are_kept_out_of_agent_homes() {
        let dir = tempfile::tempdir().unwrap();
        let (d, _) = drive(dir.path());
        let ada = d.session(Context::agent("ada", None));
        let leaked = format!("remember {}{}", "AKIA", "ABCDEFGHIJKLMNOP");
        let e = ada
            .write(
                "agents/ada/memory/MEMORY.md",
                leaked.clone().into_bytes(),
                Condition::None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.tag(), "secret_detected");
        assert!(!e.to_string().contains("ABCDEFGHIJKLMNOP"), "{e}");
        let (events, _) = d.audit().tail(5).unwrap();
        assert_eq!(events[0].action, "secret_blocked");
        assert!(!events[0].detail.contains("ABCDEF"));
        // Outside agents/ the scanner does not run (the user's own files).
        d.session(Context::user())
            .write("spaces/x/env.txt", leaked.into_bytes(), Condition::None)
            .await
            .unwrap();
        assert_eq!(
            ada.write("agents/ada/.cua-lease", b"x".to_vec(), Condition::None)
                .await
                .unwrap_err()
                .tag(),
            "invalid_argument"
        );
    }

    #[tokio::test]
    async fn listings_show_only_what_the_session_can_reach() {
        let dir = tempfile::tempdir().unwrap();
        let (d, _) = drive(dir.path());
        let user = d.session(Context::user());
        for k in [
            "agents/ada/a.md",
            "agents/bob/b.md",
            "public/p.md",
            "spaces/local-w/s.md",
        ] {
            user.write(k, b"x".to_vec(), Condition::None).await.unwrap();
        }
        let ada = d.session(Context::agent("ada", Some("local:w")));
        let names = |v: Vec<Entry>| v.into_iter().map(|e| e.path).collect::<Vec<_>>();
        assert_eq!(
            names(ada.ls("").await.unwrap()),
            ["agents/", "public/", "spaces/"]
        );
        assert_eq!(names(ada.ls("agents/").await.unwrap()), ["agents/ada/"]);
        assert_eq!(names(ada.ls("spaces").await.unwrap()), ["spaces/local-w/"]);
        let u = names(user.ls("agents/").await.unwrap());
        assert_eq!(u, ["agents/ada/", "agents/bob/"]);
        // A brand new agent sees its (empty) home.
        let cy = d.session(Context::agent("cy", None));
        assert_eq!(names(cy.ls("agents/").await.unwrap()), ["agents/cy/"]);
        // History and restore.
        let m1 = user
            .write("public/p.md", b"v2".to_vec(), Condition::None)
            .await
            .unwrap();
        let hist = user.history("public/p.md").await.unwrap();
        assert_eq!(hist.len(), 2);
        assert!(hist[0].latest && hist[0].version == m1.version);
        user.restore("public/p.md", &hist[1].version).await.unwrap();
        assert_eq!(user.read("public/p.md", None).await.unwrap().0, b"x");
    }
}
