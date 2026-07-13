//! Per-app approval gate for the Codex Computer Use compatibility surface.
//!
//! App resolution is allowed before this gate, but launch, accessibility
//! inspection, screenshots, and input are not. The daemon injects a private
//! broker credential only for MCP sessions with a live authenticated control
//! connection. A challenge is bound to that credential, expires quickly, and
//! is consumed by the first valid decision.

use serde::{Deserialize, Serialize};
use base64::Engine as _;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

const STORE_VERSION: u32 = 1;
const MAX_STORE_BYTES: u64 = 1024 * 1024;
const CHALLENGE_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppApprovalTarget {
    pub stable_id: String,
    pub app_identifier: String,
    pub display_name: String,
}

impl AppApprovalTarget {
    pub fn from_app(
        display_name: &str,
        bundle_id: Option<&str>,
        launch_path: Option<&str>,
    ) -> Result<Self, ApprovalError> {
        let path = launch_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApprovalError::InvalidIdentity(
                    "the app has no canonical launch path for approval".to_owned(),
                )
            })?;
        let code_requirement = crate::code_identity::designated_requirement(Path::new(path))
            .map_err(|error| {
                ApprovalError::InvalidIdentity(format!(
                    "the app signing identity could not be verified for approval: {error}"
                ))
            })?;
        Self::from_signed_app(display_name, bundle_id, Some(path), &code_requirement)
    }

    #[doc(hidden)]
    pub fn from_signed_app(
        display_name: &str,
        bundle_id: Option<&str>,
        launch_path: Option<&str>,
        code_requirement: &str,
    ) -> Result<Self, ApprovalError> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err(ApprovalError::InvalidIdentity(
                "the app has no display name".to_owned(),
            ));
        }
        let path = launch_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApprovalError::InvalidIdentity(
                    "the app has no canonical launch path for approval".to_owned(),
                )
            })?;
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            ApprovalError::InvalidIdentity(format!(
                "the app launch path could not be canonicalized for approval: {error}"
            ))
        })?;
        let canonical_path = canonical.to_string_lossy().into_owned();
        let code_requirement = code_requirement.trim();
        if code_requirement.is_empty() {
            return Err(ApprovalError::InvalidIdentity(
                "the app has no verified signing identity for approval".to_owned(),
            ));
        }
        let encoded_requirement = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(code_requirement.as_bytes());
        let (stable_id, app_identifier) =
            if let Some(bundle_id) = bundle_id.map(str::trim).filter(|value| !value.is_empty()) {
                (
                    format!(
                        "bundle:{}|path:{canonical_path}|requirement:{encoded_requirement}",
                        bundle_id.to_lowercase()
                    ),
                    bundle_id.to_owned(),
                )
            } else {
                (
                    format!("path:{canonical_path}|requirement:{encoded_requirement}"),
                    canonical_path,
                )
            };
        Ok(Self {
            stable_id,
            app_identifier,
            display_name: display_name.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalChallenge {
    pub id: String,
    pub app_identifier: String,
    pub display_name: String,
    pub allow_persistent_approval: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalCheck {
    Approved,
    Required(ApprovalChallenge),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalAction {
    Accept,
    Decline,
    Cancel,
}

impl ApprovalAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "accept" => Some(Self::Accept),
            "decline" => Some(Self::Decline),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalPersistence {
    Session,
    Always,
}

impl ApprovalPersistence {
    pub fn parse(value: Option<&str>) -> Result<Self, ApprovalError> {
        match value {
            None | Some("session") => Ok(Self::Session),
            Some("always") => Ok(Self::Always),
            Some(other) => Err(ApprovalError::InvalidDecision(format!(
                "unsupported approval persistence '{other}'"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalResolution {
    Approved { persistent: bool },
    Declined { message: String },
    Canceled { message: String },
}

#[derive(Debug, Eq, PartialEq)]
pub enum ApprovalError {
    Unauthenticated(String),
    InvalidIdentity(String),
    InvalidChallenge(String),
    InvalidDecision(String),
    PersistenceUnavailable(String),
    Store(String),
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthenticated(message)
            | Self::InvalidIdentity(message)
            | Self::InvalidChallenge(message)
            | Self::InvalidDecision(message)
            | Self::PersistenceUnavailable(message)
            | Self::Store(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ApprovalError {}

#[derive(Debug)]
struct ChallengeRecord {
    session_id: String,
    broker_token: String,
    stable_id: String,
    display_name: String,
    expires_at: Instant,
}

#[derive(Default)]
struct ApprovalState {
    brokers: HashMap<String, String>,
    session_approvals: HashSet<(String, String)>,
    challenges: HashMap<String, ChallengeRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistentApprovalFile {
    version: u32,
    approvals: BTreeSet<String>,
}

pub struct ApprovalGate {
    state: Mutex<ApprovalState>,
    persistent_store_lock: Mutex<()>,
    store_path: PathBuf,
    allow_persistent_approval: bool,
}

impl ApprovalGate {
    pub fn new(store_path: PathBuf, allow_persistent_approval: bool) -> Self {
        Self {
            state: Mutex::new(ApprovalState::default()),
            persistent_store_lock: Mutex::new(()),
            store_path,
            allow_persistent_approval,
        }
    }

    pub fn allow_persistent_approval(&self) -> bool {
        self.allow_persistent_approval
    }

    pub fn register_broker(&self, session_id: &str, broker_token: &str) {
        if session_id.is_empty() || broker_token.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state
            .session_approvals
            .retain(|(owner, _)| owner != session_id);
        state
            .brokers
            .insert(session_id.to_owned(), broker_token.to_owned());
        state
            .challenges
            .retain(|_, challenge| challenge.session_id != session_id);
    }

    pub fn unregister_broker(&self, session_id: &str, broker_token: &str) {
        let mut state = self.state.lock().unwrap();
        if state.brokers.get(session_id).map(String::as_str) == Some(broker_token) {
            state.brokers.remove(session_id);
            state
                .session_approvals
                .retain(|(owner, _)| owner != session_id);
            state
                .challenges
                .retain(|_, challenge| challenge.session_id != session_id);
        }
    }

    pub fn check(
        &self,
        session_id: &str,
        target: &AppApprovalTarget,
    ) -> Result<ApprovalCheck, ApprovalError> {
        // Broker ownership is the outer transaction lock for approval checks
        // and decisions. Whenever both locks are needed, the persistent store
        // nests inside this lock, so register/unregister cannot replace the
        // broker while a durable approval decision is being committed.
        let mut state = self.state.lock().unwrap();
        let broker_token = state.brokers.get(session_id).cloned().ok_or_else(|| {
            ApprovalError::Unauthenticated(format!(
                "Computer Use requires an authenticated MCP elicitation session before using app '{}'.",
                target.display_name
            ))
        })?;
        if state
            .session_approvals
            .contains(&(session_id.to_owned(), target.stable_id.clone()))
        {
            return Ok(ApprovalCheck::Approved);
        }

        let _store = self.persistent_store_lock.lock().unwrap();
        let _file_store = StoreFileLock::acquire(&self.store_path)?;
        let persistent = self.load_persistent_approvals_unlocked()?;
        prune_expired_challenges(&mut state);
        if persistent.contains(&target.stable_id) {
            return Ok(ApprovalCheck::Approved);
        }

        // Keep at most one outstanding decision per session and app. A client
        // that retries before answering receives a fresh challenge and cannot
        // grow daemon memory with duplicate prompts.
        state.challenges.retain(|_, challenge| {
            challenge.session_id != session_id || challenge.stable_id != target.stable_id
        });
        let id = Uuid::new_v4().to_string();
        state.challenges.insert(
            id.clone(),
            ChallengeRecord {
                session_id: session_id.to_owned(),
                broker_token,
                stable_id: target.stable_id.clone(),
                display_name: target.display_name.clone(),
                expires_at: Instant::now() + CHALLENGE_TTL,
            },
        );
        Ok(ApprovalCheck::Required(ApprovalChallenge {
            id,
            app_identifier: target.app_identifier.clone(),
            display_name: target.display_name.clone(),
            allow_persistent_approval: self.allow_persistent_approval,
        }))
    }

    pub fn resolve(
        &self,
        session_id: &str,
        broker_token: &str,
        challenge_id: &str,
        action: ApprovalAction,
        persistence: ApprovalPersistence,
    ) -> Result<ApprovalResolution, ApprovalError> {
        let mut state = self.state.lock().unwrap();
        prune_expired_challenges(&mut state);
        if state.brokers.get(session_id).map(String::as_str) != Some(broker_token) {
            return Err(ApprovalError::Unauthenticated(
                "Computer Use approval resolution requires the authenticated MCP session."
                    .to_owned(),
            ));
        }
        let challenge = state.challenges.get(challenge_id).ok_or_else(|| {
            ApprovalError::InvalidChallenge(
                "Computer Use approval challenge is invalid, expired, or already used.".to_owned(),
            )
        })?;
        if challenge.session_id != session_id || challenge.broker_token != broker_token {
            return Err(ApprovalError::Unauthenticated(
                "Computer Use approval challenge does not belong to this authenticated MCP session."
                    .to_owned(),
            ));
        }
        let challenge = state.challenges.remove(challenge_id).unwrap();

        match action {
            ApprovalAction::Decline => Ok(ApprovalResolution::Declined {
                message: format!(
                    "Computer Use approval denied via MCP elicitation for app '{}'.",
                    challenge.display_name
                ),
            }),
            ApprovalAction::Cancel => Ok(ApprovalResolution::Canceled {
                message: format!(
                    "Computer Use permission request canceled for app '{}'.",
                    challenge.display_name
                ),
            }),
            ApprovalAction::Accept => {
                if persistence == ApprovalPersistence::Always {
                    if !self.allow_persistent_approval {
                        return Err(ApprovalError::PersistenceUnavailable(format!(
                            "Computer Use could not persist the approval permanently for app '{}'.",
                            challenge.display_name
                        )));
                    }
                    let _store = self.persistent_store_lock.lock().unwrap();
                    let _file_store = StoreFileLock::acquire(&self.store_path)?;
                    let mut persistent = self.load_persistent_approvals_unlocked()?;
                    persistent.insert(challenge.stable_id.clone());
                    self.write_persistent_approvals_unlocked(&persistent)?;
                }
                // `state` has remained locked since token/challenge validation,
                // including across the durable write above. Broker replacement
                // therefore linearizes either wholly before this decision (and
                // fails validation) or wholly after the commit.
                state
                    .session_approvals
                    .insert((session_id.to_owned(), challenge.stable_id));
                Ok(ApprovalResolution::Approved {
                    persistent: persistence == ApprovalPersistence::Always,
                })
            }
        }
    }

    pub fn clear_session(&self, session_id: &str) {
        let mut state = self.state.lock().unwrap();
        state.brokers.remove(session_id);
        state
            .session_approvals
            .retain(|(owner, _)| owner != session_id);
        state
            .challenges
            .retain(|_, challenge| challenge.session_id != session_id);
    }

    /// Return path-bound stable app identities approved permanently. The CLI
    /// uses this narrow seam without expanding the public MCP tool roster.
    pub fn list_persistent(&self) -> Result<Vec<String>, ApprovalError> {
        let _store = self.persistent_store_lock.lock().unwrap();
        let _file_store = StoreFileLock::acquire(&self.store_path)?;
        Ok(self
            .load_persistent_approvals_unlocked()?
            .into_iter()
            .collect())
    }

    /// Revoke one permanent approval. A bare bundle identifier is normalized
    /// to the same stable key used by app resolution.
    pub fn revoke_persistent(&self, app_identity: &str) -> Result<bool, ApprovalError> {
        let key = normalize_management_identity(app_identity)?;
        let _store = self.persistent_store_lock.lock().unwrap();
        let _file_store = StoreFileLock::acquire(&self.store_path)?;
        let mut approvals = self.load_persistent_approvals_unlocked()?;
        let original_count = approvals.len();
        if key.starts_with("bundle:") && !key.contains("|path:") {
            let path_bound_prefix = format!("{key}|path:");
            approvals
                .retain(|approval| approval != &key && !approval.starts_with(&path_bound_prefix));
        } else {
            approvals.remove(&key);
        }
        let removed = approvals.len() != original_count;
        if removed {
            self.write_persistent_approvals_unlocked(&approvals)?;
        }
        Ok(removed)
    }

    pub fn clear_persistent(&self) -> Result<usize, ApprovalError> {
        let _store = self.persistent_store_lock.lock().unwrap();
        let _file_store = StoreFileLock::acquire(&self.store_path)?;
        let approvals = self.load_persistent_approvals_unlocked()?;
        let count = approvals.len();
        if count > 0 {
            self.write_persistent_approvals_unlocked(&BTreeSet::new())?;
        }
        Ok(count)
    }

    fn load_persistent_approvals_unlocked(&self) -> Result<BTreeSet<String>, ApprovalError> {
        let mut file = match open_store_for_read(&self.store_path)? {
            Some(file) => file,
            None => return Ok(BTreeSet::new()),
        };
        let metadata = file.metadata().map_err(store_error)?;
        if metadata.len() > MAX_STORE_BYTES {
            return Err(ApprovalError::Store(format!(
                "Computer Use approval store is larger than {MAX_STORE_BYTES} bytes."
            )));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes).map_err(store_error)?;
        let decoded: PersistentApprovalFile = serde_json::from_slice(&bytes).map_err(|error| {
            ApprovalError::Store(format!("Computer Use approval store is invalid: {error}"))
        })?;
        if decoded.version != STORE_VERSION {
            return Err(ApprovalError::Store(format!(
                "Computer Use approval store version {} is unsupported.",
                decoded.version
            )));
        }
        Ok(decoded.approvals)
    }

    fn write_persistent_approvals_unlocked(
        &self,
        approvals: &BTreeSet<String>,
    ) -> Result<(), ApprovalError> {
        let parent = self.store_path.parent().ok_or_else(|| {
            ApprovalError::Store("Computer Use approval store has no parent directory.".to_owned())
        })?;
        ensure_private_directory(parent)?;
        let payload = serde_json::to_vec_pretty(&PersistentApprovalFile {
            version: STORE_VERSION,
            approvals: approvals.clone(),
        })
        .map_err(store_error)?;
        let temporary = parent.join(format!(".app-approvals-{}.tmp", Uuid::new_v4()));
        let result = (|| {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)
                .map_err(store_error)?;
            file.write_all(&payload).map_err(store_error)?;
            file.sync_all().map_err(store_error)?;
            std::fs::rename(&temporary, &self.store_path).map_err(store_error)?;
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(store_error)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

#[derive(Debug)]
struct StoreFileLock {
    file: std::fs::File,
}

impl StoreFileLock {
    fn acquire(store_path: &Path) -> Result<Self, ApprovalError> {
        Self::acquire_with_flags(store_path, libc::LOCK_EX)
    }

    #[cfg(test)]
    fn try_acquire(store_path: &Path) -> Result<Self, ApprovalError> {
        Self::acquire_with_flags(store_path, libc::LOCK_EX | libc::LOCK_NB)
    }

    fn acquire_with_flags(store_path: &Path, flags: libc::c_int) -> Result<Self, ApprovalError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let parent = store_path.parent().ok_or_else(|| {
            ApprovalError::Store("Computer Use approval store has no parent directory.".to_owned())
        })?;
        ensure_private_directory(parent)?;
        let lock_path = store_path.with_extension("lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .map_err(store_error)?;
        let metadata = file.metadata().map_err(store_error)?;
        let expected_uid = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.uid() != expected_uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(ApprovalError::Store(format!(
                "Computer Use approval lock has unsafe ownership or permissions: {}",
                lock_path.display()
            )));
        }

        loop {
            if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
                return Ok(Self { file });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(store_error(error));
            }
        }
    }
}

impl Drop for StoreFileLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn prune_expired_challenges(state: &mut ApprovalState) {
    let now = Instant::now();
    state
        .challenges
        .retain(|_, challenge| challenge.expires_at > now);
}

fn normalize_management_identity(value: &str) -> Result<String, ApprovalError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApprovalError::InvalidIdentity(
            "app approval identity cannot be empty".to_owned(),
        ));
    }
    if let Some(bundle_identity) = value.strip_prefix("bundle:") {
        if let Some((bundle_id, path)) = bundle_identity.split_once("|path:") {
            Ok(format!("bundle:{}|path:{path}", bundle_id.to_lowercase()))
        } else {
            Ok(format!("bundle:{}", bundle_identity.to_lowercase()))
        }
    } else if value.starts_with("path:") {
        Ok(value.to_owned())
    } else {
        Ok(format!("bundle:{}", value.to_lowercase()))
    }
}

fn default_store_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Application Support/CuaDriver/app-approvals.json")
}

fn persistent_approval_allowed_by_policy() -> bool {
    !matches!(
        std::env::var("CUA_DRIVER_CODEX_ALLOW_PERSISTENT_APPROVAL")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("0" | "false" | "no" | "off")
    )
}

pub fn global() -> &'static ApprovalGate {
    static GLOBAL: OnceLock<ApprovalGate> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        ApprovalGate::new(
            default_store_path(),
            persistent_approval_allowed_by_policy(),
        )
    })
}

pub fn register_session_cleanup() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        cua_driver_core::session::register_session_end_hook(|session_id| {
            global().clear_session(session_id);
        });
    });
}

fn ensure_private_directory(path: &Path) -> Result<(), ApprovalError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(store_error)?;
    }
    let metadata = std::fs::symlink_metadata(path).map_err(store_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ApprovalError::Store(format!(
            "Computer Use approval store directory is not a real directory: {}",
            path.display()
        )));
    }
    let expected_uid = unsafe { libc::geteuid() };
    if metadata.uid() != expected_uid {
        return Err(ApprovalError::Store(format!(
            "Computer Use approval store directory is not owned by the current user: {}",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(store_error)?;
    }
    Ok(())
}

fn open_store_for_read(path: &Path) -> Result<Option<std::fs::File>, ApprovalError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(store_error(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ApprovalError::Store(format!(
            "Computer Use approval store is not a regular file: {}",
            path.display()
        )));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(store_error)?;
    let opened_metadata = file.metadata().map_err(store_error)?;
    let expected_uid = unsafe { libc::geteuid() };
    if !opened_metadata.is_file()
        || opened_metadata.uid() != expected_uid
        || opened_metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ApprovalError::Store(format!(
            "Computer Use approval store has unsafe ownership or permissions: {}",
            path.display()
        )));
    }
    Ok(Some(file))
}

fn store_error(error: impl std::fmt::Display) -> ApprovalError {
    ApprovalError::Store(format!("Computer Use approval store error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn target() -> AppApprovalTarget {
        AppApprovalTarget::from_app(
            "Calculator",
            Some("com.apple.Calculator"),
            Some("/System/Applications/Calculator.app"),
        )
        .unwrap()
    }

    fn gate(allow_persistent: bool) -> (tempfile::TempDir, ApprovalGate) {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("private/app-approvals.json");
        (directory, ApprovalGate::new(store, allow_persistent))
    }

    #[test]
    fn approval_identity_binds_bundle_id_to_canonical_app_path() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("First.app");
        let second = directory.path().join("Second.app");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();

        let first = AppApprovalTarget::from_signed_app(
            "Editor",
            Some("com.example.Editor"),
            first.to_str(),
            "identifier com.example.Editor and certificate leaf = H\"01\"",
        )
        .unwrap();
        let second = AppApprovalTarget::from_signed_app(
            "Editor",
            Some("com.example.Editor"),
            second.to_str(),
            "identifier com.example.Editor and certificate leaf = H\"01\"",
        )
        .unwrap();
        assert_ne!(first.stable_id, second.stable_id);
        assert!(first
            .stable_id
            .starts_with("bundle:com.example.editor|path:"));
        assert!(matches!(
            AppApprovalTarget::from_app("Editor", Some("com.example.Editor"), None),
            Err(ApprovalError::InvalidIdentity(_))
        ));
    }

    #[test]
    fn approval_identity_changes_when_same_path_is_resigned() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Editor.app");
        std::fs::create_dir_all(&path).unwrap();
        let original = AppApprovalTarget::from_signed_app(
            "Editor",
            Some("com.example.Editor"),
            path.to_str(),
            "identifier com.example.Editor and certificate leaf = H\"01\"",
        )
        .unwrap();
        let replacement = AppApprovalTarget::from_signed_app(
            "Editor",
            Some("com.example.Editor"),
            path.to_str(),
            "identifier com.example.Editor and certificate leaf = H\"02\"",
        )
        .unwrap();
        assert_ne!(original.stable_id, replacement.stable_id);
    }

    #[test]
    fn no_broker_credential_fails_closed_without_a_challenge() {
        let (_directory, gate) = gate(true);
        let error = gate.check("raw-session", &target()).unwrap_err();
        assert!(matches!(error, ApprovalError::Unauthenticated(_)));
        assert!(gate.state.lock().unwrap().challenges.is_empty());
    }

    #[test]
    fn plain_accept_is_session_scoped_and_challenge_is_one_time() {
        let (_directory, gate) = gate(true);
        let target = target();
        gate.register_broker("session-a", "broker-a");
        let challenge = match gate.check("session-a", &target).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };
        assert_eq!(challenge.display_name, "Calculator");
        assert!(challenge.allow_persistent_approval);
        assert_eq!(
            gate.resolve(
                "session-a",
                "broker-a",
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Session,
            )
            .unwrap(),
            ApprovalResolution::Approved { persistent: false }
        );
        assert_eq!(
            gate.check("session-a", &target).unwrap(),
            ApprovalCheck::Approved
        );
        gate.register_broker("session-b", "broker-b");
        assert!(matches!(
            gate.check("session-b", &target).unwrap(),
            ApprovalCheck::Required(_)
        ));
        assert!(matches!(
            gate.resolve(
                "session-a",
                "broker-a",
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Session,
            ),
            Err(ApprovalError::InvalidChallenge(_))
        ));

        gate.clear_session("session-a");
        gate.register_broker("session-a", "broker-a");
        assert!(matches!(
            gate.check("session-a", &target).unwrap(),
            ApprovalCheck::Required(_)
        ));
    }

    #[test]
    fn replacing_or_removing_a_broker_cannot_inherit_session_approval() {
        let (_directory, gate) = gate(true);
        let target = target();
        gate.register_broker("session", "broker-a");
        let challenge = match gate.check("session", &target).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };
        gate.resolve(
            "session",
            "broker-a",
            &challenge.id,
            ApprovalAction::Accept,
            ApprovalPersistence::Session,
        )
        .unwrap();

        gate.register_broker("session", "broker-b");
        assert!(matches!(
            gate.check("session", &target).unwrap(),
            ApprovalCheck::Required(_)
        ));
        gate.unregister_broker("session", "broker-b");
        let state = gate.state.lock().unwrap();
        assert!(!state.brokers.contains_key("session"));
        assert!(!state
            .session_approvals
            .contains(&("session".to_owned(), target.stable_id)));
    }

    #[test]
    fn persistent_accept_survives_gate_reopen_and_uses_private_storage() {
        let (directory, gate) = gate(true);
        let store = gate.store_path.clone();
        let target = target();
        gate.register_broker("session-a", "broker-a");
        let challenge = match gate.check("session-a", &target).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };
        assert_eq!(
            gate.resolve(
                "session-a",
                "broker-a",
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Always,
            )
            .unwrap(),
            ApprovalResolution::Approved { persistent: true }
        );

        let reopened = ApprovalGate::new(store.clone(), true);
        reopened.register_broker("session-b", "broker-b");
        assert_eq!(
            reopened.check("session-b", &target).unwrap(),
            ApprovalCheck::Approved
        );
        assert_eq!(
            std::fs::metadata(store.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let metadata = std::fs::metadata(&store).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        drop(directory);
    }

    #[test]
    fn persistent_commit_holds_broker_ownership_until_the_write_finishes() {
        use std::sync::{Arc, TryLockError};

        let (_directory, gate) = gate(true);
        let gate = Arc::new(gate);
        let target = target();
        gate.register_broker("session", "broker");
        let challenge = match gate.check("session", &target).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };

        // Stop the resolver at the interprocess store lock. A race-safe
        // resolver must already hold ApprovalState here, making broker removal
        // wait until the durable decision has committed.
        let file_lock = StoreFileLock::acquire(&gate.store_path).unwrap();
        let resolving_gate = gate.clone();
        let resolving = std::thread::spawn(move || {
            resolving_gate.resolve(
                "session",
                "broker",
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Always,
            )
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match gate.state.try_lock() {
                Err(TryLockError::WouldBlock) => break,
                Err(TryLockError::Poisoned(error)) => panic!("approval state poisoned: {error}"),
                Ok(state) => drop(state),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "resolver never held broker ownership while waiting for the store lock"
            );
            std::thread::yield_now();
        }

        let removing_gate = gate.clone();
        let removing = std::thread::spawn(move || {
            removing_gate.unregister_broker("session", "broker");
        });
        drop(file_lock);

        assert_eq!(
            resolving.join().unwrap().unwrap(),
            ApprovalResolution::Approved { persistent: true }
        );
        removing.join().unwrap();
        assert_eq!(gate.list_persistent().unwrap(), vec![target.stable_id]);
    }

    #[test]
    fn broker_removal_before_resolution_cannot_leave_a_persistent_grant() {
        let (_directory, gate) = gate(true);
        let target = target();
        gate.register_broker("session", "broker");
        let challenge = match gate.check("session", &target).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };

        gate.unregister_broker("session", "broker");
        assert!(matches!(
            gate.resolve(
                "session",
                "broker",
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Always,
            ),
            Err(ApprovalError::Unauthenticated(_)) | Err(ApprovalError::InvalidChallenge(_))
        ));
        assert!(gate.list_persistent().unwrap().is_empty());
    }

    #[test]
    fn decline_and_cancel_consume_challenges_without_approving() {
        for (action, expected) in [
            (ApprovalAction::Decline, "approval denied"),
            (ApprovalAction::Cancel, "request canceled"),
        ] {
            let (_directory, gate) = gate(true);
            let target = target();
            gate.register_broker("session", "broker");
            let challenge = match gate.check("session", &target).unwrap() {
                ApprovalCheck::Required(challenge) => challenge,
                ApprovalCheck::Approved => panic!("unexpected approval"),
            };
            let resolution = gate
                .resolve(
                    "session",
                    "broker",
                    &challenge.id,
                    action,
                    ApprovalPersistence::Session,
                )
                .unwrap();
            let message = match resolution {
                ApprovalResolution::Declined { message }
                | ApprovalResolution::Canceled { message } => message,
                ApprovalResolution::Approved { .. } => panic!("unexpected approval"),
            };
            assert!(message.contains(expected));
            assert!(matches!(
                gate.resolve(
                    "session",
                    "broker",
                    &challenge.id,
                    action,
                    ApprovalPersistence::Session,
                ),
                Err(ApprovalError::InvalidChallenge(_))
            ));
            assert!(matches!(
                gate.check("session", &target).unwrap(),
                ApprovalCheck::Required(_)
            ));
        }
    }

    #[test]
    fn policy_can_forbid_permanent_approval() {
        let (_directory, gate) = gate(false);
        gate.register_broker("session", "broker");
        let challenge = match gate.check("session", &target()).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };
        assert!(!challenge.allow_persistent_approval);
        assert!(matches!(
            gate.resolve(
                "session",
                "broker",
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Always,
            ),
            Err(ApprovalError::PersistenceUnavailable(_))
        ));
    }

    #[test]
    fn persistent_management_can_list_revoke_and_clear() {
        let (_directory, gate) = gate(true);
        let target = target();
        gate.register_broker("session", "broker");
        let challenge = match gate.check("session", &target).unwrap() {
            ApprovalCheck::Required(challenge) => challenge,
            ApprovalCheck::Approved => panic!("unexpected approval"),
        };
        gate.resolve(
            "session",
            "broker",
            &challenge.id,
            ApprovalAction::Accept,
            ApprovalPersistence::Always,
        )
        .unwrap();
        assert_eq!(
            gate.list_persistent().unwrap(),
            vec![target.stable_id.clone()]
        );
        assert!(gate
            .revoke_persistent("bundle:com.apple.Calculator")
            .unwrap());
        assert!(gate.list_persistent().unwrap().is_empty());
        assert_eq!(gate.clear_persistent().unwrap(), 0);
    }

    #[test]
    fn bundle_revoke_removes_every_path_bound_copy() {
        let (directory, gate) = gate(true);
        let first_path = directory.path().join("First.app");
        let second_path = directory.path().join("Second.app");
        std::fs::create_dir_all(&first_path).unwrap();
        std::fs::create_dir_all(&second_path).unwrap();
        let first = AppApprovalTarget::from_signed_app(
            "Editor",
            Some("com.example.Editor"),
            first_path.to_str(),
            "identifier com.example.Editor and certificate leaf = H\"01\"",
        )
        .unwrap();
        let second = AppApprovalTarget::from_signed_app(
            "Editor",
            Some("com.example.Editor"),
            second_path.to_str(),
            "identifier com.example.Editor and certificate leaf = H\"01\"",
        )
        .unwrap();

        for (session, broker, target) in [
            ("first", "broker-first", &first),
            ("second", "broker-second", &second),
        ] {
            gate.register_broker(session, broker);
            let challenge = match gate.check(session, target).unwrap() {
                ApprovalCheck::Required(challenge) => challenge,
                ApprovalCheck::Approved => panic!("unexpected approval"),
            };
            gate.resolve(
                session,
                broker,
                &challenge.id,
                ApprovalAction::Accept,
                ApprovalPersistence::Always,
            )
            .unwrap();
        }
        assert_eq!(gate.list_persistent().unwrap().len(), 2);
        assert!(gate.revoke_persistent("com.example.Editor").unwrap());
        assert!(gate.list_persistent().unwrap().is_empty());
    }

    #[test]
    fn persistent_store_uses_an_exclusive_private_interprocess_lock() {
        let (_directory, gate) = gate(true);
        let first = StoreFileLock::acquire(&gate.store_path).unwrap();
        let second = StoreFileLock::try_acquire(&gate.store_path).unwrap_err();
        assert!(matches!(second, ApprovalError::Store(_)));
        drop(first);
        let reacquired = StoreFileLock::try_acquire(&gate.store_path).unwrap();
        let lock_path = gate.store_path.with_extension("lock");
        assert_eq!(
            std::fs::metadata(lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(reacquired);
    }

    #[test]
    fn persistent_store_rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let (directory, gate) = gate(true);
        std::fs::create_dir_all(gate.store_path.parent().unwrap()).unwrap();
        let victim = directory.path().join("victim.json");
        std::fs::write(&victim, b"{}\n").unwrap();
        symlink(&victim, &gate.store_path).unwrap();
        gate.register_broker("session", "broker");
        let error = gate.check("session", &target()).unwrap_err();
        assert!(matches!(error, ApprovalError::Store(_)));
    }
}
