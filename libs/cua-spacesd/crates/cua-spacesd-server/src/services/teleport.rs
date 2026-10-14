// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `TeleportService`: chunked app-session import and file transfers into
//! the guest's Downloads folder. Replaces the old `:8700` teleport server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_proto::env::v1::teleport_service_server::{TeleportService, TeleportServiceServer};
use cua_proto::env::v1::*;
use cua_spacesd_teleport::ledger::{create_private_dir_all, now_ms};
use cua_spacesd_teleport::{
    conflict_free_path, validate_relative_path, IgnoreRules, Ledger, LedgerStore, Receiver,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tonic::{Code, Request, Response, Status};

use crate::config::MAX_CHUNK_BYTES;
use crate::context::ServerContext;
use crate::error::{io_status, session_not_found, status, StatusBuilder};
use crate::util::{duration, hex_digest, system_time};

/// Default lifetime of a partial import or transfer.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

/// Feature advertised when `WipeImport`, import ledgers and
/// `ImportOptions.expires_at_ms` are supported. Not an app id: consumers
/// that list `teleport.<app>` features must skip it.
pub const WIPE_FEATURE: &str = "teleport.wipe";

/// How often partial uploads and expired ledgers are swept.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

struct Import {
    app: String,
    staging: PathBuf,
    file: tokio::fs::File,
    received: u64,
    hasher: Sha256,
    expires: Instant,
}

struct Transfer {
    root: PathBuf,
    staging: PathBuf,
    accepted: Vec<TransferEntry>,
    ignored: Vec<String>,
    received: Vec<u64>,
    hashers: Vec<Sha256>,
    conflict: ConflictPolicy,
    honor_gitignore: bool,
    ttl: Duration,
    expires: Instant,
}

/// The gRPC service.
#[derive(Clone)]
pub struct TeleportServiceImpl {
    ctx: ServerContext,
    receiver: Arc<Receiver>,
    ledger: LedgerStore,
    imports: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<Import>>>>>,
    transfers: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<Transfer>>>>>,
}

impl TeleportServiceImpl {
    /// Creates the service. `receiver` decides where imports land and how
    /// apps are launched (tests pass a fake host).
    pub fn new(ctx: ServerContext, receiver: Arc<Receiver>) -> Self {
        let ledger_root = ctx
            .config()
            .teleport_ledger_dir
            .clone()
            .unwrap_or_else(|| LedgerStore::default_root(&ctx.config().data_dir));
        let service = Self {
            ctx,
            receiver,
            ledger: LedgerStore::new(ledger_root),
            imports: Arc::default(),
            transfers: Arc::default(),
        };
        let sweeper = service.clone();
        let shutdown = service.ctx.shutdown_token();
        tokio::spawn(async move {
            // The first tick completes immediately: that is the startup
            // sweep, which wipes imports that expired while spacesd (or the
            // guest) was down.
            let mut tick = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                tokio::select! {
                    _ = tick.tick() => sweeper.sweep().await,
                    _ = shutdown.cancelled() => return,
                }
            }
        });
        service
    }

    /// Tonic server.
    pub fn into_server(self) -> TeleportServiceServer<Self> {
        TeleportServiceServer::new(self)
            .max_decoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
    }

    /// App ids this guest can import.
    pub fn supported_apps(&self) -> Vec<String> {
        self.receiver.provider_ids()
    }

    /// Where import ledgers live.
    pub fn ledger_store(&self) -> &LedgerStore {
        &self.ledger
    }

    /// Wipes every ledgered import whose `expires_at_ms` is at or before
    /// `now_ms`. An unreadable ledger is logged and left for the next sweep.
    /// Returns the wiped import ids.
    pub async fn sweep_ledgers(&self, now_ms: u64) -> Vec<String> {
        let store = self.ledger.clone();
        let receiver = self.receiver.clone();
        let swept = tokio::task::spawn_blocking(move || {
            store.sweep_expired(now_ms, &receiver.dest_home, &*receiver.host)
        })
        .await;
        let Ok((wiped, unreadable)) = swept else {
            tracing::warn!("teleport ledger sweep panicked");
            return Vec::new();
        };
        for bad in &unreadable {
            tracing::warn!(
                path = %bad.path.display(),
                error = %bad.error,
                "unreadable teleport ledger left for the next sweep"
            );
        }
        for w in &wiped {
            log_wipe(&w.import_id, &w.report, "expired");
        }
        wiped.into_iter().map(|w| w.import_id).collect()
    }

    async fn sweep(&self) {
        self.sweep_ledgers(now_ms()).await;
        let now = Instant::now();
        let expired: Vec<_> = {
            let imports = self.imports.lock().expect("imports");
            imports.keys().cloned().collect()
        };
        for id in expired {
            let entry = self.imports.lock().expect("imports").get(&id).cloned();
            if let Some(entry) = entry {
                let import = entry.lock().await;
                if import.expires <= now {
                    let _ = tokio::fs::remove_file(&import.staging).await;
                    self.imports.lock().expect("imports").remove(&id);
                }
            }
        }
        let ids: Vec<_> = self
            .transfers
            .lock()
            .expect("transfers")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let entry = self.transfers.lock().expect("transfers").get(&id).cloned();
            if let Some(entry) = entry {
                let transfer = entry.lock().await;
                if transfer.expires <= now {
                    let _ = tokio::fs::remove_dir_all(&transfer.staging).await;
                    self.transfers.lock().expect("transfers").remove(&id);
                }
            }
        }
    }

    fn downloads(&self) -> PathBuf {
        if let Some(dir) = &self.ctx.config().downloads_dir {
            return dir.clone();
        }
        crate::filesystem::PathResolver::new(self.ctx.clone())
            .home()
            .join("Downloads")
    }

    fn staging_dir(&self) -> PathBuf {
        self.ctx.config().data_dir.join("teleport")
    }

    /// Records a committed import in its ledger, merged with an earlier
    /// import under the same id.
    fn write_ledger(
        &self,
        import_id: &str,
        provider: &str,
        record: &cua_spacesd_teleport::ImportRecord,
        expires_at_ms: u64,
    ) -> std::io::Result<()> {
        let mut merged = record.clone();
        if let Some(previous) = self.ledger.read(import_id)? {
            let mut earlier = previous.record();
            earlier.merge(&merged);
            merged = earlier;
        }
        self.ledger.write(&Ledger::new(
            import_id,
            provider,
            &merged,
            expires_at_ms,
            now_ms(),
        ))
    }
}

fn log_wipe(import_id: &str, report: &cua_spacesd_teleport::WipeReport, why: &str) {
    tracing::info!(
        import_id,
        why,
        removed = report.removed_paths.len(),
        keychain_items = report.keychain_items_removed,
        cookie_rows = report.cookie_rows_removed,
        "teleport import wiped"
    );
    for (path, reason) in &report.refused {
        tracing::warn!(import_id, path = %path.display(), reason, "ledgered path refused");
    }
    for error in &report.errors {
        tracing::warn!(import_id, error, "ledgered entry kept for retry");
    }
}

/// `create_new` (O_EXCL), mode 0600: a pre-planted file or symlink at the
/// staging path is refused instead of written through.
async fn create_staging_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).await
}

fn app_on_path(app: &str) -> bool {
    let candidates: &[&str] = match app {
        "chrome" => &["google-chrome", "google-chrome-stable", "chrome"],
        "chromium" => &["chromium", "chromium-browser"],
        "firefox" => &["firefox", "firefox-esr"],
        "vscode" => &["code"],
        other => return which(other),
    };
    candidates.iter().any(|c| which(c))
}

fn which(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(binary).is_file()))
        .unwrap_or(false)
}

fn chunk_check(len: usize) -> Result<(), Status> {
    if len > MAX_CHUNK_BYTES as usize {
        return Err(StatusBuilder::new(
            Code::InvalidArgument,
            ErrorReason::LimitExceeded,
            format!("chunk of {len} bytes exceeds {MAX_CHUNK_BYTES}"),
        )
        .build());
    }
    Ok(())
}

fn offset_mismatch(expected: u64, got: u64) -> Status {
    StatusBuilder::new(
        Code::FailedPrecondition,
        ErrorReason::OffsetMismatch,
        format!("expected offset {expected}, got {got}"),
    )
    .meta("expected_offset", expected)
    .build()
}

#[tonic::async_trait]
impl TeleportService for TeleportServiceImpl {
    async fn get_manifest(
        &self,
        request: Request<GetManifestRequest>,
    ) -> Result<Response<GetManifestResponse>, Status> {
        let body = request.into_inner();
        let apps = self.supported_apps();
        let supported = apps.iter().any(|a| a.eq_ignore_ascii_case(&body.app));
        Ok(Response::new(GetManifestResponse {
            supported,
            limitation: if supported {
                String::new()
            } else {
                format!("no teleport provider for {:?} on this guest", body.app)
            },
            app_installed: supported && app_on_path(&body.app),
            app_running: false,
            bundle_version: cua_spacesd_teleport::BUNDLE_VERSION,
            scopes: if supported {
                vec![TeleportScope::Session as i32, TeleportScope::Profile as i32]
            } else {
                vec![]
            },
            supported_apps: apps,
        }))
    }

    async fn import_session(
        &self,
        request: Request<ImportSessionRequest>,
    ) -> Result<Response<ImportSessionResponse>, Status> {
        let body = request.into_inner();
        if body.import_id.is_empty() {
            return Err(crate::error::invalid("import_id is required"));
        }
        chunk_check(body.data.len())?;
        let existing = self
            .imports
            .lock()
            .expect("imports")
            .get(&body.import_id)
            .cloned();
        let entry = match existing {
            Some(entry) => entry,
            None => {
                if body.offset != 0 {
                    return Err(session_not_found("import", &body.import_id));
                }
                if !self
                    .supported_apps()
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&body.app))
                {
                    return Err(crate::error::unsupported(
                        &format!("teleport.{}", body.app),
                        format!("no teleport provider for {:?}", body.app),
                    ));
                }
                let dir = self.staging_dir();
                let private = dir.clone();
                tokio::task::spawn_blocking(move || create_private_dir_all(&private))
                    .await
                    .map_err(|e| crate::error::internal(e.to_string()))?
                    .map_err(|e| io_status(&e, &dir))?;
                let staging = dir.join(format!("{}.bundle", crate::util::random_id(12)));
                let file = create_staging_file(&staging)
                    .await
                    .map_err(|e| io_status(&e, &staging))?;
                let entry = Arc::new(tokio::sync::Mutex::new(Import {
                    app: body.app.clone(),
                    staging,
                    file,
                    received: 0,
                    hasher: Sha256::new(),
                    expires: Instant::now() + DEFAULT_TTL,
                }));
                self.imports
                    .lock()
                    .expect("imports")
                    .entry(body.import_id.clone())
                    .or_insert(entry)
                    .clone()
            }
        };
        let mut import = entry.lock().await;
        let end = body.offset + body.data.len() as u64;
        let duplicate = if body.offset == import.received {
            let staging = import.staging.clone();
            import
                .file
                .write_all(&body.data)
                .await
                .map_err(|e| io_status(&e, &staging))?;
            import.hasher.update(&body.data);
            import.received = end;
            false
        } else if end <= import.received && !body.commit {
            true
        } else if end == import.received && body.commit {
            // A retried final chunk whose data already landed.
            true
        } else {
            return Err(offset_mismatch(import.received, body.offset));
        };
        import.expires = Instant::now() + DEFAULT_TTL;
        if !body.commit {
            return Ok(Response::new(ImportSessionResponse {
                received_bytes: import.received,
                duplicate,
                result: None,
            }));
        }
        let digest = hex_digest(import.hasher.clone().finalize());
        if body.sha256.is_empty() {
            return Err(crate::error::invalid(
                "sha256 is required on the final chunk",
            ));
        }
        let staging = import.staging.clone();
        if !digest.eq_ignore_ascii_case(&body.sha256) {
            let _ = tokio::fs::remove_file(&staging).await;
            self.imports
                .lock()
                .expect("imports")
                .remove(&body.import_id);
            return Err(StatusBuilder::new(
                Code::FailedPrecondition,
                ErrorReason::ChecksumMismatch,
                format!("bundle sha256 {digest} does not match {}", body.sha256),
            )
            .build());
        }
        import
            .file
            .flush()
            .await
            .map_err(|e| io_status(&e, &staging))?;
        let options = body.options.unwrap_or_default();
        // Opt-in aggregate telemetry: a count per grant state, nothing else.
        crate::telemetry::counters().delivery(&options.broker_grant);
        let receiver = self.receiver.clone();
        let app = import.app.clone();
        let path = staging.clone();
        let import_id = body.import_id.clone();
        let ctx = self.ctx.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            // S1: a sealed delivery (cua_machine_seal) is unsealed here,
            // before the bundle is parsed at all, so the importer below
            // never sees anything but the plaintext bundle it always has.
            // The guest's existing sha256 check (above, on the raw bytes
            // as received) already covers the sealed wire bytes.
            let raw = std::fs::read(&path)?;
            if cua_machine_seal::looks_sealed(&raw) {
                let envelope = cua_machine_seal::SealedEnvelope::from_bytes(&raw).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("sealed delivery: {e}"),
                    )
                })?;
                let Some(keypair) = ctx.machine_seal() else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "sealed delivery: this guest has no sealed-delivery key to open it with",
                    ));
                };
                let plaintext = cua_machine_seal::open(
                    keypair,
                    &import_id,
                    cua_machine_seal::purpose::TELEPORT_BUNDLE,
                    ctx.seal_replay_guard(),
                    &envelope,
                )
                .map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("sealed delivery: {e}"),
                    )
                })?;
                let cursor = std::io::Cursor::new(plaintext.to_vec());
                Ok::<_, std::io::Error>(receiver.import_bundle(cursor, &app, options.launch_after))
            } else {
                let file = std::fs::File::open(&path)?;
                Ok::<_, std::io::Error>(receiver.import_bundle(file, &app, options.launch_after))
            }
        })
        .await
        .map_err(|e| crate::error::internal(e.to_string()))?
        .map_err(|e| io_status(&e, &staging))?;
        let _ = tokio::fs::remove_file(&staging).await;
        self.imports
            .lock()
            .expect("imports")
            .remove(&body.import_id);
        if outcome.is_err() {
            crate::telemetry::counters().import_failed();
        }
        let outcome = outcome.map_err(|e| match e {
            cua_spacesd_teleport::ImportError::InvalidBundle(m) => crate::error::invalid(m),
            cua_spacesd_teleport::ImportError::AppMismatch { .. } => {
                crate::error::invalid(e.to_string())
            }
            cua_spacesd_teleport::ImportError::UnknownProvider(_) => {
                crate::error::unsupported(&format!("teleport.{}", body.app), e.to_string())
            }
            cua_spacesd_teleport::ImportError::Failed(m) => {
                status(Code::Internal, ErrorReason::DeliveryFailed, m)
            }
        })?;
        // Every committed import is ledgered. If that fails the import is
        // undone: a session nobody can wipe must not stay in the guest.
        let ledgered = {
            let service = self.clone();
            let import_id = body.import_id.clone();
            let provider = outcome.provider_id.clone();
            let record = outcome.record.clone();
            let expires = options.expires_at_ms;
            tokio::task::spawn_blocking(move || {
                let result = service.write_ledger(&import_id, &provider, &record, expires);
                if result.is_err() {
                    let _ = service
                        .receiver
                        .wipe(&Ledger::new(&import_id, &provider, &record, 0, 0));
                }
                result
            })
            .await
            .map_err(|e| crate::error::internal(e.to_string()))?
        };
        if let Err(e) = ledgered {
            return Err(crate::error::internal(format!(
                "could not record the import ledger, import undone: {e}"
            )));
        }
        let mut skipped = Vec::new();
        if options.close_running_app {
            skipped.push(
                "close_running_app: not supported by this driver; close the app first".into(),
            );
        }
        for notice in &outcome.notices {
            skipped.push(format!("notice: {notice}"));
        }
        if let Some(why) = &outcome.launch_error {
            skipped.push(format!("launch: {why}"));
        }
        Ok(Response::new(ImportSessionResponse {
            received_bytes: import.received,
            duplicate,
            result: Some(ImportResult {
                imported: outcome.imported,
                skipped,
                launched: outcome.launched,
            }),
        }))
    }

    async fn begin_receive_files(
        &self,
        request: Request<BeginReceiveFilesRequest>,
    ) -> Result<Response<BeginReceiveFilesResponse>, Status> {
        let body = request.into_inner();
        if body.transfer_id.is_empty() {
            return Err(crate::error::invalid("transfer_id is required"));
        }
        let existing = self
            .transfers
            .lock()
            .expect("transfers")
            .get(&body.transfer_id)
            .cloned();
        if let Some(existing) = existing {
            let transfer = existing.lock().await;
            return Ok(Response::new(progress(&body.transfer_id, &transfer)));
        }
        let rules =
            IgnoreRules::from_patterns(&body.ignore_patterns).map_err(crate::error::invalid)?;
        let mut accepted = Vec::new();
        let mut ignored = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for mut entry in body.entries {
            let rel =
                validate_relative_path(&entry.relative_path).map_err(crate::error::invalid)?;
            if rules.is_ignored(&rel, entry.directory) {
                ignored.push(entry.relative_path.clone());
                continue;
            }
            let normalized = rel.to_string_lossy().replace('\\', "/");
            if !seen.insert(normalized.clone()) {
                return Err(crate::error::invalid(format!(
                    "duplicate entry {normalized:?}"
                )));
            }
            if !entry.directory && entry.sha256.len() != 64 {
                return Err(crate::error::invalid(format!(
                    "{normalized:?}: sha256 (64 hex chars) is required for files"
                )));
            }
            entry.relative_path = normalized;
            accepted.push(entry);
        }
        let mut root = self.downloads();
        if !body.destination_subdir.is_empty() {
            root = root.join(
                validate_relative_path(&body.destination_subdir).map_err(crate::error::invalid)?,
            );
        }
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| io_status(&e, &root))?;
        // Staging inside the destination keeps the final renames on one
        // filesystem.
        let staging = root.join(format!(".cua-transfer-{}", crate::util::random_id(9)));
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| io_status(&e, &staging))?;
        let ttl = duration(body.ttl.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(DEFAULT_TTL)
            .min(Duration::from_secs(24 * 3600));
        let count = accepted.len();
        let transfer = Transfer {
            root,
            staging,
            accepted,
            ignored,
            received: vec![0; count],
            hashers: vec![Sha256::new(); count],
            conflict: ConflictPolicy::try_from(body.conflict_policy)
                .unwrap_or(ConflictPolicy::Rename),
            honor_gitignore: body.honor_gitignore,
            ttl,
            expires: Instant::now() + ttl,
        };
        let response = progress(&body.transfer_id, &transfer);
        self.transfers.lock().expect("transfers").insert(
            body.transfer_id,
            Arc::new(tokio::sync::Mutex::new(transfer)),
        );
        Ok(Response::new(response))
    }

    async fn receive_files_chunk(
        &self,
        request: Request<ReceiveFilesChunkRequest>,
    ) -> Result<Response<ReceiveFilesChunkResponse>, Status> {
        let body = request.into_inner();
        chunk_check(body.data.len())?;
        let entry = self
            .transfers
            .lock()
            .expect("transfers")
            .get(&body.transfer_id)
            .cloned()
            .ok_or_else(|| session_not_found("transfer", &body.transfer_id))?;
        let mut transfer = entry.lock().await;
        let index = body.index as usize;
        let file = transfer
            .accepted
            .get(index)
            .cloned()
            .ok_or_else(|| crate::error::invalid(format!("no accepted entry {index}")))?;
        if file.directory {
            return Err(crate::error::invalid("directories carry no content"));
        }
        let received = transfer.received[index];
        let end = body.offset + body.data.len() as u64;
        if end > file.size {
            return Err(StatusBuilder::new(
                Code::InvalidArgument,
                ErrorReason::LimitExceeded,
                format!(
                    "{}: chunk ends at {end}, past the declared size {}",
                    file.relative_path, file.size
                ),
            )
            .build());
        }
        let duplicate = if body.offset == received {
            let path = transfer.staging.join(&file.relative_path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| io_status(&e, parent))?;
            }
            let mut out = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|e| io_status(&e, &path))?;
            out.write_all(&body.data)
                .await
                .map_err(|e| io_status(&e, &path))?;
            out.flush().await.map_err(|e| io_status(&e, &path))?;
            transfer.hashers[index].update(&body.data);
            transfer.received[index] = end;
            false
        } else if end <= received {
            true
        } else {
            return Err(offset_mismatch(received, body.offset));
        };
        transfer.expires = Instant::now() + transfer.ttl;
        Ok(Response::new(ReceiveFilesChunkResponse {
            received_bytes: transfer.received[index],
            duplicate,
        }))
    }

    async fn commit_receive_files(
        &self,
        request: Request<CommitReceiveFilesRequest>,
    ) -> Result<Response<CommitReceiveFilesResponse>, Status> {
        let body = request.into_inner();
        let entry = self
            .transfers
            .lock()
            .expect("transfers")
            .get(&body.transfer_id)
            .cloned()
            .ok_or_else(|| session_not_found("transfer", &body.transfer_id))?;
        let transfer = entry.lock().await;
        // Verify every file first; nothing is published on a mismatch.
        for (index, file) in transfer.accepted.iter().enumerate() {
            if file.directory {
                continue;
            }
            let digest = hex_digest(transfer.hashers[index].clone().finalize());
            if transfer.received[index] != file.size || !digest.eq_ignore_ascii_case(&file.sha256) {
                return Err(StatusBuilder::new(
                    Code::FailedPrecondition,
                    ErrorReason::ChecksumMismatch,
                    format!(
                        "{}: received {} of {} bytes, sha256 {digest}",
                        file.relative_path, transfer.received[index], file.size
                    ),
                )
                .meta("path", &file.relative_path)
                .build());
            }
        }
        let mut rules = IgnoreRules::default();
        if transfer.honor_gitignore {
            for file in &transfer.accepted {
                let rel = Path::new(&file.relative_path);
                if rel.file_name().is_some_and(|n| n == ".gitignore") && !file.directory {
                    let contents = tokio::fs::read_to_string(transfer.staging.join(rel))
                        .await
                        .unwrap_or_default();
                    rules
                        .add_gitignore(rel.parent().unwrap_or(Path::new("")), &contents)
                        .map_err(crate::error::invalid)?;
                }
            }
        }
        let mut files = Vec::new();
        let mut skipped = Vec::new();
        for file in &transfer.accepted {
            let rel = Path::new(&file.relative_path);
            if transfer.honor_gitignore && rules.is_ignored(rel, file.directory) {
                skipped.push(file.relative_path.clone());
                continue;
            }
            let target = transfer.root.join(rel);
            if file.directory {
                tokio::fs::create_dir_all(&target)
                    .await
                    .map_err(|e| io_status(&e, &target))?;
                continue;
            }
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| io_status(&e, parent))?;
            }
            let staged = transfer.staging.join(rel);
            if file.size == 0 && !staged.exists() {
                tokio::fs::write(&staged, b"")
                    .await
                    .map_err(|e| io_status(&e, &staged))?;
            }
            let final_path = if target.exists() {
                match transfer.conflict {
                    ConflictPolicy::Skip => {
                        skipped.push(file.relative_path.clone());
                        continue;
                    }
                    ConflictPolicy::Overwrite => target.clone(),
                    _ => conflict_free_path(&target),
                }
            } else {
                target.clone()
            };
            tokio::fs::rename(&staged, &final_path)
                .await
                .map_err(|e| io_status(&e, &final_path))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = if file.mode == 0 {
                    0o644
                } else {
                    crate::filesystem::remote_mode(file.mode)
                };
                let _ =
                    std::fs::set_permissions(&final_path, std::fs::Permissions::from_mode(mode));
            }
            if let Some(mtime) = &file.modified_at {
                if let Ok(handle) = std::fs::File::options().write(true).open(&final_path) {
                    let _ = handle.set_modified(system_time(mtime));
                }
            }
            files.push(ReceivedFile {
                path: final_path.display().to_string(),
                size: file.size,
                sha256: file.sha256.to_ascii_lowercase(),
            });
        }
        let _ = tokio::fs::remove_dir_all(&transfer.staging).await;
        let destination = transfer.root.display().to_string();
        drop(transfer);
        self.transfers
            .lock()
            .expect("transfers")
            .remove(&body.transfer_id);
        Ok(Response::new(CommitReceiveFilesResponse {
            destination,
            files,
            skipped,
        }))
    }

    async fn wipe_import(
        &self,
        request: Request<WipeImportRequest>,
    ) -> Result<Response<WipeImportResponse>, Status> {
        self.wipe(request.into_inner()).await.map(Response::new)
    }

    async fn abort_receive_files(
        &self,
        request: Request<AbortReceiveFilesRequest>,
    ) -> Result<Response<AbortReceiveFilesResponse>, Status> {
        let body = request.into_inner();
        let entry = self
            .transfers
            .lock()
            .expect("transfers")
            .remove(&body.transfer_id)
            .ok_or_else(|| session_not_found("transfer", &body.transfer_id))?;
        let transfer = entry.lock().await;
        let _ = tokio::fs::remove_dir_all(&transfer.staging).await;
        Ok(Response::new(AbortReceiveFilesResponse {}))
    }
}

impl TeleportServiceImpl {
    /// `WipeImport`: undo one ledgered import, or all of them.
    pub async fn wipe(&self, body: WipeImportRequest) -> Result<WipeImportResponse, Status> {
        if !body.all && body.import_id.is_empty() {
            return Err(crate::error::invalid(
                "import_id is required unless all is set",
            ));
        }
        let service = self.clone();
        let wiped = tokio::task::spawn_blocking(move || {
            let ledgers = if body.all {
                let (ledgers, unreadable) = service.ledger.list();
                for bad in &unreadable {
                    tracing::warn!(
                        path = %bad.path.display(),
                        error = %bad.error,
                        "unreadable teleport ledger left in place"
                    );
                }
                ledgers
            } else {
                match service.ledger.read(&body.import_id) {
                    Ok(Some(ledger)) => vec![ledger],
                    Ok(None) => return Err(session_not_found("import", &body.import_id)),
                    Err(e) => return Err(io_status(&e, service.ledger.root())),
                }
            };
            let mut response = WipeImportResponse::default();
            for ledger in ledgers {
                let report = service
                    .ledger
                    .wipe_and_forget(
                        &ledger,
                        &service.receiver.dest_home,
                        &*service.receiver.host,
                    )
                    .map_err(|e| io_status(&e, service.ledger.root()))?;
                log_wipe(&ledger.import_id, &report, "requested");
                response.wiped_import_ids.push(ledger.import_id.clone());
                response
                    .removed_paths
                    .extend(report.removed_paths.iter().map(|p| p.display().to_string()));
                response.keychain_items_removed += report.keychain_items_removed;
                response.cookie_rows_removed += report.cookie_rows_removed;
            }
            Ok(response)
        })
        .await
        .map_err(|e| crate::error::internal(e.to_string()))??;
        Ok(wiped)
    }
}

fn progress(id: &str, transfer: &Transfer) -> BeginReceiveFilesResponse {
    BeginReceiveFilesResponse {
        transfer_id: id.to_owned(),
        accepted: transfer.accepted.clone(),
        ignored: transfer.ignored.clone(),
        progress: transfer
            .received
            .iter()
            .enumerate()
            .map(|(index, received)| TransferFileProgress {
                index: index as u32,
                received_bytes: *received,
            })
            .collect(),
        max_chunk_bytes: MAX_CHUNK_BYTES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use cua_spacesd_teleport::bundle::BundleWriter;
    use cua_spacesd_teleport::{FakeHost, TransferScope};

    struct Harness {
        service: TeleportServiceImpl,
        ctx: ServerContext,
        home: PathBuf,
        // Read by the Unix-only permission tests.
        #[cfg_attr(not(unix), allow(dead_code))]
        data: PathBuf,
        #[cfg_attr(not(unix), allow(dead_code))]
        ledger: PathBuf,
        _dirs: Vec<tempfile::TempDir>,
    }

    fn harness() -> Harness {
        let data = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ledger = data.path().join("ledger");
        let config = ServerConfig {
            data_dir: data.path().to_path_buf(),
            teleport_home: Some(home.path().to_path_buf()),
            teleport_ledger_dir: Some(ledger.clone()),
            ..ServerConfig::default()
        };
        let ctx = ServerContext::new(config, None);
        let receiver = Arc::new(Receiver::with_host(
            home.path().to_path_buf(),
            Arc::new(FakeHost::new()),
        ));
        Harness {
            service: TeleportServiceImpl::new(ctx.clone(), receiver),
            ctx,
            home: home.path().to_path_buf(),
            data: data.path().to_path_buf(),
            ledger,
            _dirs: vec![data, home],
        }
    }

    fn bundle(tag: &str) -> Vec<u8> {
        let mut w = BundleWriter::new(Vec::new(), "chrome", "Chrome", TransferScope::FullProfile);
        w.add_bytes("tabs.json", 0o644, b"[]").unwrap();
        w.add_bytes(
            format!(".config/google-chrome/{tag}/Cookies"),
            0o600,
            b"cookie-bytes",
        )
        .unwrap();
        w.finish().unwrap()
    }

    async fn import(h: &Harness, id: &str, tag: &str, expires_at_ms: u64) -> ImportResult {
        import_with_grant(h, id, tag, expires_at_ms, "").await
    }

    /// Session deliveries are counted by grant state on arrival (opt-in
    /// aggregate telemetry), and nothing about them is kept.
    #[tokio::test]
    async fn deliveries_are_counted_by_broker_grant_presence() {
        let h = harness();
        let before = crate::telemetry::counters().peek();
        import_with_grant(&h, "imp-absent", "a", 0, "").await;
        import_with_grant(&h, "imp-present", "b", 0, "keyvault").await;
        let after = crate::telemetry::counters().peek();
        assert!(after.grant_absent > before.grant_absent);
        assert!(after.grant_present > before.grant_present);
    }

    async fn import_with_grant(
        h: &Harness,
        id: &str,
        tag: &str,
        expires_at_ms: u64,
        broker_grant: &str,
    ) -> ImportResult {
        let bytes = bundle(tag);
        let sha = hex_digest(Sha256::digest(&bytes));
        let half = bytes.len() / 2;
        let chunk = |offset: usize, end: usize, commit: bool| ImportSessionRequest {
            import_id: id.into(),
            app: "chrome".into(),
            offset: offset as u64,
            data: bytes[offset..end].to_vec(),
            commit,
            sha256: if commit { sha.clone() } else { String::new() },
            options: Some(ImportOptions {
                expires_at_ms,
                broker_grant: broker_grant.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        h.service
            .import_session(Request::new(chunk(0, half, false)))
            .await
            .unwrap();
        h.service
            .import_session(Request::new(chunk(half, bytes.len(), true)))
            .await
            .unwrap()
            .into_inner()
            .result
            .unwrap()
    }

    fn home_files(home: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![home.to_path_buf()];
        let mut budget = 10_000;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                budget -= 1;
                assert!(budget > 0, "runaway walk");
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn wipe_removes_exactly_what_the_import_wrote() {
        let h = harness();
        std::fs::write(h.home.join("preexisting"), b"keep").unwrap();
        import(&h, "imp-1", "Default", 0).await;
        let store = h.service.ledger_store().clone();
        let ledger = store.read("imp-1").unwrap().expect("ledger written");
        assert_eq!(ledger.provider, "chrome");
        assert_eq!(ledger.files.len(), 1);
        assert!(ledger.files[0].ends_with("Default/Cookies"));
        #[cfg(unix)]
        {
            assert_eq!(mode(&h.ledger), 0o700);
            assert_eq!(mode(&store.path_for("imp-1")), 0o600);
        }
        let response = h
            .service
            .wipe_import(Request::new(WipeImportRequest {
                import_id: "imp-1".into(),
                all: false,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.wiped_import_ids, ["imp-1"]);
        assert!(response
            .removed_paths
            .contains(&ledger.files[0].display().to_string()));
        assert_eq!(response.keychain_items_removed, 0);
        assert_eq!(home_files(&h.home), vec![h.home.join("preexisting")]);
        assert_eq!(std::fs::read_dir(&h.home).unwrap().count(), 1);
        assert!(store.read("imp-1").unwrap().is_none(), "ledger removed");
    }

    #[tokio::test]
    async fn unknown_import_is_not_found() {
        let h = harness();
        let error = h
            .service
            .wipe_import(Request::new(WipeImportRequest {
                import_id: "nope".into(),
                all: false,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::NotFound);
        assert_eq!(
            crate::error::error_info(&error).map(|i| i.reason),
            Some(ErrorReason::SessionNotFound as i32)
        );
        let error = h
            .service
            .wipe_import(Request::new(WipeImportRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn wipe_all_wipes_every_ledger() {
        let h = harness();
        import(&h, "a", "A", 0).await;
        import(&h, "b", "B", 0).await;
        let mut response = h
            .service
            .wipe_import(Request::new(WipeImportRequest {
                import_id: String::new(),
                all: true,
            }))
            .await
            .unwrap()
            .into_inner();
        response.wiped_import_ids.sort();
        assert_eq!(response.wiped_import_ids, ["a", "b"]);
        assert!(home_files(&h.home).is_empty());
        assert!(h.service.ledger_store().list().0.is_empty());
    }

    #[tokio::test]
    async fn ttl_sweep_wipes_expired_but_not_live_imports() {
        let h = harness();
        let now = now_ms();
        import(&h, "old", "Old", now + 1_000).await;
        import(&h, "live", "Live", now + 3_600_000).await;
        import(&h, "forever", "Forever", 0).await;
        let wiped = h.service.sweep_ledgers(now + 2_000).await;
        assert_eq!(wiped, ["old"]);
        let files = home_files(&h.home);
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(files.iter().all(|f| !f.to_string_lossy().contains("/Old/")));
        let store = h.service.ledger_store();
        assert!(store.read("old").unwrap().is_none());
        assert!(store.read("live").unwrap().is_some());
        assert!(store.read("forever").unwrap().is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staging_is_private_and_exclusive() {
        let h = harness();
        let bytes = bundle("Default");
        h.service
            .import_session(Request::new(ImportSessionRequest {
                import_id: "s".into(),
                app: "chrome".into(),
                data: bytes[..10].to_vec(),
                ..Default::default()
            }))
            .await
            .unwrap();
        let dir = h.data.join("teleport");
        assert_eq!(mode(&dir), 0o700);
        let staged: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "bundle"))
            .collect();
        assert_eq!(staged.len(), 1);
        assert_eq!(mode(&staged[0]), 0o600);
        // O_EXCL: an existing path (a planted file or symlink) is refused.
        let error = create_staging_file(&staged[0]).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    }

    /// Sends `envelope`'s wire bytes as the whole import (single chunk,
    /// committed) and returns the raw response (so callers can assert on
    /// success or on the exact refusal).
    async fn import_sealed(
        h: &Harness,
        id: &str,
        app: &str,
        envelope: &cua_machine_seal::SealedEnvelope,
    ) -> Result<ImportResult, tonic::Status> {
        let wire = envelope.to_bytes();
        let sha = hex_digest(Sha256::digest(&wire));
        h.service
            .import_session(Request::new(ImportSessionRequest {
                import_id: id.into(),
                app: app.into(),
                offset: 0,
                data: wire,
                commit: true,
                sha256: sha,
                options: Some(ImportOptions::default()),
                ..Default::default()
            }))
            .await
            .map(|r| r.into_inner().result.unwrap())
    }

    #[tokio::test]
    async fn a_sealed_import_is_unsealed_before_parsing() {
        let h = harness();
        let keypair = h.ctx.machine_seal().expect("guest generated a key");
        let bytes = bundle("Default");
        let envelope = cua_machine_seal::seal(
            keypair.public(),
            "sealed-1",
            cua_machine_seal::purpose::TELEPORT_BUNDLE,
            &bytes,
        )
        .unwrap();
        let result = import_sealed(&h, "sealed-1", "chrome", &envelope)
            .await
            .unwrap();
        assert!(!result.imported.is_empty(), "{result:?}");
        // The real, decrypted content landed, exactly like a plaintext
        // import would have: the importer never saw ciphertext.
        assert!(home_files(&h.home)
            .iter()
            .any(|p| p.to_string_lossy().contains("Cookies")));
    }

    #[tokio::test]
    async fn a_sealed_import_with_the_wrong_key_is_refused() {
        let h = harness();
        // Sealed to a key this guest did not generate: it cannot open it.
        let other = cua_machine_seal::MachineKeypair::generate();
        let bytes = bundle("Default");
        let envelope = cua_machine_seal::seal(
            other.public(),
            "sealed-2",
            cua_machine_seal::purpose::TELEPORT_BUNDLE,
            &bytes,
        )
        .unwrap();
        let status = import_sealed(&h, "sealed-2", "chrome", &envelope)
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::InvalidArgument);
        assert!(
            status.message().contains("sealed delivery"),
            "{}",
            status.message()
        );
        // Nothing was imported.
        assert!(home_files(&h.home).is_empty());
    }

    #[tokio::test]
    async fn a_tampered_sealed_import_is_refused() {
        let h = harness();
        let keypair = h.ctx.machine_seal().expect("guest generated a key");
        let bytes = bundle("Default");
        let mut envelope = cua_machine_seal::seal(
            keypair.public(),
            "sealed-3",
            cua_machine_seal::purpose::TELEPORT_BUNDLE,
            &bytes,
        )
        .unwrap();
        // Flip a bit in the ciphertext, then re-derive a wire form whose
        // own sha256 the guest's integrity check will still accept (it
        // checks the bytes it received, not the plaintext): this proves
        // the AEAD tag, not just the outer checksum, guards the content.
        let mut wire = envelope.to_bytes();
        let last = wire.len() - 1;
        wire[last] ^= 1;
        envelope = cua_machine_seal::SealedEnvelope::from_bytes(&wire).unwrap();
        let status = import_sealed(&h, "sealed-3", "chrome", &envelope)
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::InvalidArgument);
        assert!(home_files(&h.home).is_empty());
    }
}
