// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `FilesystemService`: metadata, directory operations, watches, chunked
//! reads and writes, resumable uploads and signed URLs.

pub mod paths;
pub mod watch;
pub mod write;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use cua_proto::env::v1::filesystem_service_server::{FilesystemService, FilesystemServiceServer};
use cua_proto::env::v1::read_file_response::Message as ReadMessage;
use cua_proto::env::v1::watch_dir_response::Message as WatchMessageProto;
use cua_proto::env::v1::write_file_request::Message as WriteMessage;
use cua_proto::env::v1::*;
use futures_util::Stream;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status, Streaming};

use crate::config::{MAX_CHUNK_BYTES, PREFERRED_CHUNK_BYTES};

/// The permission bits a remote caller may set on a file or directory it
/// writes: read, write and execute only. Set-user-ID, set-group-ID and sticky
/// bits are dropped, so a write never creates a file that runs with the
/// daemon's identity.
pub(crate) fn remote_mode(mode: u32) -> u32 {
    mode & 0o777
}
use crate::context::ServerContext;
use crate::error::{io_status, session_not_found, status, StatusBuilder};
use crate::util::{duration, hex_digest, random_id, timestamp};

use paths::entry_info;
use write::{AtomicWriter, WriteTarget};

/// Default lifetime of a partial upload without progress.
pub const UPLOAD_DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);
/// Longest partial-upload lifetime.
pub const UPLOAD_MAX_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Buffered watchers not polled for this long are removed.
pub const WATCHER_IDLE_TTL: Duration = Duration::from_secs(5 * 60);

struct Upload {
    path: PathBuf,
    header: WriteFileHeader,
    writer: Option<AtomicWriter>,
    ttl: Duration,
    expires: Instant,
}

/// Resolves paths against the `Init` defaults.
#[derive(Clone)]
pub struct PathResolver {
    ctx: ServerContext,
}

impl PathResolver {
    /// Creates a resolver.
    pub fn new(ctx: ServerContext) -> Self {
        Self { ctx }
    }

    /// Home of the acting (`Init` default) user.
    pub fn home(&self) -> PathBuf {
        let init = self.ctx.init_state();
        if let Some(user) = init.default_user.as_deref() {
            if let Ok(info) = crate::process::spawn::lookup_user(user) {
                return info.home;
            }
        }
        crate::config::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    }

    /// Resolves a request path. Empty paths are rejected.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, Status> {
        if path.is_empty() {
            return Err(crate::error::invalid("path is required"));
        }
        if path.contains('\0') {
            return Err(crate::error::invalid("path contains NUL"));
        }
        let init = self.ctx.init_state();
        Ok(paths::expand(
            path,
            &self.home(),
            init.default_workdir.as_deref(),
        ))
    }
}

/// Shared filesystem state.
pub struct FsState {
    ctx: ServerContext,
    resolver: PathResolver,
    uploads: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Upload>>>>,
    watchers: Mutex<HashMap<String, Arc<watch::BufferedWatcher>>>,
}

impl FsState {
    /// Creates the state and its expiry sweeper.
    pub fn new(ctx: ServerContext) -> Arc<Self> {
        let state = Arc::new(Self {
            resolver: PathResolver::new(ctx.clone()),
            ctx,
            uploads: Mutex::new(HashMap::new()),
            watchers: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&state);
        let shutdown = state.ctx.shutdown_token();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = shutdown.cancelled() => return,
                }
                let Some(state) = weak.upgrade() else { return };
                state.sweep().await;
            }
        });
        state
    }

    /// Path resolver.
    pub fn resolver(&self) -> &PathResolver {
        &self.resolver
    }

    async fn sweep(&self) {
        let uploads: Vec<(String, Arc<tokio::sync::Mutex<Upload>>)> = self
            .uploads
            .lock()
            .expect("uploads lock")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, upload) in uploads {
            let mut guard = upload.lock().await;
            if guard.expires <= Instant::now() {
                if let Some(writer) = guard.writer.take() {
                    writer.abort().await;
                }
                self.uploads.lock().expect("uploads lock").remove(&id);
                tracing::debug!(upload = %id, "expired partial upload removed");
            }
        }
        self.watchers
            .lock()
            .expect("watchers lock")
            .retain(|_, w| w.idle() < WATCHER_IDLE_TTL);
    }

    fn upload(&self, id: &str) -> Result<Arc<tokio::sync::Mutex<Upload>>, Status> {
        self.uploads
            .lock()
            .expect("uploads lock")
            .get(id)
            .cloned()
            .ok_or_else(|| session_not_found("upload", id))
    }
}

/// The gRPC service.
#[derive(Clone)]
pub struct FilesystemServiceImpl {
    state: Arc<FsState>,
}

impl FilesystemServiceImpl {
    /// Creates the service.
    pub fn new(state: Arc<FsState>) -> Self {
        Self { state }
    }

    /// Tonic server with the driver's message limits.
    pub fn into_server(self) -> FilesystemServiceServer<Self> {
        FilesystemServiceServer::new(self)
            .max_decoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
            .max_encoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, Status> {
        self.state.resolver.resolve(path)
    }

    /// Resolves `path` for a caller: a viewer ticket may only touch its
    /// grant's `files_root` (symlinks included).
    fn resolve_as(
        &self,
        viewer: Option<&crate::auth::ViewerGrant>,
        path: &str,
    ) -> Result<PathBuf, Status> {
        let resolved = self.resolve(path)?;
        match viewer {
            None => Ok(resolved),
            Some(grant) => confine(grant, resolved),
        }
    }
}

/// Keeps a viewer inside its grant's `files_root`: the lexical path must be
/// under the root, and so must the canonical form of its deepest existing
/// ancestor (a symlink inside the root cannot lead out of it).
pub fn confine(grant: &crate::auth::ViewerGrant, path: PathBuf) -> Result<PathBuf, Status> {
    let denied = |why: &str| {
        StatusBuilder::new(
            Code::PermissionDenied,
            ErrorReason::PermissionDenied,
            format!("viewer ticket: {why}"),
        )
        .build()
    };
    let Some(root) = grant.files_root.as_deref().map(Path::new) else {
        return Err(denied("no file access was granted"));
    };
    if !path.starts_with(root) {
        return Err(denied(&format!(
            "{} is outside {}",
            path.display(),
            root.display()
        )));
    }
    let mut probe = path.as_path();
    loop {
        match std::fs::canonicalize(probe) {
            Ok(real) => {
                if !real.starts_with(root) {
                    return Err(denied(&format!(
                        "{} leaves {}",
                        path.display(),
                        root.display()
                    )));
                }
                break;
            }
            Err(_) => match probe.parent() {
                Some(parent) => probe = parent,
                None => break,
            },
        }
    }
    Ok(path)
}

fn write_mode(value: i32) -> Result<WriteMode, Status> {
    match WriteMode::try_from(value) {
        Ok(WriteMode::Unspecified) => Ok(WriteMode::Overwrite),
        Ok(mode) => Ok(mode),
        Err(_) => Err(crate::error::invalid("unknown write mode")),
    }
}

fn target_for(path: PathBuf, header: &WriteFileHeader) -> Result<WriteTarget, Status> {
    Ok(WriteTarget {
        dest: path,
        mode: write_mode(header.mode)?,
        permissions: remote_mode(header.permissions),
        create_parents: header.create_parents,
    })
}

fn chunk_too_large(len: usize, max: u32) -> Option<Status> {
    (len > max as usize).then(|| {
        StatusBuilder::new(
            Code::InvalidArgument,
            ErrorReason::LimitExceeded,
            format!("chunk of {len} bytes exceeds the {max}-byte limit"),
        )
        .build()
    })
}

async fn stat(path: &Path, follow: bool) -> Result<EntryInfo, Status> {
    let meta = if follow {
        tokio::fs::metadata(path).await
    } else {
        tokio::fs::symlink_metadata(path).await
    }
    .map_err(|e| io_status(&e, path))?;
    Ok(entry_info(path, &meta))
}

/// Depth-first listing, directories before their contents, sorted by name.
fn list_dir_blocking(
    root: &Path,
    depth: u32,
    include_hidden: bool,
    skip: usize,
    take: usize,
) -> std::io::Result<(Vec<EntryInfo>, bool)> {
    struct Walk {
        max_depth: u32,
        include_hidden: bool,
        skip: usize,
        take: usize,
        seen: usize,
        out: Vec<EntryInfo>,
        more: bool,
    }
    impl Walk {
        fn visit(&mut self, dir: &Path, level: u32) -> std::io::Result<()> {
            let mut children: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
            children.sort_by_key(|e| e.file_name());
            for child in children {
                if self.more {
                    return Ok(());
                }
                if !self.include_hidden && child.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let path = child.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if self.seen >= self.skip {
                    if self.out.len() == self.take {
                        self.more = true;
                        return Ok(());
                    }
                    self.out.push(entry_info(&path, &meta));
                }
                self.seen += 1;
                if meta.is_dir() && level < self.max_depth {
                    // Unreadable subdirectories are listed but not descended.
                    let _ = self.visit(&path, level + 1);
                }
            }
            Ok(())
        }
    }
    let mut walk = Walk {
        max_depth: depth.max(1),
        include_hidden,
        skip,
        take,
        seen: 0,
        out: Vec::new(),
        more: false,
    };
    walk.visit(root, 1)?;
    Ok((walk.out, walk.more))
}

/// Recursively copies (used when `rename` crosses filesystems).
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, meta.permissions())?;
    } else if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
        #[cfg(not(unix))]
        std::fs::copy(from, to).map(|_| ())?;
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) => Err(e),
    }
}

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[tonic::async_trait]
impl FilesystemService for FilesystemServiceImpl {
    type WatchDirStream = BoxStream<WatchDirResponse>;
    type ReadFileStream = BoxStream<ReadFileResponse>;

    async fn stat(&self, request: Request<StatRequest>) -> Result<Response<StatResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        Ok(Response::new(StatResponse {
            entry: Some(stat(&path, !body.no_follow_symlinks).await?),
        }))
    }

    async fn list_dir(
        &self,
        request: Request<ListDirRequest>,
    ) -> Result<Response<ListDirResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| io_status(&e, &path))?;
        if !meta.is_dir() {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::NotADirectory,
                format!("{}: not a directory", path.display()),
            ));
        }
        let page_size = match body.page_size {
            0 => 1000,
            n => n.min(10_000),
        } as usize;
        let skip: usize = if body.page_token.is_empty() {
            0
        } else {
            body.page_token
                .parse()
                .map_err(|_| crate::error::invalid("invalid page_token"))?
        };
        let root = path.clone();
        let (entries, more) = tokio::task::spawn_blocking(move || {
            list_dir_blocking(&root, body.depth, body.include_hidden, skip, page_size)
        })
        .await
        .map_err(|e| crate::error::internal(e.to_string()))?
        .map_err(|e| io_status(&e, &path))?;
        let next_page_token = if more {
            (skip + entries.len()).to_string()
        } else {
            String::new()
        };
        Ok(Response::new(ListDirResponse {
            entries,
            next_page_token,
        }))
    }

    async fn make_dir(
        &self,
        request: Request<MakeDirRequest>,
    ) -> Result<Response<MakeDirResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_dir() => {
                return Ok(Response::new(MakeDirResponse {
                    entry: Some(entry_info(&path, &meta)),
                    created: false,
                }))
            }
            Ok(_) => {
                return Err(StatusBuilder::new(
                    Code::AlreadyExists,
                    ErrorReason::PathExists,
                    format!("{} exists and is not a directory", path.display()),
                )
                .meta("path", path.display())
                .build())
            }
            Err(_) => {}
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(body.parents);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(if body.mode == 0 {
                0o755
            } else {
                remote_mode(body.mode)
            });
        }
        builder.create(&path).map_err(|e| io_status(&e, &path))?;
        Ok(Response::new(MakeDirResponse {
            entry: Some(stat(&path, true).await?),
            created: true,
        }))
    }

    async fn r#move(
        &self,
        request: Request<MoveRequest>,
    ) -> Result<Response<MoveResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let source = self.resolve_as(viewer.as_deref(), &body.source)?;
        let destination = self.resolve_as(viewer.as_deref(), &body.destination)?;
        tokio::fs::symlink_metadata(&source)
            .await
            .map_err(|e| io_status(&e, &source))?;
        if tokio::fs::symlink_metadata(&destination).await.is_ok() {
            if !body.overwrite {
                return Err(StatusBuilder::new(
                    Code::AlreadyExists,
                    ErrorReason::PathExists,
                    format!("{} already exists", destination.display()),
                )
                .meta("path", destination.display())
                .build());
            }
            let dest = destination.clone();
            tokio::task::spawn_blocking(move || remove_any(&dest))
                .await
                .map_err(|e| crate::error::internal(e.to_string()))?
                .map_err(|e| io_status(&e, &destination))?;
        }
        if body.create_parents {
            if let Some(parent) = destination.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| io_status(&e, parent))?;
            }
        }
        if let Err(error) = tokio::fs::rename(&source, &destination).await {
            #[cfg(unix)]
            let cross_device = error.raw_os_error() == Some(libc::EXDEV);
            #[cfg(not(unix))]
            let cross_device = error.raw_os_error() == Some(17); // ERROR_NOT_SAME_DEVICE
            if !cross_device {
                return Err(io_status(&error, &source));
            }
            let (from, to) = (source.clone(), destination.clone());
            tokio::task::spawn_blocking(move || {
                copy_recursive(&from, &to)?;
                remove_any(&from)
            })
            .await
            .map_err(|e| crate::error::internal(e.to_string()))?
            .map_err(|e| io_status(&e, &destination))?;
        }
        Ok(Response::new(MoveResponse {
            entry: Some(stat(&destination, false).await?),
        }))
    }

    async fn remove(
        &self,
        request: Request<RemoveRequest>,
    ) -> Result<Response<RemoveResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        let meta = match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && body.missing_ok => {
                return Ok(Response::new(RemoveResponse {}))
            }
            Err(e) => return Err(io_status(&e, &path)),
        };
        let result = if meta.is_dir() {
            if body.recursive {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_dir(&path).await
            }
        } else {
            tokio::fs::remove_file(&path).await
        };
        result.map_err(|e| io_status(&e, &path))?;
        Ok(Response::new(RemoveResponse {}))
    }

    async fn watch_dir(
        &self,
        request: Request<WatchDirRequest>,
    ) -> Result<Response<Self::WatchDirStream>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| io_status(&e, &path))?;
        if !meta.is_dir() {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::NotADirectory,
                format!("{}: not a directory", path.display()),
            ));
        }
        let keepalive = duration(body.keepalive_interval.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_secs(30))
            .max(Duration::from_millis(100));
        // Watch events flow through a bounded queue; a full queue becomes an
        // overflow notice instead of blocking the notifier thread.
        let (events_tx, mut events_rx) = mpsc::channel::<watch::WatchMessage>(4096);
        let overflow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink: Arc<dyn Fn(watch::WatchMessage) + Send + Sync> = {
            let overflow = overflow.clone();
            let events_tx = events_tx.clone();
            Arc::new(move |message| {
                if events_tx.try_send(message).is_err() {
                    overflow.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            })
        };
        let armed = watch::arm(
            &path,
            body.recursive,
            self.state.ctx.config().force_poll_watcher,
            sink,
        )
        .map_err(|e| {
            crate::error::unsupported("fs_watch", format!("cannot watch {}: {e}", path.display()))
        })?;
        drop(events_tx);
        let (tx, rx) = mpsc::channel::<Result<WatchDirResponse, Status>>(64);
        tokio::spawn(async move {
            let _armed = armed;
            let wrap = |m| Ok(WatchDirResponse { message: Some(m) });
            if tx
                .send(wrap(WatchMessageProto::Started(WatchStarted {})))
                .await
                .is_err()
            {
                return;
            }
            let mut last = tokio::time::Instant::now();
            loop {
                let message = tokio::select! {
                    message = events_rx.recv() => match message {
                        Some(watch::WatchMessage::Event(e)) => WatchMessageProto::Event(e),
                        Some(watch::WatchMessage::Overflow) => WatchMessageProto::Overflow(WatchOverflow {}),
                        None => return,
                    },
                    _ = tokio::time::sleep_until(last + keepalive) => WatchMessageProto::Keepalive(KeepAlive {}),
                    _ = tx.closed() => return,
                };
                if overflow.swap(false, std::sync::atomic::Ordering::SeqCst)
                    && tx
                        .send(wrap(WatchMessageProto::Overflow(WatchOverflow {})))
                        .await
                        .is_err()
                {
                    return;
                }
                if tx.send(wrap(message)).await.is_err() {
                    return;
                }
                last = tokio::time::Instant::now();
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn create_watcher(
        &self,
        request: Request<CreateWatcherRequest>,
    ) -> Result<Response<CreateWatcherResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| io_status(&e, &path))?;
        if !meta.is_dir() {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::NotADirectory,
                format!("{}: not a directory", path.display()),
            ));
        }
        let watcher = watch::BufferedWatcher::new(
            &path,
            body.recursive,
            self.state.ctx.config().force_poll_watcher,
        )
        .map_err(|e| {
            crate::error::unsupported("fs_watch", format!("cannot watch {}: {e}", path.display()))
        })?;
        let id = format!("w-{}", random_id(12));
        self.state
            .watchers
            .lock()
            .expect("watchers lock")
            .insert(id.clone(), Arc::new(watcher));
        Ok(Response::new(CreateWatcherResponse { watcher_id: id }))
    }

    async fn get_watcher_events(
        &self,
        request: Request<GetWatcherEventsRequest>,
    ) -> Result<Response<GetWatcherEventsResponse>, Status> {
        let body = request.into_inner();
        let watcher = self
            .state
            .watchers
            .lock()
            .expect("watchers lock")
            .get(&body.watcher_id)
            .cloned()
            .ok_or_else(|| session_not_found("watcher", &body.watcher_id))?;
        let (events, overflowed) = watcher.drain(body.max_events as usize);
        Ok(Response::new(GetWatcherEventsResponse {
            events,
            overflowed,
        }))
    }

    async fn remove_watcher(
        &self,
        request: Request<RemoveWatcherRequest>,
    ) -> Result<Response<RemoveWatcherResponse>, Status> {
        let body = request.into_inner();
        self.state
            .watchers
            .lock()
            .expect("watchers lock")
            .remove(&body.watcher_id)
            .ok_or_else(|| session_not_found("watcher", &body.watcher_id))?;
        Ok(Response::new(RemoveWatcherResponse {}))
    }

    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<Self::ReadFileStream>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| io_status(&e, &path))?;
        if meta.is_dir() {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::IsADirectory,
                format!("{}: is a directory", path.display()),
            ));
        }
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| io_status(&e, &path))?;
        if body.offset > 0 {
            file.seek(std::io::SeekFrom::Start(body.offset))
                .await
                .map_err(|e| io_status(&e, &path))?;
        }
        let chunk_size = match body.chunk_size {
            0 => PREFERRED_CHUNK_BYTES,
            n => n.min(MAX_CHUNK_BYTES),
        } as usize;
        let limit = if body.length == 0 {
            u64::MAX
        } else {
            body.length
        };
        let entry = entry_info(&path, &meta);
        let (tx, rx) = mpsc::channel::<Result<ReadFileResponse, Status>>(4);
        tokio::spawn(async move {
            let wrap = |m| Ok(ReadFileResponse { message: Some(m) });
            if tx.send(wrap(ReadMessage::Entry(entry))).await.is_err() {
                return;
            }
            let mut hasher = body.compute_sha256.then(Sha256::new);
            let mut offset = body.offset;
            let mut sent = 0u64;
            while sent < limit {
                let want = (limit - sent).min(chunk_size as u64) as usize;
                let mut buf = vec![0u8; want];
                let mut filled = 0;
                while filled < want {
                    match file.read(&mut buf[filled..]).await {
                        Ok(0) => break,
                        Ok(n) => filled += n,
                        Err(e) => {
                            let _ = tx.send(Err(io_status(&e, &path))).await;
                            return;
                        }
                    }
                }
                if filled == 0 {
                    break;
                }
                buf.truncate(filled);
                if let Some(h) = hasher.as_mut() {
                    h.update(&buf);
                }
                let chunk = FileChunk { offset, data: buf };
                offset += filled as u64;
                sent += filled as u64;
                if tx.send(wrap(ReadMessage::Chunk(chunk))).await.is_err() {
                    return;
                }
                if filled < want {
                    break;
                }
            }
            let _ = tx
                .send(wrap(ReadMessage::End(ReadFileEnd {
                    bytes_read: sent,
                    sha256: hasher.map(|h| hex_digest(h.finalize())).unwrap_or_default(),
                })))
                .await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn write_file(
        &self,
        request: Request<Streaming<WriteFileRequest>>,
    ) -> Result<Response<WriteFileResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let mut stream = request.into_inner();
        let first = stream
            .message()
            .await?
            .ok_or_else(|| crate::error::invalid("empty WriteFile stream"))?;
        let Some(WriteMessage::Header(header)) = first.message else {
            return Err(crate::error::invalid(
                "the first WriteFile message must be a header",
            ));
        };
        let path = self.resolve_as(viewer.as_deref(), &header.path)?;
        let mut writer = AtomicWriter::create(target_for(path, &header)?).await?;
        loop {
            let message = match stream.message().await {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(status) => {
                    writer.abort().await;
                    return Err(status);
                }
            };
            match message.message {
                Some(WriteMessage::Data(data)) => {
                    if let Some(error) = chunk_too_large(data.len(), MAX_CHUNK_BYTES) {
                        writer.abort().await;
                        return Err(error);
                    }
                    if let Err(error) = writer.write(&data).await {
                        writer.abort().await;
                        return Err(error);
                    }
                }
                Some(WriteMessage::Header(_)) => {
                    writer.abort().await;
                    return Err(crate::error::invalid("header may only be sent once"));
                }
                None => {}
            }
        }
        let (entry, sha256) = writer
            .finish(header.expected_size, &header.expected_sha256)
            .await?;
        Ok(Response::new(WriteFileResponse {
            entry: Some(entry),
            sha256,
        }))
    }

    async fn begin_upload(
        &self,
        request: Request<BeginUploadRequest>,
    ) -> Result<Response<BeginUploadResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let header = body
            .header
            .ok_or_else(|| crate::error::invalid("header is required"))?;
        let path = self.resolve_as(viewer.as_deref(), &header.path)?;
        let ttl = duration(body.ttl.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(UPLOAD_DEFAULT_TTL)
            .min(UPLOAD_MAX_TTL);
        if !body.upload_id.is_empty() {
            let existing = self
                .state
                .uploads
                .lock()
                .expect("uploads lock")
                .get(&body.upload_id)
                .cloned();
            if let Some(existing) = existing {
                let mut upload = existing.lock().await;
                if upload.path != path {
                    return Err(StatusBuilder::new(
                        Code::FailedPrecondition,
                        ErrorReason::Unspecified,
                        format!("upload id {:?} is in use for another path", body.upload_id),
                    )
                    .build());
                }
                upload.expires = Instant::now() + upload.ttl;
                let received = upload.writer.as_ref().map(|w| w.written()).unwrap_or(0);
                return Ok(Response::new(BeginUploadResponse {
                    upload_id: body.upload_id,
                    received_bytes: received,
                    max_chunk_bytes: MAX_CHUNK_BYTES,
                    expires_at: Some(timestamp(SystemTime::now() + upload.ttl)),
                }));
            }
        }
        let id = if body.upload_id.is_empty() {
            format!("u-{}", random_id(16))
        } else {
            body.upload_id
        };
        let writer = AtomicWriter::create(target_for(path.clone(), &header)?).await?;
        let upload = Upload {
            path,
            header,
            writer: Some(writer),
            ttl,
            expires: Instant::now() + ttl,
        };
        self.state
            .uploads
            .lock()
            .expect("uploads lock")
            .insert(id.clone(), Arc::new(tokio::sync::Mutex::new(upload)));
        Ok(Response::new(BeginUploadResponse {
            upload_id: id,
            received_bytes: 0,
            max_chunk_bytes: MAX_CHUNK_BYTES,
            expires_at: Some(timestamp(SystemTime::now() + ttl)),
        }))
    }

    async fn upload_chunk(
        &self,
        request: Request<UploadChunkRequest>,
    ) -> Result<Response<UploadChunkResponse>, Status> {
        let body = request.into_inner();
        if let Some(error) = chunk_too_large(body.data.len(), MAX_CHUNK_BYTES) {
            return Err(error);
        }
        let upload = self.state.upload(&body.upload_id)?;
        let mut upload = upload.lock().await;
        let ttl = upload.ttl;
        let writer = upload
            .writer
            .as_mut()
            .ok_or_else(|| session_not_found("upload", &body.upload_id))?;
        let received = writer.written();
        let end = body.offset + body.data.len() as u64;
        let duplicate = if body.offset == received {
            writer.write(&body.data).await?;
            false
        } else if end <= received {
            true
        } else {
            return Err(StatusBuilder::new(
                Code::FailedPrecondition,
                ErrorReason::OffsetMismatch,
                format!("expected offset {received}, got {}", body.offset),
            )
            .meta("expected_offset", received)
            .build());
        };
        let received = writer.written();
        upload.expires = Instant::now() + ttl;
        Ok(Response::new(UploadChunkResponse {
            received_bytes: received,
            duplicate,
            expires_at: Some(timestamp(SystemTime::now() + ttl)),
        }))
    }

    async fn commit_upload(
        &self,
        request: Request<CommitUploadRequest>,
    ) -> Result<Response<CommitUploadResponse>, Status> {
        let body = request.into_inner();
        let upload = self.state.upload(&body.upload_id)?;
        let mut upload = upload.lock().await;
        let writer = upload
            .writer
            .take()
            .ok_or_else(|| session_not_found("upload", &body.upload_id))?;
        self.state
            .uploads
            .lock()
            .expect("uploads lock")
            .remove(&body.upload_id);
        let expected_sha = if body.sha256.is_empty() {
            upload.header.expected_sha256.clone()
        } else {
            body.sha256
        };
        let (entry, sha256) = writer
            .finish(upload.header.expected_size, &expected_sha)
            .await?;
        Ok(Response::new(CommitUploadResponse {
            entry: Some(entry),
            sha256,
        }))
    }

    async fn abort_upload(
        &self,
        request: Request<AbortUploadRequest>,
    ) -> Result<Response<AbortUploadResponse>, Status> {
        let body = request.into_inner();
        let upload = self.state.upload(&body.upload_id)?;
        self.state
            .uploads
            .lock()
            .expect("uploads lock")
            .remove(&body.upload_id);
        if let Some(writer) = upload.lock().await.writer.take() {
            writer.abort().await;
        }
        Ok(Response::new(AbortUploadResponse {}))
    }

    async fn create_signed_url(
        &self,
        request: Request<CreateSignedUrlRequest>,
    ) -> Result<Response<CreateSignedUrlResponse>, Status> {
        let viewer = crate::auth::caller(&request).viewer;
        let body = request.into_inner();
        let path = self.resolve_as(viewer.as_deref(), &body.path)?;
        let method = match SignedUrlMethod::try_from(body.method) {
            Ok(SignedUrlMethod::Get) => "GET",
            Ok(SignedUrlMethod::Put) => "PUT",
            _ => return Err(crate::error::invalid("method must be GET or PUT")),
        };
        let ttl = duration(body.ttl.as_ref())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| crate::error::invalid("ttl is required"))?;
        if ttl > crate::http::files::MAX_SIGNED_URL_TTL {
            return Err(crate::error::invalid("ttl may be at most 24 hours"));
        }
        let signed = crate::http::files::sign(
            &self.state.ctx,
            method,
            &path.display().to_string(),
            SystemTime::now() + ttl,
            &body.content_type,
            &body.download_name,
        );
        Ok(Response::new(CreateSignedUrlResponse {
            url_path: signed.url_path,
            expires_at: Some(timestamp(signed.expires_at)),
        }))
    }
}

#[cfg(test)]
mod remote_mode_tests {
    use super::remote_mode;

    #[test]
    fn remote_writes_keep_only_rwx_bits() {
        assert_eq!(remote_mode(0o4755), 0o755);
        assert_eq!(remote_mode(0o2750), 0o750);
        assert_eq!(remote_mode(0o1777), 0o777);
        assert_eq!(remote_mode(0o644), 0o644);
    }
}
