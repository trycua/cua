//! Encrypted, metadata-only Computer History.
//!
//! History is deliberately separate from trajectory recording.  It accepts
//! only strongly typed, fixed-field events and performs encryption before any
//! event bytes reach disk.  The action path uses `try_send`; storage failure or
//! backpressure can never fail the computer action that produced the event.

use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use coset::{
    iana, AsCborValue, CoseEncrypt0, CoseEncrypt0Builder, HeaderBuilder, TaggedCborSerializable,
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use zeroize::Zeroizing;

use crate::action_record::{
    ActionEffect, ActionExecutionRecord, ActionRoute, ActualDelivery, EscalationKind,
    ProjectedEvidenceKind,
};
use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use async_trait::async_trait;

pub const HISTORY_PROFILE_VERSION: u8 = 1;
pub const HISTORY_SCHEMA_URN: &str = "urn:cua-driver:schema:history-event:v0";
pub const DEFAULT_RETENTION_DAYS: u64 = 7;
pub const DEFAULT_QUOTA_BYTES: u64 = 100 * 1024 * 1024;
const WRITER_QUEUE_CAPACITY: usize = 512;
const MAX_QUERY_LIMIT: usize = 200;
const CHUNK_KEY_INFO: &[u8] = b"cua-driver/history-profile/v1/chunk-key";
const SESSION_ID_KEY_INFO: &[u8] = b"cua-driver/history-profile/v1/session-id-key";
const CHUNK_ROTATION_INTERVAL: Duration = Duration::from_secs(55 * 60);
const WRITER_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);
const ROOT_MARKER_NAME: &str = ".cua-history-root-v1";
const ROOT_MARKER_CONTENT: &[u8] = b"cua-history-root-v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryHealthCategory {
    Ready,
    Disabled,
    Paused,
    NotAdmitted,
    KeyUnavailable,
    KeyLocked,
    KeyCorrupt,
    KeyDestroyFailed,
    StorageUnavailable,
    StorageCorrupt,
    QuotaReached,
    EventsDropped,
    WriterStopped,
}

#[derive(Debug, thiserror::Error)]
#[error("{category:?}")]
pub struct HistoryError {
    pub category: HistoryHealthCategory,
}

impl HistoryError {
    pub fn new(category: HistoryHealthCategory) -> Self {
        Self { category }
    }

    pub fn code(&self) -> &'static str {
        match self.category {
            HistoryHealthCategory::Ready => "ready",
            HistoryHealthCategory::Disabled => "history_disabled",
            HistoryHealthCategory::Paused => "history_paused",
            HistoryHealthCategory::NotAdmitted => "history_preview_not_admitted",
            HistoryHealthCategory::KeyUnavailable => "history_key_unavailable",
            HistoryHealthCategory::KeyLocked => "history_key_locked",
            HistoryHealthCategory::KeyCorrupt => "history_key_corrupt",
            HistoryHealthCategory::KeyDestroyFailed => "history_key_destroy_failed",
            HistoryHealthCategory::StorageUnavailable => "history_storage_unavailable",
            HistoryHealthCategory::StorageCorrupt => "history_storage_corrupt",
            HistoryHealthCategory::QuotaReached => "history_quota_reached",
            HistoryHealthCategory::EventsDropped => "history_events_dropped",
            HistoryHealthCategory::WriterStopped => "history_writer_stopped",
        }
    }
}

/// Zeroizing key material returned only to the encrypted store.
pub struct HistoryKey {
    pub reference: String,
    pub epoch: u64,
    pub bytes: Zeroizing<Vec<u8>>,
}

pub trait KeyProvider: Send + Sync {
    fn load_or_create(&self, namespace: &str) -> Result<HistoryKey, HistoryError>;
    fn load(&self, namespace: &str, reference: &str) -> Result<HistoryKey, HistoryError>;
    fn references(&self, namespace: &str) -> Result<Vec<String>, HistoryError>;
    fn destroy(&self, namespace: &str, reference: &str) -> Result<(), HistoryError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflinePurgeResult {
    pub destroyed_keys: usize,
    pub removed_files: usize,
}

/// Cryptographically purge one exact history namespace while no writer owns it.
/// Keys are destroyed and re-enumerated before any retryable on-disk state is
/// removed, so a partial Keychain failure leaves the ciphertext available for
/// another purge attempt.
pub fn purge_offline(
    root: &Path,
    namespace: &str,
    key_provider: &dyn KeyProvider,
) -> Result<OfflinePurgeResult, HistoryError> {
    prepare_history_root(root)?;
    let _writer_lease = WriterLease::acquire(root)?;
    let references = key_provider.references(namespace)?;
    for reference in &references {
        key_provider.destroy(namespace, reference)?;
    }
    if !key_provider.references(namespace)?.is_empty() {
        return Err(HistoryError::new(HistoryHealthCategory::KeyDestroyFailed));
    }

    let mut removed_files = 0;
    for path in history_chunk_paths(root)? {
        fs::remove_file(path)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        removed_files += 1;
    }
    for name in [
        "state.json",
        "state.json.tmp",
        "admission.json",
        "admission.json.tmp",
    ] {
        let path = root.join(name);
        if path.exists() {
            fs::remove_file(path)
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
            removed_files += 1;
        }
    }
    Ok(OfflinePurgeResult {
        destroyed_keys: references.len(),
        removed_files,
    })
}

/// Establish that `root` is a dedicated Cua History directory before any
/// state write or destructive operation. A new or empty root is claimed by
/// writing an ownership marker. A non-empty unmarked root fails closed, as do
/// symlinked roots or managed entries.
pub fn prepare_history_root(root: &Path) -> Result<(), HistoryError> {
    if !root.is_absolute() {
        return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
    }
    reject_unsafe_root_components(root)?;
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => secure_create_dir(root)?,
        Err(_) => {
            return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
        }
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
    }
    reject_unsafe_root_components(root)?;

    let marker = root.join(ROOT_MARKER_NAME);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || fs::read(&marker).ok().as_deref() != Some(ROOT_MARKER_CONTENT)
            {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let empty = fs::read_dir(root)
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?
                .next()
                .is_none();
            if !empty {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
            let mut file = secure_create_new_file(&marker)?;
            file.write_all(ROOT_MARKER_CONTENT)
                .and_then(|_| file.sync_data())
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        }
        Err(_) => {
            return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
        }
    }
    validate_history_root_layout(root)
}

#[cfg(unix)]
fn reject_unsafe_root_components(root: &Path) -> Result<(), HistoryError> {
    use std::os::unix::fs::MetadataExt;
    for component in root.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        match fs::symlink_metadata(component) {
            // Root-owned compatibility links such as macOS `/var` are outside
            // the user's control. A user-owned link in the storage ancestry
            // is not a stable destination for destructive history commands.
            Ok(metadata) if metadata.file_type().is_symlink() && metadata.uid() != 0 => {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn reject_unsafe_root_components(root: &Path) -> Result<(), HistoryError> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    for component in root.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        match fs::symlink_metadata(component) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 =>
            {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn reject_unsafe_root_components(_root: &Path) -> Result<(), HistoryError> {
    Ok(())
}

fn validate_history_root_layout(root: &Path) -> Result<(), HistoryError> {
    let mut marker_seen = false;
    for entry in fs::read_dir(root)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?
    {
        let entry =
            entry.map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        let kind = entry
            .file_type()
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        if is_reparse_point(&entry.path())? {
            return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
        }
        match name {
            ROOT_MARKER_NAME => {
                marker_seen = true;
                if !kind.is_file()
                    || fs::read(entry.path()).ok().as_deref() != Some(ROOT_MARKER_CONTENT)
                {
                    return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
                }
            }
            "admission.json" | "admission.json.tmp" | "state.json" | "state.json.tmp"
            | "writer.lock" => {
                if !kind.is_file() {
                    return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
                }
            }
            "chunks" => validate_chunks_layout(&entry.path(), kind.is_dir())?,
            _ => {
                return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
            }
        }
    }
    if !marker_seen {
        return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
    }
    Ok(())
}

fn validate_chunks_layout(path: &Path, is_directory: bool) -> Result<(), HistoryError> {
    if !is_directory {
        return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
    }
    for entry in fs::read_dir(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?
    {
        let entry =
            entry.map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        let kind = entry
            .file_type()
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        if is_reparse_point(&entry.path())?
            || !kind.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("cborseq")
        {
            return Err(HistoryError::new(HistoryHealthCategory::StorageUnavailable));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(path: &Path) -> Result<bool, HistoryError> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(not(windows))]
fn is_reparse_point(_path: &Path) -> Result<bool, HistoryError> {
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

pub trait ApplicationIdentityProvider: Send + Sync {
    fn resolve(&self, pid: i64, window_id: Option<u64>) -> Option<ApplicationIdentity>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryPayload {
    Control {
        operation: HistoryControlOperation,
    },
    ActionStarted,
    ActionCompleted {
        effect: String,
        route: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        delivery: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        delivered_count: Option<u32>,
        evidence_kinds: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        escalation_kind: Option<String>,
    },
    Session {
        phase: String,
    },
    Access {
        operation: String,
        returned_events: u32,
    },
    Health {
        category: HistoryHealthCategory,
        count: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryControlOperation {
    Enable,
    Disable,
    Pause,
    Resume,
    Flush,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAccessOperation {
    AgentQuery,
    LocalCli,
}

impl HistoryAccessOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::AgentQuery => "agent_query",
            Self::LocalCli => "local_cli",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEventData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    pub sequence: u64,
    pub platform: String,
    pub process_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub caller_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<ApplicationIdentity>,
    pub payload: HistoryPayload,
}

/// CloudEvents 1.0 JSON envelope used as the plaintext inside COSE_Encrypt0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEvent {
    pub specversion: String,
    pub id: String,
    pub source: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub subject: String,
    pub time: String,
    pub datacontenttype: String,
    pub dataschema: String,
    pub data: HistoryEventData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedState {
    enabled: bool,
    paused: bool,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            enabled: false,
            paused: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryConfig {
    pub root: PathBuf,
    pub namespace: String,
    pub admitted: bool,
    pub platform: String,
    pub retention_days: u64,
    pub quota_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryStatus {
    pub supported: bool,
    pub admitted: bool,
    pub enabled: bool,
    pub paused: bool,
    pub encrypted: bool,
    pub profile: &'static str,
    pub retention_days: u64,
    pub quota_bytes: u64,
    pub bytes_used: u64,
    pub dropped_events: u64,
    pub health: HistoryHealthCategory,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub since_sequence: Option<u64>,
    #[serde(default)]
    pub until_sequence: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PendingHistoryAction {
    action_id: String,
    session_id: Option<String>,
    capability: String,
    application_pid: Option<i64>,
    application_window_id: Option<u64>,
}

enum WriterMessage {
    Event {
        event: HistoryEvent,
        application_pid: Option<i64>,
        application_window_id: Option<u64>,
    },
    Flush(mpsc::Sender<Result<(), HistoryError>>),
    ReadSnapshot {
        retention_days: u64,
        response: mpsc::Sender<Result<Vec<HistoryEvent>, HistoryError>>,
    },
    Shutdown(mpsc::Sender<()>),
}

struct WriterHandle {
    tx: SyncSender<WriterMessage>,
    join: Option<thread::JoinHandle<()>>,
}

impl WriterHandle {
    fn shutdown(mut self) {
        let (tx, rx) = mpsc::channel();
        let _ = self.tx.send(WriterMessage::Shutdown(tx));
        let _ = rx.recv_timeout(Duration::from_secs(5));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn flush_sender(sender: &SyncSender<WriterMessage>) -> Result<(), HistoryError> {
    let (tx, rx) = mpsc::channel();
    sender
        .send(WriterMessage::Flush(tx))
        .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))?
}

pub struct HistoryManager {
    config: HistoryConfig,
    key_provider: Arc<dyn KeyProvider>,
    app_provider: Option<Arc<dyn ApplicationIdentityProvider>>,
    stream_id: String,
    session_id_key: Mutex<Option<Zeroizing<Vec<u8>>>>,
    enabled: AtomicBool,
    paused: AtomicBool,
    dropped_events: Arc<AtomicU64>,
    pending_dropped_events: Arc<AtomicU64>,
    next_sequence: AtomicU64,
    health: Arc<Mutex<HistoryHealthCategory>>,
    writer: Mutex<Option<WriterHandle>>,
    control: Mutex<()>,
}

impl HistoryManager {
    pub fn new(
        config: HistoryConfig,
        key_provider: Arc<dyn KeyProvider>,
        app_provider: Option<Arc<dyn ApplicationIdentityProvider>>,
    ) -> Arc<Self> {
        let persisted = read_state(&config.root).unwrap_or_default();
        let manager = Arc::new(Self {
            stream_id: opaque_id(&format!("{}:{}", config.namespace, uuid::Uuid::new_v4())),
            session_id_key: Mutex::new(None),
            enabled: AtomicBool::new(persisted.enabled && config.admitted),
            paused: AtomicBool::new(persisted.paused),
            config,
            key_provider,
            app_provider,
            dropped_events: Arc::new(AtomicU64::new(0)),
            pending_dropped_events: Arc::new(AtomicU64::new(0)),
            next_sequence: AtomicU64::new(1),
            health: Arc::new(Mutex::new(if persisted.enabled {
                HistoryHealthCategory::WriterStopped
            } else {
                HistoryHealthCategory::Disabled
            })),
            writer: Mutex::new(None),
            control: Mutex::new(()),
        });
        if manager.enabled.load(Ordering::Acquire) {
            match manager.start_writer(None) {
                Ok(()) => {
                    *manager.health.lock().unwrap() = if manager.paused.load(Ordering::Acquire) {
                        HistoryHealthCategory::Paused
                    } else {
                        HistoryHealthCategory::Ready
                    };
                }
                Err(error) => {
                    *manager.health.lock().unwrap() = error.category;
                    manager.enabled.store(false, Ordering::Release);
                }
            }
        } else if !manager.config.admitted && persisted.enabled {
            *manager.health.lock().unwrap() = HistoryHealthCategory::NotAdmitted;
        }
        manager
    }

    pub fn status(&self) -> HistoryStatus {
        HistoryStatus {
            supported: matches!(self.config.platform.as_str(), "macos" | "windows" | "linux"),
            admitted: self.config.admitted,
            enabled: self.enabled.load(Ordering::Acquire),
            paused: self.paused.load(Ordering::Acquire),
            encrypted: true,
            profile: "cua-history-profile-v1/cbor-sequence+cose-encrypt0+cloudevents-json",
            retention_days: self.config.retention_days,
            quota_bytes: self.config.quota_bytes,
            bytes_used: directory_bytes(&self.config.root),
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
            health: *self.health.lock().unwrap(),
        }
    }

    pub fn enable(&self) -> Result<HistoryStatus, HistoryError> {
        let _control = self.control.lock().unwrap();
        if !self.config.admitted {
            return Err(HistoryError::new(HistoryHealthCategory::NotAdmitted));
        }
        if self.enabled.load(Ordering::Acquire) {
            return Ok(self.status());
        }
        self.paused.store(false, Ordering::Release);
        let event = self.control_event(HistoryControlOperation::Enable);
        self.start_writer(Some(event))?;
        if let Err(error) = self.persist_state_values(true, false) {
            self.stop_writer();
            *self.health.lock().unwrap() = error.category;
            return Err(error);
        }
        self.enabled.store(true, Ordering::Release);
        *self.health.lock().unwrap() = HistoryHealthCategory::Ready;
        Ok(self.status())
    }

    pub fn disable(&self) -> Result<HistoryStatus, HistoryError> {
        let _control = self.control.lock().unwrap();
        if self.enabled.load(Ordering::Acquire) {
            // Close the producer gate before persistence or writer shutdown.
            self.enabled.store(false, Ordering::Release);
            if let Err(error) = self.persist_state_values(false, false) {
                self.enabled.store(true, Ordering::Release);
                return Err(error);
            }
            // A poisoned or full writer must never trap the user in an
            // enabled state. The durable disabled state and producer gate are
            // authoritative; the terminal writer health already accounts for
            // any lifecycle marker it cannot append.
            let _ = self.send_and_flush(self.control_event(HistoryControlOperation::Disable));
            self.stop_writer();
        } else {
            self.persist_state_values(false, false)?;
        }
        self.enabled.store(false, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        *self.health.lock().unwrap() = HistoryHealthCategory::Disabled;
        Ok(self.status())
    }

    pub fn pause(&self) -> Result<HistoryStatus, HistoryError> {
        let _control = self.control.lock().unwrap();
        self.require_enabled()?;
        self.persist_state_values(true, true)?;
        self.paused.store(true, Ordering::Release);
        if let Err(error) = self.send_and_flush(self.control_event(HistoryControlOperation::Pause))
        {
            self.paused.store(false, Ordering::Release);
            let _ = self.persist_state_values(true, false);
            return Err(error);
        }
        *self.health.lock().unwrap() = HistoryHealthCategory::Paused;
        Ok(self.status())
    }

    pub fn resume(&self) -> Result<HistoryStatus, HistoryError> {
        let _control = self.control.lock().unwrap();
        self.require_enabled()?;
        self.persist_state_values(true, false)?;
        if let Err(error) = self.send_and_flush(self.control_event(HistoryControlOperation::Resume))
        {
            let _ = self.persist_state_values(true, true);
            return Err(error);
        }
        // Keep producers gated until the lossless resume marker is durable.
        self.paused.store(false, Ordering::Release);
        *self.health.lock().unwrap() = HistoryHealthCategory::Ready;
        Ok(self.status())
    }

    pub fn flush(&self) -> Result<HistoryStatus, HistoryError> {
        let _control = self.control.lock().unwrap();
        self.require_enabled()?;
        self.send_and_flush(self.control_event(HistoryControlOperation::Flush))?;
        Ok(self.status())
    }

    pub fn delete_all(&self) -> Result<HistoryStatus, HistoryError> {
        let _control = self.control.lock().unwrap();
        if self.enabled.load(Ordering::Acquire) {
            self.enabled.store(false, Ordering::Release);
            self.paused.store(false, Ordering::Release);
            // Deletion is the recovery path for an unhealthy store. A failed
            // audit append cannot prevent writer shutdown, key destruction,
            // and removal of the unreadable ciphertext.
            let _ = self.send_and_flush(self.control_event(HistoryControlOperation::Delete));
            self.stop_writer();
        }
        self.enabled.store(false, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        self.persist_state_values(false, false)?;
        let _writer_lease = WriterLease::acquire(&self.config.root)?;
        // The key provider is the authority for namespace key enumeration.
        // Do not parse untrusted chunk headers during an explicit delete: a
        // torn or corrupt first CBOR item must not prevent the user from
        // destroying the keys and removing the unreadable ciphertext.
        let references = self.key_provider.references(&self.config.namespace)?;
        for reference in references {
            if let Err(error) = self
                .key_provider
                .destroy(&self.config.namespace, &reference)
            {
                *self.health.lock().unwrap() = error.category;
                return Err(error);
            }
        }
        if !self
            .key_provider
            .references(&self.config.namespace)?
            .is_empty()
        {
            let error = HistoryError::new(HistoryHealthCategory::KeyDestroyFailed);
            *self.health.lock().unwrap() = error.category;
            return Err(error);
        }
        *self.session_id_key.lock().unwrap() = None;
        if let Err(error) = remove_history_files(&self.config.root) {
            *self.health.lock().unwrap() = error.category;
            return Err(error);
        }
        self.next_sequence.store(1, Ordering::Release);
        *self.health.lock().unwrap() = HistoryHealthCategory::Disabled;
        Ok(self.status())
    }

    pub fn query(
        &self,
        query: HistoryQuery,
        audit_operation: HistoryAccessOperation,
    ) -> Result<Vec<HistoryEvent>, HistoryError> {
        let _control = self.control.lock().unwrap();
        self.require_admitted()?;
        // The active writer performs the checkpoint and read as one queued
        // operation, so it cannot append a partial item while scanning. Keep
        // the handle lock while waiting; producers use try_lock and drop rather
        // than blocking behind the query.
        let writer_guard = self.writer.lock().unwrap();
        let writer_active = writer_guard.is_some();
        let (mut events, one_shot_lease) = if let Some(writer) = writer_guard.as_ref() {
            (
                read_snapshot_sender(&writer.tx, self.config.retention_days)?,
                None,
            )
        } else {
            let lease = WriterLease::acquire(&self.config.root)?;
            prune_expired_chunks(&self.config.root, self.config.retention_days)?;
            (
                HistoryStore::read_all(
                    &self.config.root,
                    &self.config.namespace,
                    self.key_provider.as_ref(),
                    self.config.quota_bytes,
                )?,
                Some(lease),
            )
        };
        drop(writer_guard);
        let next_sequence = events
            .iter()
            .map(|event| event.data.sequence)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.next_sequence
            .fetch_max(next_sequence, Ordering::Relaxed);
        let session = query
            .session_id
            .as_deref()
            .map(|value| self.session_id(value))
            .transpose()?;
        let retention_cutoff = OffsetDateTime::now_utc()
            - time::Duration::seconds(
                i64::try_from(self.config.retention_days.saturating_mul(24 * 60 * 60))
                    .unwrap_or(i64::MAX),
            );
        events.retain(|event| {
            self.config.retention_days > 0
                && OffsetDateTime::parse(&event.time, &Rfc3339)
                    .is_ok_and(|event_time| event_time >= retention_cutoff)
                && session.as_ref().is_none_or(|session| {
                    event.data.session_id.as_ref() == Some(session)
                        || event.data.session_id.as_deref() == query.session_id.as_deref()
                })
                && query
                    .since_sequence
                    .is_none_or(|since| event.data.sequence >= since)
                && query
                    .until_sequence
                    .is_none_or(|until| event.data.sequence <= until)
        });
        let limit = query.limit.unwrap_or(50).clamp(1, MAX_QUERY_LIMIT);
        if events.len() > limit {
            events.drain(0..events.len() - limit);
        }
        if events.is_empty() {
            return Ok(events);
        }
        let mut audit = self.event(
            "cua-driver.history.access.v0",
            "access",
            None,
            None,
            None,
            HistoryPayload::Access {
                operation: audit_operation.as_str().to_owned(),
                returned_events: events.len() as u32,
            },
        );
        if writer_active {
            self.send_and_flush(audit)?;
        } else {
            let mut store = HistoryStore::create_with_lease(
                &self.config.root,
                &self.config.namespace,
                &self.stream_id,
                self.config.quota_bytes,
                self.key_provider.as_ref(),
                one_shot_lease.expect("disabled query acquired a writer lease"),
            )?;
            audit.data.sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
            store.append(&audit)?;
            store.flush()?;
        }
        Ok(events)
    }

    pub fn begin_action(
        &self,
        tool_name: &str,
        args: &Value,
        implicit_session: Option<&str>,
    ) -> Option<PendingHistoryAction> {
        if !self.is_capturing() || !crate::action_record::is_action_tool(tool_name) {
            return None;
        }
        let action_id = opaque_id(&uuid::Uuid::new_v4().to_string());
        let session_id = args
            .get("session")
            .and_then(Value::as_str)
            .or(implicit_session)
            .filter(|value| !value.is_empty())
            .and_then(|value| self.session_id(value).ok());
        let capability = crate::tool::default_capabilities_for(tool_name)
            .into_iter()
            .next()
            .unwrap_or_else(|| format!("tool.{tool_name}"));
        let application_pid = args
            .get("pid")
            .and_then(Value::as_i64)
            .filter(|pid| *pid > 0);
        let application_window_id = args.get("window_id").and_then(Value::as_u64);
        let pending = PendingHistoryAction {
            action_id,
            session_id,
            capability,
            application_pid,
            application_window_id,
        };
        self.try_send_with_application_pid(
            self.event(
                "cua-driver.history.action_started.v0",
                &format!("action/{}", pending.action_id),
                pending.session_id.clone(),
                Some(pending.action_id.clone()),
                Some(pending.capability.clone()),
                HistoryPayload::ActionStarted,
            ),
            pending.application_pid,
            pending.application_window_id,
        );
        Some(pending)
    }

    pub fn finish_action(
        &self,
        pending: PendingHistoryAction,
        record: Option<&ActionExecutionRecord>,
        failed: bool,
    ) {
        // A pending action owns its admission decision. Pause suppresses new
        // begins, but must not split an already-started action pair. Disable
        // still closes the writer and therefore drops the completion safely.
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let payload = match record.and_then(|record| record.stable_projection().ok()) {
            Some(projection) => HistoryPayload::ActionCompleted {
                effect: effect_name(projection.effect).to_owned(),
                route: route_name(projection.route).to_owned(),
                delivery: projection
                    .delivery
                    .as_ref()
                    .map(|delivery| delivery_name(delivery.actual).to_owned()),
                delivered_count: projection
                    .delivery
                    .and_then(|delivery| delivery.delivered_count),
                evidence_kinds: projection
                    .evidence
                    .unwrap_or_default()
                    .into_iter()
                    .map(|evidence| evidence_name(evidence.kind).to_owned())
                    .collect(),
                escalation_kind: projection
                    .escalation
                    .map(|escalation| escalation_name(escalation.kind).to_owned()),
            },
            None => HistoryPayload::ActionCompleted {
                effect: if failed { "failed" } else { "unverifiable" }.to_owned(),
                route: "unknown".to_owned(),
                delivery: None,
                delivered_count: None,
                evidence_kinds: Vec::new(),
                escalation_kind: None,
            },
        };
        self.try_send_with_application_pid(
            self.event(
                "cua-driver.history.action_completed.v0",
                &format!("action/{}", pending.action_id),
                pending.session_id,
                Some(pending.action_id),
                Some(pending.capability),
                payload,
            ),
            pending.application_pid,
            pending.application_window_id,
        );
    }

    pub fn session_event(&self, session: Option<&str>, started: bool) {
        if !self.is_capturing() {
            return;
        }
        let session_id = session
            .filter(|value| !value.is_empty())
            .and_then(|value| self.session_id(value).ok());
        self.try_send(self.event(
            if started {
                "cua-driver.history.session_started.v0"
            } else {
                "cua-driver.history.session_ended.v0"
            },
            "session",
            session_id,
            None,
            None,
            HistoryPayload::Session {
                phase: if started { "started" } else { "ended" }.to_owned(),
            },
        ));
    }

    fn is_capturing(&self) -> bool {
        self.config.admitted
            && self.enabled.load(Ordering::Acquire)
            && !self.paused.load(Ordering::Acquire)
    }

    fn require_enabled(&self) -> Result<(), HistoryError> {
        self.require_admitted()?;
        if !self.enabled.load(Ordering::Acquire) {
            return Err(HistoryError::new(HistoryHealthCategory::Disabled));
        }
        Ok(())
    }

    fn require_admitted(&self) -> Result<(), HistoryError> {
        if !self.config.admitted {
            return Err(HistoryError::new(HistoryHealthCategory::NotAdmitted));
        }
        Ok(())
    }

    fn start_writer(&self, first_event: Option<HistoryEvent>) -> Result<(), HistoryError> {
        let mut slot = self.writer.lock().unwrap();
        if slot.is_some() {
            return Ok(());
        }
        self.ensure_session_id_key()?;
        let writer_lease = WriterLease::acquire(&self.config.root)?;
        prune_expired_chunks(&self.config.root, self.config.retention_days)?;
        let existing = HistoryStore::read_all(
            &self.config.root,
            &self.config.namespace,
            self.key_provider.as_ref(),
            self.config.quota_bytes,
        )?;
        let max_sequence = existing
            .iter()
            .map(|event| event.data.sequence)
            .max()
            .unwrap_or(0);
        let mut store = HistoryStore::create_with_lease(
            &self.config.root,
            &self.config.namespace,
            &self.stream_id,
            self.config.quota_bytes,
            self.key_provider.as_ref(),
            writer_lease,
        )?;
        self.next_sequence
            .store(max_sequence.saturating_add(1), Ordering::Release);
        if let Some(mut event) = first_event {
            event.data.sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
            validate_event(&event)?;
            store.append(&event)?;
            let verified = store.read_events()?;
            if verified.last().map(|event| &event.id) != Some(&event.id) {
                return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
            }
        }
        let (tx, rx) = mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let thread_health = self.health.clone();
        let thread_dropped_events = self.dropped_events.clone();
        let thread_pending_dropped_events = self.pending_dropped_events.clone();
        let thread_app_provider = self.app_provider.clone();
        let thread_key_provider = self.key_provider.clone();
        let thread_namespace = self.config.namespace.clone();
        let quota_bytes = self.config.quota_bytes;
        let retention_days = self.config.retention_days;
        let join = thread::Builder::new()
            .name("cua-history-writer".to_owned())
            .spawn(move || {
                writer_loop(
                    &mut store,
                    rx,
                    thread_health,
                    thread_dropped_events,
                    thread_pending_dropped_events,
                    thread_app_provider,
                    thread_key_provider,
                    thread_namespace,
                    quota_bytes,
                    retention_days,
                )
            })
            .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))?;
        *slot = Some(WriterHandle {
            tx,
            join: Some(join),
        });
        Ok(())
    }

    fn stop_writer(&self) {
        if let Some(writer) = self.writer.lock().unwrap().take() {
            writer.shutdown();
        }
    }

    fn send_lossless(&self, mut event: HistoryEvent) -> Result<(), HistoryError> {
        validate_event(&event)?;
        let writer = self.writer.lock().unwrap();
        let tx = writer
            .as_ref()
            .map(|writer| &writer.tx)
            .ok_or_else(|| HistoryError::new(HistoryHealthCategory::WriterStopped))?;
        event.data.sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        tx.send(WriterMessage::Event {
            event,
            application_pid: None,
            application_window_id: None,
        })
        .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))
    }

    fn flush_writer(&self) -> Result<(), HistoryError> {
        let sender = self
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .map(|writer| writer.tx.clone())
            .ok_or_else(|| HistoryError::new(HistoryHealthCategory::WriterStopped))?;
        flush_sender(&sender)
    }

    fn send_and_flush(&self, event: HistoryEvent) -> Result<(), HistoryError> {
        self.send_lossless(event)?;
        self.flush_writer()
    }

    fn try_send(&self, event: HistoryEvent) {
        self.try_send_with_application_pid(event, None, None);
    }

    fn try_send_with_application_pid(
        &self,
        mut event: HistoryEvent,
        application_pid: Option<i64>,
        application_window_id: Option<u64>,
    ) {
        if validate_event(&event).is_err() {
            self.record_drop();
            return;
        }
        let Ok(writer) = self.writer.try_lock() else {
            self.record_drop();
            return;
        };
        let tx = writer.as_ref().map(|writer| &writer.tx);
        let Some(tx) = tx else {
            self.record_drop();
            return;
        };
        let pending = self.pending_dropped_events.swap(0, Ordering::AcqRel);
        if pending > 0 {
            let health = self.event(
                "cua-driver.history.health.v0",
                "writer",
                None,
                None,
                None,
                HistoryPayload::Health {
                    category: HistoryHealthCategory::EventsDropped,
                    count: pending,
                },
            );
            let mut health = health;
            health.data.sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
            if tx
                .try_send(WriterMessage::Event {
                    event: health,
                    application_pid: None,
                    application_window_id: None,
                })
                .is_err()
            {
                self.pending_dropped_events
                    .fetch_add(pending, Ordering::Relaxed);
            }
        }
        event.data.sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        if matches!(
            tx.try_send(WriterMessage::Event {
                event,
                application_pid,
                application_window_id,
            }),
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_))
        ) {
            self.record_drop();
        }
    }

    fn record_drop(&self) {
        self.dropped_events.fetch_add(1, Ordering::Relaxed);
        self.pending_dropped_events.fetch_add(1, Ordering::Relaxed);
    }

    fn ensure_session_id_key(&self) -> Result<(), HistoryError> {
        let mut cached = self.session_id_key.lock().unwrap();
        if cached.is_none() {
            let namespace_key = self.key_provider.load_or_create(&self.config.namespace)?;
            if namespace_key.bytes.len() != 32 {
                return Err(HistoryError::new(HistoryHealthCategory::KeyCorrupt));
            }
            let hkdf = Hkdf::<Sha256>::new(None, namespace_key.bytes.as_slice());
            let mut derived = Zeroizing::new(vec![0_u8; 32]);
            hkdf.expand(SESSION_ID_KEY_INFO, &mut derived)
                .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyCorrupt))?;
            *cached = Some(derived);
        }
        Ok(())
    }

    fn session_id(&self, value: &str) -> Result<String, HistoryError> {
        self.ensure_session_id_key()?;
        let key = self.session_id_key.lock().unwrap();
        let key = key
            .as_ref()
            .ok_or_else(|| HistoryError::new(HistoryHealthCategory::KeyUnavailable))?;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_slice())
            .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyCorrupt))?;
        mac.update(value.as_bytes());
        Ok(hex_128(&mac.finalize().into_bytes()[..16]))
    }

    fn control_event(&self, operation: HistoryControlOperation) -> HistoryEvent {
        self.event(
            "cua-driver.history.control.v0",
            "control",
            None,
            None,
            None,
            HistoryPayload::Control { operation },
        )
    }

    fn event(
        &self,
        event_type: &str,
        subject: &str,
        session_id: Option<String>,
        action_id: Option<String>,
        capability: Option<String>,
        payload: HistoryPayload,
    ) -> HistoryEvent {
        HistoryEvent {
            specversion: "1.0".to_owned(),
            id: opaque_id(&uuid::Uuid::new_v4().to_string()),
            source: format!("urn:cua-driver:history:{}", self.stream_id),
            event_type: event_type.to_owned(),
            subject: subject.to_owned(),
            time: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned()),
            datacontenttype: "application/json".to_owned(),
            dataschema: HISTORY_SCHEMA_URN.to_owned(),
            data: HistoryEventData {
                session_id,
                action_id,
                // Persistence paths replace this placeholder while serializing
                // enqueue order. Assigning here would let concurrent callers
                // reserve sequence numbers and enqueue them out of order.
                sequence: 1,
                platform: self.config.platform.clone(),
                process_model: "in_daemon".to_owned(),
                capability,
                caller_category: "cua_runtime".to_owned(),
                application: None,
                payload,
            },
        }
    }

    fn persist_state_values(&self, enabled: bool, paused: bool) -> Result<(), HistoryError> {
        write_state(&self.config.root, &PersistedState { enabled, paused })
    }
}

pub struct HistoryStatusTool {
    manager: Arc<HistoryManager>,
}

static HISTORY_STATUS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

impl HistoryStatusTool {
    pub fn new(manager: Arc<HistoryManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for HistoryStatusTool {
    fn def(&self) -> &ToolDef {
        HISTORY_STATUS_DEF.get_or_init(|| ToolDef {
            name: "history_status".to_owned(),
            description: "For prompts to continue or revisit recent work, call this first, before broad desktop discovery. Reports whether encrypted local history is admitted, enabled, healthy, paused, or dropping events. If history is absent or access is denied, continue with normal discovery. Returns metadata-only status, never history entries, screen or page content, geometry, or inferred user intent, and does not change history lifecycle state.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let status = serde_json::to_value(self.manager.status())
            .expect("HistoryStatus contains only infallibly serializable fields");
        ToolResult::text("Computer History status returned as structured metadata.")
            .with_structured(status)
    }
}

pub struct HistoryQueryTool {
    manager: Arc<HistoryManager>,
}

static HISTORY_QUERY_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

impl HistoryQueryTool {
    pub fn new(manager: Arc<HistoryManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for HistoryQueryTool {
    fn def(&self) -> &ToolDef {
        HISTORY_QUERY_DEF.get_or_init(|| ToolDef {
            name: "history_query".to_owned(),
            description: "After history_status reports ready for a prompt to continue or revisit recent work, make one bounded initial query before broad desktop discovery. Reads metadata-only encrypted local history; it omits screen and page content, geometry, and inferred user intent. Results may enter model context. Requires the existing permission and capability-manifest checks for this exact operation; if history is absent or access is denied, continue with normal discovery. This read does not enable, pause, resume, disable, or delete history.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_QUERY_LIMIT},
                    "session_id": {"type": "string", "minLength": 1, "maxLength": 128},
                    "since_sequence": {"type": "integer", "minimum": 1},
                    "until_sequence": {"type": "integer", "minimum": 1}
                },
                "additionalProperties": false
            }),
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        })
    }

    async fn invoke(&self, mut args: Value) -> ToolResult {
        // Authorization deliberately reattaches trusted runtime/session
        // metadata before dispatch. It is not part of history_query's public
        // closed schema, so remove only reserved adapter fields before the
        // deny_unknown_fields deserialization below.
        crate::tool_args::sanitize_reserved_args(&mut args);
        let query: HistoryQuery = match serde_json::from_value(args) {
            Ok(query) => query,
            Err(_) => {
                return ToolResult::error("history_query arguments are invalid")
                    .with_structured(serde_json::json!({"code": "invalid_history_query"}))
            }
        };
        if query
            .since_sequence
            .zip(query.until_sequence)
            .is_some_and(|(since, until)| since > until)
        {
            return ToolResult::error("since_sequence must not exceed until_sequence")
                .with_structured(serde_json::json!({"code": "invalid_history_query_range"}));
        }
        match self
            .manager
            .query(query, HistoryAccessOperation::AgentQuery)
        {
            Ok(events) => ToolResult::text(format!(
                "Returned {} encrypted Computer History metadata event(s).",
                events.len()
            ))
            .with_structured(serde_json::json!({
                "events": events,
                "metadata_only": true,
                "model_context_disclosure": true
            })),
            Err(error) => {
                ToolResult::error(format!("Computer History query refused: {}", error.code()))
                    .with_structured(serde_json::json!({"code": error.code()}))
            }
        }
    }
}

impl Drop for HistoryManager {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.get_mut().ok().and_then(Option::take) {
            writer.shutdown();
        }
    }
}

fn writer_loop(
    store: &mut HistoryStore,
    rx: mpsc::Receiver<WriterMessage>,
    health: Arc<Mutex<HistoryHealthCategory>>,
    dropped_events: Arc<AtomicU64>,
    pending_dropped_events: Arc<AtomicU64>,
    app_provider: Option<Arc<dyn ApplicationIdentityProvider>>,
    key_provider: Arc<dyn KeyProvider>,
    namespace: String,
    quota_bytes: u64,
    retention_days: u64,
) {
    let mut terminal_error = None;
    let mut last_maintenance = Instant::now();
    loop {
        let message = match rx.recv_timeout(WRITER_MAINTENANCE_INTERVAL) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if terminal_error.is_none() {
                    if let Err(error) = store.checkpoint_retention(retention_days) {
                        terminal_error = Some(error.category);
                        *health.lock().unwrap() = error.category;
                    }
                    last_maintenance = Instant::now();
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match message {
            WriterMessage::Event {
                mut event,
                application_pid,
                application_window_id,
            } => {
                if terminal_error.is_none() {
                    let maintenance_due = retention_days == 0
                        || store.created_at.elapsed() >= CHUNK_ROTATION_INTERVAL
                        || last_maintenance.elapsed() >= WRITER_MAINTENANCE_INTERVAL;
                    if maintenance_due {
                        if let Err(error) = store.checkpoint_retention(retention_days) {
                            dropped_events.fetch_add(1, Ordering::Relaxed);
                            pending_dropped_events.fetch_add(1, Ordering::Relaxed);
                            terminal_error = Some(error.category);
                            *health.lock().unwrap() = error.category;
                            continue;
                        }
                    }
                    if maintenance_due {
                        last_maintenance = Instant::now();
                    }
                    if let Some(pid) = application_pid {
                        event.data.application = app_provider
                            .as_ref()
                            .and_then(|provider| provider.resolve(pid, application_window_id))
                            .and_then(sanitize_application_identity);
                    }
                    if let Err(error) = store.append(&event) {
                        dropped_events.fetch_add(1, Ordering::Relaxed);
                        pending_dropped_events.fetch_add(1, Ordering::Relaxed);
                        terminal_error = Some(error.category);
                        *health.lock().unwrap() = error.category;
                    }
                } else {
                    dropped_events.fetch_add(1, Ordering::Relaxed);
                    pending_dropped_events.fetch_add(1, Ordering::Relaxed);
                }
            }
            WriterMessage::Flush(response) => {
                let result = terminal_error
                    .map(|category| Err(HistoryError::new(category)))
                    .unwrap_or_else(|| store.flush());
                let _ = response.send(result);
            }
            WriterMessage::ReadSnapshot {
                retention_days,
                response,
            } => {
                let result = terminal_error
                    .map(|category| Err(HistoryError::new(category)))
                    .unwrap_or_else(|| {
                        store.checkpoint_retention(retention_days)?;
                        HistoryStore::read_all(
                            &store.root,
                            &namespace,
                            key_provider.as_ref(),
                            quota_bytes,
                        )
                    });
                let _ = response.send(result);
            }
            WriterMessage::Shutdown(response) => {
                let _ = store.flush();
                let _ = response.send(());
                break;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoncePrefix([u8; 4]);

impl NoncePrefix {
    fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }

    fn as_mut_bytes(&mut self) -> &mut [u8; 4] {
        &mut self.0
    }
}

impl Serialize for NoncePrefix {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for NoncePrefix {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct NoncePrefixVisitor;

        impl<'de> serde::de::Visitor<'de> for NoncePrefixVisitor {
            type Value = NoncePrefix;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a four-byte nonce prefix")
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let bytes: [u8; 4] = value
                    .try_into()
                    .map_err(|_| E::invalid_length(value.len(), &self))?;
                Ok(NoncePrefix(bytes))
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_bytes(&value)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = [0_u8; 4];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = sequence
                        .next_element()?
                        .ok_or_else(|| serde::de::Error::invalid_length(index, &self))?;
                }
                if sequence.next_element::<u8>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(5, &self));
                }
                Ok(NoncePrefix(bytes))
            }
        }

        deserializer.deserialize_bytes(NoncePrefixVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChunkHeader(u8, i64, u64, String, String, String, NoncePrefix);

struct WriterLease {
    _file: File,
}

impl WriterLease {
    fn acquire(root: &Path) -> Result<Self, HistoryError> {
        prepare_history_root(root)?;
        let file = secure_open_lock_file(&root.join("writer.lock"))?;
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))?;
        Ok(Self { _file: file })
    }
}

struct HistoryStore {
    root: PathBuf,
    path: PathBuf,
    file: File,
    header: ChunkHeader,
    header_bytes: Vec<u8>,
    key: Zeroizing<Vec<u8>>,
    next_nonce_counter: u64,
    quota_bytes: u64,
    created_at: Instant,
    _writer_lease: WriterLease,
}

impl HistoryStore {
    fn create_with_lease(
        root: &Path,
        namespace: &str,
        stream_id: &str,
        quota_bytes: u64,
        key_provider: &dyn KeyProvider,
        writer_lease: WriterLease,
    ) -> Result<Self, HistoryError> {
        secure_create_dir(root)?;
        let chunks = chunks_dir(root);
        secure_create_dir(&chunks)?;
        let key = key_provider.load_or_create(namespace)?;
        if key.bytes.len() != 32 {
            return Err(HistoryError::new(HistoryHealthCategory::KeyCorrupt));
        }
        let chunk_id = opaque_id(&uuid::Uuid::new_v4().to_string());
        let mut prefix = [0_u8; 4];
        getrandom::fill(&mut prefix)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyUnavailable))?;
        let header = ChunkHeader(
            HISTORY_PROFILE_VERSION,
            iana::Algorithm::ChaCha20Poly1305 as i64,
            key.epoch,
            key.reference.clone(),
            stream_id.to_owned(),
            chunk_id.clone(),
            NoncePrefix(prefix),
        );
        let mut header_bytes = Vec::new();
        ciborium::ser::into_writer(&header, &mut header_bytes)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        if directory_bytes(root).saturating_add(header_bytes.len() as u64) > quota_bytes {
            return Err(HistoryError::new(HistoryHealthCategory::QuotaReached));
        }
        let path = chunks.join(format!("{chunk_id}.cborseq"));
        let mut file = secure_create_new_file(&path)?;
        file.write_all(&header_bytes)
            .and_then(|_| file.sync_data())
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        Ok(Self {
            root: root.to_path_buf(),
            path,
            file,
            header,
            header_bytes,
            key: key.bytes,
            next_nonce_counter: 0,
            quota_bytes,
            created_at: Instant::now(),
            _writer_lease: writer_lease,
        })
    }

    fn append(&mut self, event: &HistoryEvent) -> Result<(), HistoryError> {
        validate_event(event)?;
        if directory_bytes(&self.root) >= self.quota_bytes {
            return Err(HistoryError::new(HistoryHealthCategory::QuotaReached));
        }
        let plaintext = serde_json::to_vec(event)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        let chunk_key = derive_chunk_key(&self.key, &self.header)?;
        let mut nonce = [0_u8; 12];
        nonce[..4].copy_from_slice(self.header.6.as_bytes());
        nonce[4..].copy_from_slice(&self.next_nonce_counter.to_be_bytes());
        self.next_nonce_counter = self
            .next_nonce_counter
            .checked_add(1)
            .ok_or_else(|| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        let cipher_key = Key::try_from(chunk_key.as_slice())
            .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyCorrupt))?;
        let cipher = ChaCha20Poly1305::new(&cipher_key);
        let protected = HeaderBuilder::new()
            .algorithm(iana::Algorithm::ChaCha20Poly1305)
            .build();
        let unprotected = HeaderBuilder::new().iv(nonce.to_vec()).build();
        let encrypted = CoseEncrypt0Builder::new()
            .protected(protected)
            .unprotected(unprotected)
            .try_create_ciphertext(&plaintext, &self.header_bytes, |plaintext, aad| {
                let nonce =
                    Nonce::try_from(nonce.as_slice()).map_err(|_| chacha20poly1305::Error)?;
                cipher.encrypt(
                    &nonce,
                    Payload {
                        msg: plaintext,
                        aad,
                    },
                )
            })
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?
            .build();
        let bytes = encrypted
            .to_tagged_vec()
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        if directory_bytes(&self.root).saturating_add(bytes.len() as u64) > self.quota_bytes {
            return Err(HistoryError::new(HistoryHealthCategory::QuotaReached));
        }
        self.file
            .write_all(&bytes)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
    }

    fn flush(&mut self) -> Result<(), HistoryError> {
        self.file
            .flush()
            .and_then(|_| self.file.sync_data())
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
    }

    fn checkpoint_retention(&mut self, retention_days: u64) -> Result<(), HistoryError> {
        self.flush()?;
        let rotation_due = self.next_nonce_counter > 0
            && (retention_days == 0 || self.created_at.elapsed() >= CHUNK_ROTATION_INTERVAL);
        if rotation_due {
            let chunk_id = opaque_id(&uuid::Uuid::new_v4().to_string());
            getrandom::fill(self.header.6.as_mut_bytes())
                .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyUnavailable))?;
            self.header.5 = chunk_id.clone();
            self.header_bytes.clear();
            ciborium::ser::into_writer(&self.header, &mut self.header_bytes)
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
            self.path = chunks_dir(&self.root).join(format!("{chunk_id}.cborseq"));
            self.file = secure_create_new_file(&self.path)?;
            self.file
                .write_all(&self.header_bytes)
                .and_then(|_| self.file.sync_data())
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
            self.next_nonce_counter = 0;
            self.created_at = Instant::now();
        }
        prune_expired_chunks_except(&self.root, retention_days, Some(&self.path))
    }

    fn read_events(&mut self) -> Result<Vec<HistoryEvent>, HistoryError> {
        self.flush()?;
        let bytes = read_bounded(&self.path, self.quota_bytes)?;
        let (_, _, events, _) =
            decode_chunk_bytes(&bytes, &self.header, &self.header_bytes, &self.key)?;
        Ok(events)
    }

    fn read_all(
        root: &Path,
        namespace: &str,
        key_provider: &dyn KeyProvider,
        quota_bytes: u64,
    ) -> Result<Vec<HistoryEvent>, HistoryError> {
        if directory_bytes(root) > quota_bytes {
            return Err(HistoryError::new(HistoryHealthCategory::QuotaReached));
        }
        let mut paths = history_chunk_paths(root)?;
        paths.sort();
        let mut events = Vec::new();
        for path in paths {
            let bytes = read_bounded(&path, quota_bytes)?;
            let (_, _, mut decoded, _) = decode_chunk(&bytes, namespace, key_provider)?;
            events.append(&mut decoded);
        }
        events.sort_by(|left, right| {
            left.data
                .sequence
                .cmp(&right.data.sequence)
                .then_with(|| left.source.cmp(&right.source))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut seen = std::collections::HashSet::new();
        events.retain(|event| seen.insert((event.source.clone(), event.id.clone())));
        if events.iter().any(|event| event.data.sequence == 0)
            || events
                .windows(2)
                .any(|pair| pair[0].data.sequence >= pair[1].data.sequence)
        {
            return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
        }
        Ok(events)
    }
}

fn decode_chunk(
    bytes: &[u8],
    namespace: &str,
    key_provider: &dyn KeyProvider,
) -> Result<(ChunkHeader, Vec<u8>, Vec<HistoryEvent>, u64), HistoryError> {
    let mut cursor = Cursor::new(bytes);
    let header: ChunkHeader = ciborium::de::from_reader(&mut cursor)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
    validate_header(&header)?;
    let header_end = cursor.position() as usize;
    let header_bytes = bytes[..header_end].to_vec();
    let key = key_provider.load(namespace, &header.3)?;
    let (_, _, events, counter) = decode_chunk_bytes(bytes, &header, &header_bytes, &key.bytes)?;
    Ok((header, header_bytes, events, counter))
}

fn decode_chunk_bytes(
    bytes: &[u8],
    header: &ChunkHeader,
    header_bytes: &[u8],
    key: &[u8],
) -> Result<(ChunkHeader, Vec<u8>, Vec<HistoryEvent>, u64), HistoryError> {
    let mut cursor = Cursor::new(bytes);
    let _: ChunkHeader = ciborium::de::from_reader(&mut cursor)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
    let chunk_key = derive_chunk_key(key, header)?;
    let cipher_key = Key::try_from(chunk_key.as_slice())
        .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyCorrupt))?;
    let cipher = ChaCha20Poly1305::new(&cipher_key);
    let mut events = Vec::new();
    let mut expected_counter = 0_u64;
    while (cursor.position() as usize) < bytes.len() {
        let value: coset::cbor::value::Value = ciborium::de::from_reader(&mut cursor)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        let coset::cbor::value::Value::Tag(tag, value) = value else {
            return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
        };
        if tag != <CoseEncrypt0 as TaggedCborSerializable>::TAG {
            return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
        }
        let encrypted = CoseEncrypt0::from_cbor_value(*value)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        validate_cose_headers(&encrypted)?;
        let nonce = encrypted.unprotected.iv.as_slice();
        if nonce.len() != 12 || nonce[..4] != *header.6.as_bytes() {
            return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
        }
        let counter = u64::from_be_bytes(
            nonce[4..]
                .try_into()
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?,
        );
        if counter != expected_counter {
            return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
        }
        expected_counter = expected_counter
            .checked_add(1)
            .ok_or_else(|| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        let plaintext = encrypted.decrypt_ciphertext(
            header_bytes,
            || HistoryError::new(HistoryHealthCategory::StorageCorrupt),
            |ciphertext, aad| {
                let nonce = Nonce::try_from(nonce)
                    .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
                cipher
                    .decrypt(
                        &nonce,
                        Payload {
                            msg: ciphertext,
                            aad,
                        },
                    )
                    .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))
            },
        )?;
        let event: HistoryEvent = serde_json::from_slice(&plaintext)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
        validate_event(&event)?;
        events.push(event);
    }
    Ok((
        header.clone(),
        header_bytes.to_vec(),
        events,
        expected_counter,
    ))
}

fn validate_cose_headers(encrypted: &CoseEncrypt0) -> Result<(), HistoryError> {
    let protected = &encrypted.protected.header;
    let unprotected = &encrypted.unprotected;
    let protected_exact = protected.alg
        == Some(coset::Algorithm::Assigned(
            iana::Algorithm::ChaCha20Poly1305,
        ))
        && protected.crit.is_empty()
        && protected.content_type.is_none()
        && protected.key_id.is_empty()
        && protected.iv.is_empty()
        && protected.partial_iv.is_empty()
        && protected.counter_signatures.is_empty()
        && protected.rest.is_empty();
    let unprotected_exact = unprotected.alg.is_none()
        && unprotected.crit.is_empty()
        && unprotected.content_type.is_none()
        && unprotected.key_id.is_empty()
        && unprotected.iv.len() == 12
        && unprotected.partial_iv.is_empty()
        && unprotected.counter_signatures.is_empty()
        && unprotected.rest.is_empty();
    if !protected_exact || !unprotected_exact {
        return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
    }
    Ok(())
}

fn validate_header(header: &ChunkHeader) -> Result<(), HistoryError> {
    if header.0 != HISTORY_PROFILE_VERSION
        || header.1 != iana::Algorithm::ChaCha20Poly1305 as i64
        || header.2 == 0
        || header.3.is_empty()
        || header.3.len() > 128
        || header.4.is_empty()
        || header.4.len() > 128
        || header.5.is_empty()
        || header.5.len() > 128
    {
        return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
    }
    Ok(())
}

fn derive_chunk_key(key: &[u8], header: &ChunkHeader) -> Result<Zeroizing<Vec<u8>>, HistoryError> {
    if key.len() != 32 {
        return Err(HistoryError::new(HistoryHealthCategory::KeyCorrupt));
    }
    let salt = decode_hex_128(&header.5)
        .ok_or_else(|| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), key);
    let mut info = Vec::with_capacity(CHUNK_KEY_INFO.len() + header.4.len() + 8);
    info.extend_from_slice(CHUNK_KEY_INFO);
    info.extend_from_slice(header.4.as_bytes());
    info.extend_from_slice(&header.2.to_be_bytes());
    let mut output = Zeroizing::new(vec![0_u8; 32]);
    hkdf.expand(&info, &mut output)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyCorrupt))?;
    Ok(output)
}

fn validate_event(event: &HistoryEvent) -> Result<(), HistoryError> {
    let allowed_type = matches!(
        event.event_type.as_str(),
        "cua-driver.history.control.v0"
            | "cua-driver.history.action_started.v0"
            | "cua-driver.history.action_completed.v0"
            | "cua-driver.history.session_started.v0"
            | "cua-driver.history.session_ended.v0"
            | "cua-driver.history.access.v0"
            | "cua-driver.history.health.v0"
    );
    if event.specversion != "1.0"
        || event.datacontenttype != "application/json"
        || event.dataschema != HISTORY_SCHEMA_URN
        || event
            .source
            .strip_prefix("urn:cua-driver:history:")
            .and_then(decode_hex_128)
            .is_none()
        || !allowed_type
        || decode_hex_128(&event.id).is_none()
        || event.subject.len() > 160
        || event.data.sequence == 0
        || !matches!(event.data.platform.as_str(), "macos" | "windows" | "linux")
        || event.data.process_model != "in_daemon"
        || event.data.caller_category != "cua_runtime"
        || event
            .data
            .session_id
            .as_deref()
            .is_some_and(|value| decode_hex_128(value).is_none())
        || event
            .data
            .action_id
            .as_deref()
            .is_some_and(|value| decode_hex_128(value).is_none())
        || event
            .data
            .capability
            .as_deref()
            .is_some_and(|value| !is_bounded_scalar(value, 128))
        || event.data.application.as_ref().is_some_and(|application| {
            application
                .bundle_id
                .as_deref()
                .is_some_and(|value| !is_bounded_scalar(value, 160))
                || application
                    .display_name
                    .as_deref()
                    .is_some_and(|value| !is_bounded_scalar(value, 120))
        })
        || !valid_payload_for_event(&event.event_type, &event.data.payload)
    {
        return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
    }
    let value = serde_json::to_value(event)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageCorrupt))?;
    reject_forbidden_fields(&value)?;
    Ok(())
}

fn valid_payload_for_event(event_type: &str, payload: &HistoryPayload) -> bool {
    match (event_type, payload) {
        ("cua-driver.history.control.v0", HistoryPayload::Control { .. })
        | ("cua-driver.history.action_started.v0", HistoryPayload::ActionStarted) => true,
        (
            "cua-driver.history.action_completed.v0",
            HistoryPayload::ActionCompleted {
                effect,
                route,
                delivery,
                evidence_kinds,
                escalation_kind,
                ..
            },
        ) => {
            matches!(
                effect.as_str(),
                "confirmed" | "partial" | "unverifiable" | "suspected_noop" | "refused" | "failed"
            ) && matches!(
                route.as_str(),
                "accessibility"
                    | "synthetic_events"
                    | "global_input"
                    | "system_api"
                    | "dom"
                    | "trusted_input"
                    | "unknown"
            ) && delivery.as_deref().is_none_or(|value| {
                matches!(
                    value,
                    "background" | "foreground" | "not_applicable" | "unknown"
                )
            }) && evidence_kinds.len() <= 16
                && evidence_kinds.iter().all(|value| {
                    matches!(
                        value.as_str(),
                        "accessibility_readback"
                            | "browser_readback"
                            | "value_readback"
                            | "window_change"
                    )
                })
                && escalation_kind.as_deref().is_none_or(|value| {
                    matches!(
                        value,
                        "activate_target"
                            | "retry_with_pixel_target"
                            | "retry_with_page_action"
                            | "refresh_page_state"
                            | "request_permission"
                            | "elevate_access"
                            | "expand_capture_scope"
                            | "prepare_session"
                            | "retry_with_foreground_delivery"
                    )
                })
        }
        ("cua-driver.history.session_started.v0", HistoryPayload::Session { phase }) => {
            phase == "started"
        }
        ("cua-driver.history.session_ended.v0", HistoryPayload::Session { phase }) => {
            phase == "ended"
        }
        ("cua-driver.history.access.v0", HistoryPayload::Access { operation, .. }) => {
            matches!(operation.as_str(), "agent_query" | "local_cli")
        }
        ("cua-driver.history.health.v0", HistoryPayload::Health { .. }) => true,
        _ => false,
    }
}

fn is_bounded_scalar(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains(['\n', '\r'])
}

fn reject_forbidden_fields(value: &Value) -> Result<(), HistoryError> {
    const FORBIDDEN: &[&str] = &[
        "args",
        "arguments",
        "result",
        "content",
        "text",
        "clipboard",
        "path",
        "url",
        "title",
        "screenshot",
        "image",
        "tree",
        "selector",
        "detail",
        "keystroke",
        "password",
        "credential",
        "token",
    ];
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_ascii_lowercase();
                if FORBIDDEN
                    .iter()
                    .any(|forbidden| key == *forbidden || key.ends_with(&format!("_{forbidden}")))
                {
                    return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
                }
                reject_forbidden_fields(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_forbidden_fields(value)?;
            }
        }
        Value::String(value) if value.len() > 256 => {
            return Err(HistoryError::new(HistoryHealthCategory::StorageCorrupt));
        }
        _ => {}
    }
    Ok(())
}

fn sanitize_application_identity(mut identity: ApplicationIdentity) -> Option<ApplicationIdentity> {
    identity.bundle_id = identity
        .bundle_id
        .and_then(|value| bounded_string(value, 160));
    identity.display_name = identity
        .display_name
        .and_then(|value| bounded_string(value, 120));
    (identity.bundle_id.is_some() || identity.display_name.is_some()).then_some(identity)
}

fn bounded_string(value: String, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= max && !value.contains(['\n', '\r']))
        .then(|| value.to_owned())
}

fn opaque_id(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex_128(&digest[..16])
}

fn hex_128(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex_128(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 || !value.is_ascii() {
        return None;
    }
    let mut decoded = [0_u8; 16];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(decoded)
}

fn effect_name(value: ActionEffect) -> &'static str {
    match value {
        ActionEffect::Confirmed => "confirmed",
        ActionEffect::Partial => "partial",
        ActionEffect::Unverifiable => "unverifiable",
        ActionEffect::SuspectedNoop => "suspected_noop",
        ActionEffect::Refused => "refused",
    }
}

fn route_name(value: ActionRoute) -> &'static str {
    match value {
        ActionRoute::Accessibility => "accessibility",
        ActionRoute::SyntheticEvents => "synthetic_events",
        ActionRoute::GlobalInput => "global_input",
        ActionRoute::SystemApi => "system_api",
        ActionRoute::Dom => "dom",
        ActionRoute::TrustedInput => "trusted_input",
    }
}

fn delivery_name(value: ActualDelivery) -> &'static str {
    match value {
        ActualDelivery::Background => "background",
        ActualDelivery::Foreground => "foreground",
        ActualDelivery::NotApplicable => "not_applicable",
        ActualDelivery::Unknown => "unknown",
    }
}

fn evidence_name(value: ProjectedEvidenceKind) -> &'static str {
    match value {
        ProjectedEvidenceKind::AccessibilityReadback => "accessibility_readback",
        ProjectedEvidenceKind::BrowserReadback => "browser_readback",
        ProjectedEvidenceKind::ValueReadback => "value_readback",
    }
}

fn escalation_name(value: EscalationKind) -> &'static str {
    match value {
        EscalationKind::ActivateTarget => "activate_target",
        EscalationKind::RetryWithPixelTarget => "retry_with_pixel_target",
        EscalationKind::RetryWithPageAction => "retry_with_page_action",
        EscalationKind::RefreshPageState => "refresh_page_state",
        EscalationKind::RequestPermission => "request_permission",
        EscalationKind::ElevateAccess => "elevate_access",
        EscalationKind::ExpandCaptureScope => "expand_capture_scope",
        EscalationKind::PrepareSession => "prepare_session",
        EscalationKind::RetryWithForegroundDelivery => "retry_with_foreground_delivery",
    }
}

fn chunks_dir(root: &Path) -> PathBuf {
    root.join("chunks")
}

fn state_path(root: &Path) -> PathBuf {
    root.join("state.json")
}

fn read_state(root: &Path) -> Option<PersistedState> {
    let bytes = fs::read(state_path(root)).ok()?;
    (bytes.len() <= 4096)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}

fn write_state(root: &Path, state: &PersistedState) -> Result<(), HistoryError> {
    prepare_history_root(root)?;
    let temporary = root.join("state.json.tmp");
    if temporary.exists() {
        fs::remove_file(&temporary)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    }
    let bytes = serde_json::to_vec(state)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    let mut file = secure_create_new_file(&temporary)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_data())
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    fs::rename(&temporary, state_path(root))
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

fn history_chunk_paths(root: &Path) -> Result<Vec<PathBuf>, HistoryError> {
    let directory = chunks_dir(root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(directory)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("cborseq")
            && entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn remove_history_files(root: &Path) -> Result<(), HistoryError> {
    for path in history_chunk_paths(root)? {
        fs::remove_file(path)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    }
    Ok(())
}

fn prune_expired_chunks(root: &Path, retention_days: u64) -> Result<(), HistoryError> {
    prune_expired_chunks_except(root, retention_days, None)
}

fn prune_expired_chunks_except(
    root: &Path,
    retention_days: u64,
    except: Option<&Path>,
) -> Result<(), HistoryError> {
    let retention = Duration::from_secs(retention_days.saturating_mul(24 * 60 * 60));
    let now = SystemTime::now();
    for path in history_chunk_paths(root)? {
        if except.is_some_and(|except| except == path) {
            continue;
        }
        let metadata = fs::metadata(&path)
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        let modified = metadata
            .modified()
            .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        let expired = now
            .duration_since(modified)
            .map(|age| age >= retention)
            .unwrap_or(false);
        if expired {
            fs::remove_file(path)
                .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
        }
    }
    Ok(())
}

fn read_snapshot_sender(
    sender: &SyncSender<WriterMessage>,
    retention_days: u64,
) -> Result<Vec<HistoryEvent>, HistoryError> {
    let (tx, rx) = mpsc::channel();
    sender
        .send(WriterMessage::ReadSnapshot {
            retention_days,
            response: tx,
        })
        .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| HistoryError::new(HistoryHealthCategory::WriterStopped))?
}

fn directory_bytes(root: &Path) -> u64 {
    history_chunk_paths(root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
        .sum()
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, HistoryError> {
    let metadata = fs::metadata(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    if metadata.len() > limit.max(1) {
        return Err(HistoryError::new(HistoryHealthCategory::QuotaReached));
    }
    let mut file = File::open(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    Ok(bytes)
}

#[cfg(unix)]
fn secure_create_dir(path: &Path) -> Result<(), HistoryError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(not(unix))]
fn secure_create_dir(path: &Path) -> Result<(), HistoryError> {
    fs::create_dir_all(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(unix)]
fn secure_create_new_file(path: &Path) -> Result<File, HistoryError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(not(unix))]
fn secure_create_new_file(path: &Path) -> Result<File, HistoryError> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(unix)]
fn secure_open_lock_file(path: &Path) -> Result<File, HistoryError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(not(unix))]
fn secure_open_lock_file(path: &Path) -> Result<File, HistoryError> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| HistoryError::new(HistoryHealthCategory::StorageUnavailable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct RecordingApplicationProvider {
        targets: Mutex<Vec<(i64, Option<u64>)>>,
    }

    impl ApplicationIdentityProvider for RecordingApplicationProvider {
        fn resolve(&self, pid: i64, window_id: Option<u64>) -> Option<ApplicationIdentity> {
            self.targets.lock().unwrap().push((pid, window_id));
            Some(ApplicationIdentity {
                bundle_id: Some("test.app".to_owned()),
                display_name: Some("Test App".to_owned()),
            })
        }
    }

    #[derive(Default)]
    struct MemoryKeyProvider {
        keys: Mutex<HashMap<String, Vec<u8>>>,
        load_threads: Mutex<Vec<Option<String>>>,
    }

    impl KeyProvider for MemoryKeyProvider {
        fn load_or_create(&self, namespace: &str) -> Result<HistoryKey, HistoryError> {
            let reference = format!("{namespace}.history.v1");
            let mut keys = self.keys.lock().unwrap();
            let bytes = keys.entry(reference.clone()).or_insert_with(|| {
                let mut bytes = vec![0_u8; 32];
                getrandom::fill(&mut bytes).expect("test random key");
                bytes
            });
            Ok(HistoryKey {
                reference,
                epoch: 1,
                bytes: Zeroizing::new(bytes.clone()),
            })
        }

        fn load(&self, _namespace: &str, reference: &str) -> Result<HistoryKey, HistoryError> {
            self.load_threads
                .lock()
                .unwrap()
                .push(thread::current().name().map(std::borrow::ToOwned::to_owned));
            let bytes = self
                .keys
                .lock()
                .unwrap()
                .get(reference)
                .cloned()
                .ok_or_else(|| HistoryError::new(HistoryHealthCategory::KeyUnavailable))?;
            Ok(HistoryKey {
                reference: reference.to_owned(),
                epoch: 1,
                bytes: Zeroizing::new(bytes),
            })
        }

        fn destroy(&self, _namespace: &str, reference: &str) -> Result<(), HistoryError> {
            self.keys.lock().unwrap().remove(reference);
            Ok(())
        }

        fn references(&self, namespace: &str) -> Result<Vec<String>, HistoryError> {
            let prefix = format!("{namespace}.history.");
            Ok(self
                .keys
                .lock()
                .unwrap()
                .keys()
                .filter(|reference| reference.starts_with(&prefix))
                .cloned()
                .collect())
        }
    }

    fn config(root: &Path) -> HistoryConfig {
        HistoryConfig {
            root: root.to_path_buf(),
            namespace: "test".to_owned(),
            admitted: true,
            platform: "macos".to_owned(),
            retention_days: DEFAULT_RETENTION_DAYS,
            quota_bytes: DEFAULT_QUOTA_BYTES,
        }
    }

    #[test]
    fn history_tool_descriptions_define_bounded_history_first_consultation() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        let status = HistoryStatusTool::new(Arc::clone(&manager));
        let query = HistoryQueryTool::new(manager);
        let status_description = &status.def().description;
        let query_description = &query.def().description;

        for trigger in ["continue", "recent work"] {
            assert!(status_description.contains(trigger));
            assert!(query_description.contains(trigger));
        }
        assert!(
            query_description.find("history_status")
                < query_description.find("one bounded initial query")
        );
        assert!(
            query_description.find("one bounded initial query")
                < query_description.find("broad desktop discovery")
        );
        for boundary in [
            "metadata-only",
            "screen and page content",
            "geometry",
            "inferred user intent",
            "model context",
        ] {
            assert!(query_description.contains(boundary), "missing {boundary}");
        }
        assert!(status_description.contains("never history entries"));
        assert!(status_description.contains("does not change history lifecycle state"));
        assert!(query_description.contains("does not enable, pause, resume, disable, or delete"));
        for fallback in ["absent", "access is denied", "normal discovery"] {
            assert!(status_description.contains(fallback));
            assert!(query_description.contains(fallback));
        }
    }

    #[test]
    fn history_consultation_keeps_closed_schemas_and_annotations() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        let status = HistoryStatusTool::new(Arc::clone(&manager));
        let query = HistoryQueryTool::new(manager);

        assert_eq!(
            status.def().input_schema,
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })
        );
        assert_eq!(
            query.def().input_schema,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_QUERY_LIMIT},
                    "session_id": {"type": "string", "minLength": 1, "maxLength": 128},
                    "since_sequence": {"type": "integer", "minimum": 1},
                    "until_sequence": {"type": "integer", "minimum": 1}
                },
                "additionalProperties": false
            })
        );
        assert_eq!(
            (
                status.def().read_only,
                status.def().destructive,
                status.def().idempotent,
                status.def().open_world,
            ),
            (true, false, true, false)
        );
        assert_eq!(
            (
                query.def().read_only,
                query.def().destructive,
                query.def().idempotent,
                query.def().open_world,
            ),
            (true, false, false, false)
        );
    }

    #[tokio::test]
    async fn history_query_ignores_trusted_mcp_session_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        let result = HistoryQueryTool::new(manager)
            .invoke(serde_json::json!({
                "limit": 1,
                "_session_id": "trusted-runtime-session",
                "_transport_session_id": "trusted-transport-session"
            }))
            .await;

        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("metadata_only")),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn encrypted_store_contains_no_event_plaintext_and_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys, None);
        manager.enable().unwrap();
        manager.pause().unwrap();
        manager.resume().unwrap();
        manager.flush().unwrap();
        let events = manager
            .query(
                HistoryQuery {
                    limit: Some(20),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.data.payload,
            HistoryPayload::Control {
                operation: HistoryControlOperation::Enable
            }
        )));
        let bytes = fs::read(history_chunk_paths(temp.path()).unwrap().pop().unwrap()).unwrap();
        let rendered = String::from_utf8_lossy(&bytes);
        assert!(!rendered.contains("action_started"));
        assert!(!rendered.contains("test_query"));
    }

    #[test]
    fn every_supported_desktop_platform_roundtrips_encrypted_events() {
        let temp = tempfile::tempdir().unwrap();
        for platform in ["macos", "windows", "linux"] {
            let root = temp.path().join(platform);
            let manager = HistoryManager::new(
                HistoryConfig {
                    root,
                    platform: platform.to_owned(),
                    ..config(temp.path())
                },
                Arc::new(MemoryKeyProvider::default()),
                None,
            );
            manager.enable().unwrap();
            manager.flush().unwrap();
            let events = manager
                .query(
                    HistoryQuery {
                        limit: Some(20),
                        ..Default::default()
                    },
                    HistoryAccessOperation::LocalCli,
                )
                .unwrap();
            assert!(!events.is_empty(), "{platform} emitted no encrypted events");
            assert!(events.iter().all(|event| event.data.platform == platform));
            manager.disable().unwrap();
            manager.delete_all().unwrap();
        }
    }

    #[test]
    fn implicit_sessions_use_keyed_ids_that_can_be_queried_as_opaque_ids() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        assert!(manager
            .begin_action(
                "click",
                &serde_json::json!({}),
                Some("private-runtime-session"),
            )
            .is_some());
        manager.flush().unwrap();
        let all = manager
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        let opaque = all
            .iter()
            .find_map(|event| event.data.session_id.clone())
            .expect("captured action has a session id");
        assert_ne!(opaque, "private-runtime-session");
        assert!(decode_hex_128(&opaque).is_some());
        let filtered = manager
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    session_id: Some(opaque.clone()),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(!filtered.is_empty());
        assert!(filtered
            .iter()
            .all(|event| event.data.session_id.as_deref() == Some(opaque.as_str())));
    }

    #[test]
    fn action_attribution_receives_pid_and_window_identity() {
        let temp = tempfile::tempdir().unwrap();
        let provider = Arc::new(RecordingApplicationProvider::default());
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            Some(provider.clone()),
        );
        manager.enable().unwrap();
        let pending = manager
            .begin_action(
                "click",
                &serde_json::json!({"pid": 42, "window_id": 9001}),
                None,
            )
            .unwrap();
        manager.finish_action(pending, None, false);
        manager.flush().unwrap();

        assert_eq!(
            provider.targets.lock().unwrap().as_slice(),
            &[(42, Some(9001)), (42, Some(9001))]
        );
        let events = manager
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter_map(|event| event.data.application.as_ref())
                .count(),
            2
        );
    }

    #[test]
    fn malformed_multibyte_hex_is_rejected_without_panicking() {
        let malformed = format!("€{}", "a".repeat(29));
        assert_eq!(malformed.len(), 32);
        assert_eq!(decode_hex_128(&malformed), None);
    }

    #[test]
    fn chunk_header_encodes_nonce_prefix_as_cbor_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        manager.disable().unwrap();
        let path = history_chunk_paths(temp.path()).unwrap().pop().unwrap();
        let value: coset::cbor::value::Value =
            ciborium::de::from_reader(File::open(path).unwrap()).unwrap();
        let coset::cbor::value::Value::Array(fields) = value else {
            panic!("history header must be a CBOR array")
        };
        assert!(matches!(
            fields.get(6),
            Some(coset::cbor::value::Value::Bytes(prefix)) if prefix.len() == 4
        ));
    }

    fn legacy_header_bytes(header: &ChunkHeader) -> Vec<u8> {
        use coset::cbor::value::{Integer, Value as CborValue};

        let legacy = CborValue::Array(vec![
            CborValue::Integer(Integer::from(header.0)),
            CborValue::Integer(Integer::from(header.1)),
            CborValue::Integer(Integer::from(header.2)),
            CborValue::Text(header.3.clone()),
            CborValue::Text(header.4.clone()),
            CborValue::Text(header.5.clone()),
            CborValue::Array(
                header
                    .6
                    .as_bytes()
                    .iter()
                    .copied()
                    .map(|byte| CborValue::Integer(Integer::from(byte)))
                    .collect(),
            ),
        ]);
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn chunk_header_reads_legacy_nonce_prefix_array_and_rewrites_canonical_bytes() {
        use coset::cbor::value::Value as CborValue;

        let header = ChunkHeader(
            HISTORY_PROFILE_VERSION,
            iana::Algorithm::ChaCha20Poly1305 as i64,
            1,
            "test.history.key".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
            "fedcba9876543210fedcba9876543210".to_owned(),
            NoncePrefix([0x12, 0x34, 0x56, 0x78]),
        );
        let legacy_bytes = legacy_header_bytes(&header);

        let decoded: ChunkHeader = ciborium::de::from_reader(legacy_bytes.as_slice()).unwrap();
        assert_eq!(decoded.6.as_bytes(), &[0x12, 0x34, 0x56, 0x78]);

        let mut canonical_bytes = Vec::new();
        ciborium::ser::into_writer(&decoded, &mut canonical_bytes).unwrap();
        let canonical: CborValue = ciborium::de::from_reader(canonical_bytes.as_slice()).unwrap();
        let CborValue::Array(fields) = canonical else {
            panic!("history header must remain a CBOR array")
        };
        assert!(matches!(
            fields.get(6),
            Some(CborValue::Bytes(prefix)) if prefix == &[0x12, 0x34, 0x56, 0x78]
        ));
    }

    #[test]
    fn legacy_nonce_prefix_array_remains_authenticated_as_original_aad() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        let key = keys.load_or_create("test").unwrap();
        let header = ChunkHeader(
            HISTORY_PROFILE_VERSION,
            iana::Algorithm::ChaCha20Poly1305 as i64,
            key.epoch,
            key.reference,
            "0123456789abcdef0123456789abcdef".to_owned(),
            "fedcba9876543210fedcba9876543210".to_owned(),
            NoncePrefix([0x12, 0x34, 0x56, 0x78]),
        );
        let header_bytes = legacy_header_bytes(&header);
        let mut event = manager.control_event(HistoryControlOperation::Enable);
        event.data.sequence = 1;
        let plaintext = serde_json::to_vec(&event).unwrap();
        let chunk_key = derive_chunk_key(&key.bytes, &header).unwrap();
        let cipher_key = Key::try_from(chunk_key.as_slice()).unwrap();
        let cipher = ChaCha20Poly1305::new(&cipher_key);
        let mut nonce = [0_u8; 12];
        nonce[..4].copy_from_slice(header.6.as_bytes());
        let protected = HeaderBuilder::new()
            .algorithm(iana::Algorithm::ChaCha20Poly1305)
            .build();
        let unprotected = HeaderBuilder::new().iv(nonce.to_vec()).build();
        let encrypted = CoseEncrypt0Builder::new()
            .protected(protected)
            .unprotected(unprotected)
            .try_create_ciphertext(&plaintext, &header_bytes, |plaintext, aad| {
                let nonce =
                    Nonce::try_from(nonce.as_slice()).map_err(|_| chacha20poly1305::Error)?;
                cipher.encrypt(
                    &nonce,
                    Payload {
                        msg: plaintext,
                        aad,
                    },
                )
            })
            .unwrap()
            .build();
        let mut chunk = header_bytes;
        chunk.extend(encrypted.to_tagged_vec().unwrap());
        secure_create_dir(&chunks_dir(temp.path())).unwrap();
        fs::write(
            chunks_dir(temp.path()).join(format!("{}.cborseq", header.5)),
            chunk,
        )
        .unwrap();

        let events =
            HistoryStore::read_all(temp.path(), "test", keys.as_ref(), DEFAULT_QUOTA_BYTES)
                .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, event.id);
        assert_eq!(events[0].data.sequence, 1);
    }

    #[test]
    fn privacy_validator_rejects_forbidden_field_names() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        let mut event = manager.control_event(HistoryControlOperation::Enable);
        let mut value = serde_json::to_value(&event).unwrap();
        value["data"]["clipboard"] = Value::String("secret".to_owned());
        assert!(serde_json::from_value::<HistoryEvent>(value).is_err());
        event.data.caller_category = "wrong".to_owned();
        assert!(validate_event(&event).is_err());
    }

    #[test]
    fn disabled_or_paused_capture_never_enqueues_actions() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        assert!(manager
            .begin_action("click", &serde_json::json!({"session":"secret/path"}), None,)
            .is_none());
        manager.enable().unwrap();
        manager.pause().unwrap();
        assert!(manager
            .begin_action("click", &serde_json::json!({}), None)
            .is_none());
    }

    #[test]
    fn the_next_accepted_action_reports_pending_drops() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        manager.record_drop();
        assert!(manager
            .begin_action("click", &serde_json::json!({}), None)
            .is_some());
        manager.flush().unwrap();
        let events = manager
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.data.payload,
            HistoryPayload::Health {
                category: HistoryHealthCategory::EventsDropped,
                count: 1
            }
        )));
        assert_eq!(manager.status().dropped_events, 1);
    }

    #[test]
    fn concurrent_capture_preserves_strict_stream_sequence_order() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        let mut threads = Vec::new();
        for _ in 0..8 {
            let manager = manager.clone();
            threads.push(thread::spawn(move || {
                for _ in 0..32 {
                    assert!(manager
                        .begin_action("click", &serde_json::json!({}), None)
                        .is_some());
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        manager.flush().unwrap();
        let events = manager
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(events
            .windows(2)
            .all(|pair| pair[0].data.sequence < pair[1].data.sequence));
    }

    #[test]
    fn disabled_capture_remains_queryable() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        manager.disable().unwrap();
        let events = manager
            .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.data.payload,
            HistoryPayload::Control {
                operation: HistoryControlOperation::Disable
            }
        )));
    }

    #[test]
    fn modified_ciphertext_and_wrong_key_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        manager.enable().unwrap();
        manager.disable().unwrap();

        let path = history_chunk_paths(temp.path()).unwrap().pop().unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0x01;
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            manager
                .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
                .unwrap_err()
                .category,
            HistoryHealthCategory::StorageCorrupt
        );

        let wrong_key_temp = tempfile::tempdir().unwrap();
        let original_keys = Arc::new(MemoryKeyProvider::default());
        let original_manager =
            HistoryManager::new(config(wrong_key_temp.path()), original_keys, None);
        original_manager.enable().unwrap();
        original_manager.disable().unwrap();
        drop(original_manager);
        let other_keys = Arc::new(MemoryKeyProvider::default());
        other_keys
            .load_or_create("test")
            .expect("create different key for same opaque reference");
        let other_manager = HistoryManager::new(config(wrong_key_temp.path()), other_keys, None);
        assert_eq!(
            other_manager
                .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
                .unwrap_err()
                .category,
            HistoryHealthCategory::StorageCorrupt
        );
    }

    fn mutate_first_cose(path: &Path, mutate: impl FnOnce(&mut CoseEncrypt0)) {
        let bytes = fs::read(path).unwrap();
        let mut cursor = Cursor::new(bytes.as_slice());
        let _: ChunkHeader = ciborium::de::from_reader(&mut cursor).unwrap();
        let header_end = cursor.position() as usize;
        let value: coset::cbor::value::Value = ciborium::de::from_reader(&mut cursor).unwrap();
        let first_end = cursor.position() as usize;
        let coset::cbor::value::Value::Tag(_, inner) = value else {
            panic!("expected tagged COSE item")
        };
        let mut encrypted = CoseEncrypt0::from_cbor_value(*inner).unwrap();
        mutate(&mut encrypted);
        let mut changed = bytes[..header_end].to_vec();
        changed.extend(encrypted.to_tagged_vec().unwrap());
        changed.extend_from_slice(&bytes[first_end..]);
        fs::write(path, changed).unwrap();
    }

    #[test]
    fn cose_profile_rejects_unknown_unprotected_and_mutated_protected_headers() {
        for mutation in 0..3 {
            let temp = tempfile::tempdir().unwrap();
            let keys = Arc::new(MemoryKeyProvider::default());
            let manager = HistoryManager::new(config(temp.path()), keys, None);
            manager.enable().unwrap();
            manager.disable().unwrap();
            let path = history_chunk_paths(temp.path()).unwrap().pop().unwrap();
            mutate_first_cose(&path, |encrypted| match mutation {
                0 => encrypted.unprotected.rest.push((
                    coset::Label::Text("unauthenticated-extension".to_owned()),
                    coset::cbor::value::Value::Bool(true),
                )),
                1 => {
                    encrypted.protected.header.alg =
                        Some(coset::Algorithm::Assigned(iana::Algorithm::A128GCM));
                    encrypted.protected.original_data = None;
                }
                _ => {
                    encrypted.protected.header.alg = None;
                    encrypted.protected.original_data = None;
                }
            });
            assert_eq!(
                manager
                    .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
                    .unwrap_err()
                    .category,
                HistoryHealthCategory::StorageCorrupt
            );
        }
    }

    #[test]
    fn quota_is_checked_before_a_chunk_header_reaches_disk() {
        let temp = tempfile::tempdir().unwrap();
        let mut limited = config(temp.path());
        limited.quota_bytes = 1;
        let manager = HistoryManager::new(limited, Arc::new(MemoryKeyProvider::default()), None);
        assert_eq!(
            manager.enable().unwrap_err().category,
            HistoryHealthCategory::QuotaReached
        );
        assert!(history_chunk_paths(temp.path()).unwrap().is_empty());
        assert!(!manager.status().enabled);
    }

    #[test]
    fn expired_chunks_are_pruned_before_a_new_writer_starts() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let mut immediate_retention = config(temp.path());
        immediate_retention.retention_days = 0;
        let first = HistoryManager::new(immediate_retention.clone(), keys.clone(), None);
        first.enable().unwrap();
        first.disable().unwrap();
        assert_eq!(history_chunk_paths(temp.path()).unwrap().len(), 1);
        drop(first);

        let second = HistoryManager::new(immediate_retention, keys, None);
        second.enable().unwrap();
        second.disable().unwrap();
        assert_eq!(history_chunk_paths(temp.path()).unwrap().len(), 1);
    }

    #[test]
    fn retention_is_enforced_for_disabled_and_long_lived_query_checkpoints() {
        let disabled_temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let mut immediate = config(disabled_temp.path());
        immediate.retention_days = 0;
        let disabled = HistoryManager::new(immediate, keys, None);
        disabled.enable().unwrap();
        disabled.disable().unwrap();
        assert!(disabled
            .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
            .unwrap()
            .is_empty());
        assert!(history_chunk_paths(disabled_temp.path())
            .unwrap()
            .is_empty());

        let active_temp = tempfile::tempdir().unwrap();
        let mut immediate = config(active_temp.path());
        immediate.retention_days = 0;
        let active = HistoryManager::new(immediate, Arc::new(MemoryKeyProvider::default()), None);
        active.enable().unwrap();
        assert!(active
            .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
            .unwrap()
            .is_empty());
        assert!(active.status().enabled);
        assert!(active
            .begin_action("click", &serde_json::json!({}), None)
            .is_some());
        active.flush().unwrap();
    }

    #[test]
    fn query_hides_old_events_even_when_the_chunk_mtime_is_recent() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        let lease = WriterLease::acquire(temp.path()).unwrap();
        let mut store = HistoryStore::create_with_lease(
            temp.path(),
            "test",
            "retention-test-stream",
            DEFAULT_QUOTA_BYTES,
            keys.as_ref(),
            lease,
        )
        .unwrap();
        let mut event = manager.control_event(HistoryControlOperation::Enable);
        event.data.sequence = 1;
        event.time = (OffsetDateTime::now_utc() - time::Duration::days(8))
            .format(&Rfc3339)
            .unwrap();
        store.append(&event).unwrap();
        store.flush().unwrap();
        drop(store);
        assert!(manager
            .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn forced_chunk_age_rotates_and_prunes_a_sealed_chunk_without_sleeping() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        let lease = WriterLease::acquire(temp.path()).unwrap();
        let mut store = HistoryStore::create_with_lease(
            temp.path(),
            "test",
            "rotation-test-stream",
            DEFAULT_QUOTA_BYTES,
            keys.as_ref(),
            lease,
        )
        .unwrap();
        let mut event = manager.control_event(HistoryControlOperation::Enable);
        event.data.sequence = 1;
        store.append(&event).unwrap();
        store.flush().unwrap();
        let sealed = store.path.clone();
        File::options()
            .write(true)
            .open(&sealed)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH)
                    .set_accessed(SystemTime::UNIX_EPOCH),
            )
            .unwrap();
        store.created_at = Instant::now() - CHUNK_ROTATION_INTERVAL;
        store.checkpoint_retention(DEFAULT_RETENTION_DAYS).unwrap();
        assert_ne!(store.path, sealed);
        assert!(!sealed.exists());
        assert!(store.path.exists());
    }

    #[test]
    fn each_writer_generation_creates_a_new_chunk_and_keeps_sequence_order() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let first = HistoryManager::new(config(temp.path()), keys.clone(), None);
        first.enable().unwrap();
        first.disable().unwrap();
        drop(first);

        let second = HistoryManager::new(config(temp.path()), keys, None);
        second.enable().unwrap();
        second.disable().unwrap();
        assert_eq!(history_chunk_paths(temp.path()).unwrap().len(), 2);
        let events = second
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(events
            .windows(2)
            .all(|pair| pair[0].data.sequence < pair[1].data.sequence));
    }

    #[test]
    fn a_second_manager_cannot_write_until_the_first_releases_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let first = HistoryManager::new(config(temp.path()), keys.clone(), None);
        first.enable().unwrap();

        let second = HistoryManager::new(config(temp.path()), keys, None);
        assert_eq!(second.status().health, HistoryHealthCategory::WriterStopped);
        assert_eq!(
            second.enable().unwrap_err().category,
            HistoryHealthCategory::WriterStopped
        );

        first.disable().unwrap();
        second.enable().unwrap();
        second.disable().unwrap();
        let events = second
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(events
            .windows(2)
            .all(|pair| pair[0].data.sequence < pair[1].data.sequence));
    }

    #[test]
    fn active_query_runs_on_writer_and_producer_hook_does_not_block() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        manager.enable().unwrap();

        keys.load_threads.lock().unwrap().clear();
        manager
            .query(HistoryQuery::default(), HistoryAccessOperation::LocalCli)
            .unwrap();
        assert!(keys
            .load_threads
            .lock()
            .unwrap()
            .iter()
            .any(|name| name.as_deref() == Some("cua-history-writer")));

        let writer_guard = manager.writer.lock().unwrap();
        let producer = manager.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            producer.session_event(Some("nonblocking-producer"), true);
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("producer hook blocked behind the writer handle lock");
        drop(writer_guard);
        join.join().unwrap();
        assert!(manager.status().dropped_events > 0);
    }

    #[test]
    fn delete_destroys_an_orphaned_namespace_key_without_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        keys.load_or_create("test").unwrap();
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        manager.delete_all().unwrap();
        assert!(keys.references("test").unwrap().is_empty());
    }

    #[test]
    fn delete_recovers_from_an_unreadable_chunk_header() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        manager.enable().unwrap();
        manager.disable().unwrap();
        let path = history_chunk_paths(temp.path()).unwrap().pop().unwrap();
        fs::write(path, [0xff]).unwrap();

        manager.delete_all().unwrap();

        assert!(keys.references("test").unwrap().is_empty());
        assert!(history_chunk_paths(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn terminal_writer_failure_cannot_block_disable_or_delete() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let mut limited = config(temp.path());
        limited.quota_bytes = 4096;
        let manager = HistoryManager::new(limited, keys.clone(), None);
        manager.enable().unwrap();
        let path = history_chunk_paths(temp.path()).unwrap().pop().unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(&vec![0_u8; 4096]).unwrap();
        file.flush().unwrap();
        manager.session_event(Some("quota-terminal"), true);
        assert_eq!(
            manager.flush().unwrap_err().category,
            HistoryHealthCategory::QuotaReached
        );

        assert!(!manager.disable().unwrap().enabled);
        manager.delete_all().unwrap();

        assert!(keys.references("test").unwrap().is_empty());
        assert!(history_chunk_paths(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn offline_purge_is_exclusive_and_removes_state_only_after_keys() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys.clone(), None);
        manager.enable().unwrap();
        assert_eq!(
            purge_offline(temp.path(), "test", keys.as_ref())
                .unwrap_err()
                .category,
            HistoryHealthCategory::WriterStopped
        );
        manager.disable().unwrap();
        drop(manager);
        fs::write(temp.path().join("admission.json"), b"{}").unwrap();
        let result = purge_offline(temp.path(), "test", keys.as_ref()).unwrap();
        assert_eq!(result.destroyed_keys, 1);
        assert!(keys.references("test").unwrap().is_empty());
        assert!(history_chunk_paths(temp.path()).unwrap().is_empty());
        assert!(!state_path(temp.path()).exists());
        assert!(!temp.path().join("admission.json").exists());
    }

    #[test]
    fn purge_refuses_a_nonempty_unmarked_override_before_touching_keys_or_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("managed-override");
        fs::create_dir(&root).unwrap();
        let state = root.join("state.json");
        fs::write(&state, br#"{"history_enabled":true}"#).unwrap();
        let keys = MemoryKeyProvider::default();
        keys.load_or_create("test").unwrap();

        assert_eq!(
            purge_offline(&root, "test", &keys).unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert!(state.exists());
        assert_eq!(keys.references("test").unwrap().len(), 1);
        assert!(!root.join(ROOT_MARKER_NAME).exists());
    }

    #[test]
    fn purge_refuses_relative_roots_without_creating_or_deleting_any_file() {
        let keys = MemoryKeyProvider::default();
        keys.load_or_create("test").unwrap();
        let root = Path::new("relative-computer-history");

        assert_eq!(
            purge_offline(root, "test", &keys).unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert_eq!(keys.references("test").unwrap().len(), 1);
        assert!(!root.exists());
    }

    #[test]
    fn nonempty_unmarked_computer_history_root_is_not_adopted_by_name() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("computer-history");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("state.json"), b"{}").unwrap();

        assert_eq!(
            prepare_history_root(&root).unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert!(!root.join(ROOT_MARKER_NAME).exists());
        assert!(root.join("state.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn purge_refuses_symlinked_roots_and_managed_entries_before_key_destruction() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let linked_root = temp.path().join("linked-root");
        symlink(&target, &linked_root).unwrap();
        let keys = MemoryKeyProvider::default();
        keys.load_or_create("test").unwrap();
        assert_eq!(
            purge_offline(&linked_root, "test", &keys)
                .unwrap_err()
                .category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert_eq!(keys.references("test").unwrap().len(), 1);

        let root = temp.path().join("safe-root");
        prepare_history_root(&root).unwrap();
        let outside = temp.path().join("outside-chunks");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("chunks")).unwrap();
        assert_eq!(
            purge_offline(&root, "test", &keys).unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert_eq!(keys.references("test").unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn state_writes_do_not_follow_a_managed_file_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("safe-root");
        prepare_history_root(&root).unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"preserve-me").unwrap();
        symlink(&sentinel, root.join("state.json.tmp")).unwrap();
        let state = PersistedState {
            enabled: true,
            paused: false,
        };

        assert_eq!(
            write_state(&root, &state).unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve-me");
    }

    #[test]
    fn delete_clears_the_cached_session_derivation_key_before_reenable() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let manager = HistoryManager::new(config(temp.path()), keys, None);
        manager.enable().unwrap();
        let before = manager.session_id("same-session").unwrap();
        manager.delete_all().unwrap();
        manager.enable().unwrap();
        let after = manager.session_id("same-session").unwrap();
        assert_ne!(before, after);
        manager.disable().unwrap();
    }

    #[test]
    fn admission_opt_in_restart_and_pause_completion_are_independent() {
        let temp = tempfile::tempdir().unwrap();
        let keys = Arc::new(MemoryKeyProvider::default());
        let denied = HistoryManager::new(
            HistoryConfig {
                admitted: false,
                ..config(temp.path())
            },
            keys.clone(),
            None,
        );
        assert_eq!(
            denied.enable().unwrap_err().category,
            HistoryHealthCategory::NotAdmitted
        );
        drop(denied);

        let admitted = HistoryManager::new(config(temp.path()), keys.clone(), None);
        assert!(!admitted.status().enabled);
        admitted.enable().unwrap();
        let pending = admitted
            .begin_action("click", &serde_json::json!({}), None)
            .unwrap();
        admitted.pause().unwrap();
        assert!(admitted
            .begin_action("click", &serde_json::json!({}), None)
            .is_none());
        admitted.finish_action(pending, None, false);
        admitted.flush_writer().unwrap();
        drop(admitted);

        let restarted = HistoryManager::new(config(temp.path()), keys, None);
        assert!(restarted.status().enabled);
        assert!(restarted.status().paused);
        let events = restarted
            .query(
                HistoryQuery {
                    limit: Some(MAX_QUERY_LIMIT),
                    ..Default::default()
                },
                HistoryAccessOperation::LocalCli,
            )
            .unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event.data.payload, HistoryPayload::ActionCompleted { .. })));
        restarted.disable().unwrap();
    }

    #[test]
    fn lifecycle_state_write_failures_do_not_create_split_brain_enablement() {
        let enable_temp = tempfile::tempdir().unwrap();
        fs::create_dir(enable_temp.path().join("state.json")).unwrap();
        let failed_enable = HistoryManager::new(
            config(enable_temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        assert_eq!(
            failed_enable.enable().unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert!(!failed_enable.status().enabled);

        let disable_temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::new(
            config(disable_temp.path()),
            Arc::new(MemoryKeyProvider::default()),
            None,
        );
        manager.enable().unwrap();
        fs::remove_file(disable_temp.path().join("state.json")).unwrap();
        fs::create_dir(disable_temp.path().join("state.json")).unwrap();
        assert_eq!(
            manager.disable().unwrap_err().category,
            HistoryHealthCategory::StorageUnavailable
        );
        assert!(manager.status().enabled);
        fs::remove_dir(disable_temp.path().join("state.json")).unwrap();
        manager.disable().unwrap();
    }
}
