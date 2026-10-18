// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Await-token-file mode: the root token comes only from a file that an
//! orchestrator writes (a Fleet claim Secret mounted at `/run/cua`), never
//! from the network.
//!
//! The file is empty until a claim binds, updated in place on rotation and
//! emptied on release. [`TokenFileWatcher`] polls it (inotify, where it
//! works, only wakes the poll early; gVisor may not deliver events) and
//! drives [`ServerContext::apply_file_token`]:
//!
//! - non-empty and valid: install (first time) or rotate;
//! - empty, missing, invalid or insecure: revoke (back to awaiting).
//!
//! While awaiting, the gRPC surface serves only `GetCapabilities` and
//! `Health`; everything else is `FAILED_PRECONDITION` / `NOT_INITIALIZED`.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::context::ServerContext;

/// Default poll interval.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Shortest accepted token (after trimming).
pub const MIN_TOKEN_LEN: usize = 16;
/// Longest accepted token (after trimming).
pub const MAX_TOKEN_LEN: usize = 4096;
/// Bytes read from the file at most; anything longer is invalid.
const MAX_FILE_BYTES: u64 = MAX_TOKEN_LEN as u64 + 64;
/// Delay before re-reading a changed file, so a non-atomic writer's partial
/// content is never installed.
const SETTLE: Duration = Duration::from_millis(40);

/// What the token file holds right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenFileState {
    /// A valid token (trimmed).
    Token(String),
    /// Missing or empty (whitespace only): no claim bound.
    Empty,
    /// Present but unusable. The reason never contains the file contents.
    Refused(String),
}

impl TokenFileState {
    /// The token, if valid.
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Token(token) => Some(token),
            _ => None,
        }
    }
}

/// Checks a token's format: after trimming surrounding whitespace, empty
/// means "no token"; otherwise [`MIN_TOKEN_LEN`]..=[`MAX_TOKEN_LEN`] bytes of
/// the RFC 6750 `b64token` alphabet (`A-Z a-z 0-9 - . _ ~ + / =`), so it is
/// always a valid bearer header value.
pub fn validate_token(raw: &str) -> Result<Option<&str>, String> {
    let token = raw.trim();
    if token.is_empty() {
        return Ok(None);
    }
    if token.len() < MIN_TOKEN_LEN {
        return Err(format!(
            "token is too short ({} bytes, minimum {MIN_TOKEN_LEN})",
            token.len()
        ));
    }
    if token.len() > MAX_TOKEN_LEN {
        return Err(format!("token is too long (maximum {MAX_TOKEN_LEN} bytes)"));
    }
    if let Some(bad) = token
        .bytes()
        .position(|b| !(b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)))
    {
        return Err(format!(
            "token has a character outside [A-Za-z0-9-._~+/=] at byte {bad}"
        ));
    }
    Ok(Some(token))
}

/// Whether world-accessible token files are accepted by default: only when
/// running as root inside a container, where the orchestrator fixes the
/// mount mode (e.g. a virtiofs share or a Secret volume without
/// `defaultMode`) and no other local user exists to read it.
pub fn default_allow_world_readable() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        let root = unsafe { libc::geteuid() } == 0;
        root && in_container()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Best-effort container detection (docker, podman, Kubernetes, gVisor).
pub fn in_container() -> bool {
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || std::env::var_os("KUBERNETES_SERVICE_HOST").is_some()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|c| c.contains("kubepods") || c.contains("docker"))
            .unwrap_or(false)
}

/// Reads and validates the token file.
pub fn read_token_file(path: &Path, allow_world_readable: bool) -> TokenFileState {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return TokenFileState::Empty,
        Err(error) => return TokenFileState::Refused(format!("cannot open: {error}")),
    };
    let metadata = match file.metadata() {
        Ok(m) => m,
        Err(error) => return TokenFileState::Refused(format!("cannot stat: {error}")),
    };
    if !metadata.is_file() {
        return TokenFileState::Refused("not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o007 != 0 && !allow_world_readable {
            return TokenFileState::Refused(format!(
                "mode {mode:04o} is accessible to every user; make it 0600 (or 0640/0440 with a dedicated group)"
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = allow_world_readable;
    let mut contents = String::new();
    if let Err(error) = file.take(MAX_FILE_BYTES + 1).read_to_string(&mut contents) {
        return TokenFileState::Refused(format!("cannot read: {error}"));
    }
    if contents.len() as u64 > MAX_FILE_BYTES {
        return TokenFileState::Refused(format!("larger than {MAX_FILE_BYTES} bytes"));
    }
    match validate_token(&contents) {
        Ok(Some(token)) => TokenFileState::Token(token.to_owned()),
        Ok(None) => TokenFileState::Empty,
        Err(reason) => TokenFileState::Refused(reason),
    }
}

/// Reads the file until two reads [`SETTLE`] apart agree (at most a few
/// attempts), so a token being written non-atomically is never installed
/// half-written. Returns the last read when it never settles.
pub async fn read_settled(path: &Path, allow_world_readable: bool) -> TokenFileState {
    let mut previous = read_token_file(path, allow_world_readable);
    for _ in 0..5 {
        tokio::time::sleep(SETTLE).await;
        let current = read_token_file(path, allow_world_readable);
        if current == previous {
            return current;
        }
        previous = current;
    }
    previous
}

/// Writes `token` (or an empty file) to `path` atomically: a temp file in
/// the same directory, mode 0600, optionally chowned to `owner` (uid, gid),
/// fsynced, then renamed over `path`. Used by the privileged token-sync
/// helper and by tests.
pub fn write_token_atomically(
    path: &Path,
    token: Option<&str>,
    owner: Option<(u32, u32)>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("env-token");
    let tmp = dir.join(format!(".{name}.{}", crate::util::random_id(6)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::fchown(&file, Some(uid), Some(gid))?;
        }
        #[cfg(not(unix))]
        let _ = owner;
        if let Some(token) = token {
            file.write_all(token.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Configuration of a [`TokenFileWatcher`].
#[derive(Debug, Clone)]
pub struct TokenFileWatcher {
    /// The file to watch.
    pub path: PathBuf,
    /// Poll interval.
    pub interval: Duration,
    /// Accept a world-accessible file.
    pub allow_world_readable: bool,
}

impl TokenFileWatcher {
    /// A watcher for `path` with the default interval and permission policy.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            interval: DEFAULT_POLL_INTERVAL,
            allow_world_readable: default_allow_world_readable(),
        }
    }

    /// Applies the file's current state once, synchronously (no settle
    /// delay). Used before serving so a token already present is live on
    /// the first request.
    pub fn apply_now(&self, ctx: &ServerContext) -> TokenFileState {
        let state = read_token_file(&self.path, self.allow_world_readable);
        log_state(&self.path, &state, None);
        ctx.apply_file_token(state.token());
        state
    }

    /// Watches until the context shuts down. Must run inside a tokio runtime.
    pub fn spawn(self, ctx: ServerContext) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move { self.run(ctx).await })
    }

    async fn run(self, ctx: ServerContext) {
        let shutdown = ctx.shutdown_token();
        let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel::<()>(1);
        // Keep the notify watcher alive for the loop's lifetime.
        let _watcher = watch_parent(&self.path, wake_tx);
        let mut tick = tokio::time::interval(self.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last = read_token_file(&self.path, self.allow_world_readable);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tick.tick() => {}
                Some(()) = wake_rx.recv() => {}
            }
            let quick = read_token_file(&self.path, self.allow_world_readable);
            if quick == last && ctx.auth().token().as_deref() == last.token() {
                continue;
            }
            let state = read_settled(&self.path, self.allow_world_readable).await;
            log_state(&self.path, &state, Some(&last));
            ctx.apply_file_token(state.token());
            last = state;
        }
    }
}

/// The privileged half of await-token-file mode: copies a root-only token
/// file (the Fleet claim Secret, `root 0600`) to a file the unprivileged
/// driver can read (`0600`, owned by `owner`), validating it the same way.
/// An empty, missing, invalid or insecure source empties the target, so a
/// revocation propagates. The target's directory should be root-owned so the
/// unprivileged user cannot swap the file.
#[derive(Debug, Clone)]
pub struct TokenFileSync {
    /// Source (root-only).
    pub from: PathBuf,
    /// Target read by the driver.
    pub to: PathBuf,
    /// uid/gid that owns the target.
    pub owner: Option<(u32, u32)>,
    /// Poll interval.
    pub interval: Duration,
    /// Accept a world-accessible source.
    pub allow_world_readable: bool,
}

impl TokenFileSync {
    /// Reads the source and rewrites the target when the source's state
    /// differs from `last` (always when `last` is `None`). Returns the state
    /// now mirrored in the target.
    pub fn sync_once(&self, last: Option<&TokenFileState>) -> std::io::Result<TokenFileState> {
        let state = read_token_file(&self.from, self.allow_world_readable);
        if last != Some(&state) {
            log_state(&self.from, &state, last);
            write_token_atomically(&self.to, state.token(), self.owner)?;
        }
        Ok(state)
    }

    /// Mirrors the source until `stop` is cancelled. Write errors are logged
    /// and retried on the next tick.
    pub async fn run(self, stop: tokio_util::sync::CancellationToken) {
        let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel::<()>(1);
        let _watcher = watch_parent(&self.from, wake_tx);
        let mut tick = tokio::time::interval(self.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last: Option<TokenFileState> = None;
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tick.tick() => {}
                Some(()) = wake_rx.recv() => {}
            }
            let quick = read_token_file(&self.from, self.allow_world_readable);
            if last.as_ref() == Some(&quick) {
                continue;
            }
            // Settle before mirroring a change.
            if last.is_some() {
                tokio::time::sleep(SETTLE).await;
            }
            match self.sync_once(last.as_ref()) {
                Ok(state) => last = Some(state),
                Err(error) => {
                    tracing::warn!(to = %self.to.display(), %error, "token sync write failed; retrying");
                    last = None;
                }
            }
        }
    }
}

fn log_state(path: &Path, state: &TokenFileState, previous: Option<&TokenFileState>) {
    if previous == Some(state) {
        return;
    }
    let path = path.display();
    match state {
        TokenFileState::Token(_) => tracing::info!(%path, "token file holds a token"),
        TokenFileState::Empty => {
            tracing::info!(%path, "token file is empty or missing; awaiting a token")
        }
        TokenFileState::Refused(reason) => {
            tracing::warn!(%path, %reason, "token file refused; awaiting a valid token")
        }
    }
}

/// Whether a filesystem event can mean the token changed. Access events
/// (open, read, close without write) never do, and must not wake the loop:
/// notify's inotify backend reports `IN_OPEN`, and every wake reads the
/// file, so waking on opens made each read wake the next one (a busy loop
/// that kept `token-sync` at a steady ~28% CPU in the guest).
fn wakes_poll(event: &notify::Result<notify::Event>) -> bool {
    match event {
        Ok(event) => !matches!(event.kind, notify::EventKind::Access(_)),
        // A watcher error: poll, the tick covers anything missed.
        Err(_) => true,
    }
}

/// Wakes the poll loop on filesystem events in the file's directory (Secret
/// volumes swap a `..data` symlink there). Best effort: `None` when the
/// platform or filesystem does not support it.
fn watch_parent(
    path: &Path,
    wake: tokio::sync::mpsc::Sender<()>,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher as _;
    let dir = path.parent()?.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if wakes_poll(&event) {
            let _ = wake.try_send(());
        }
    })
    .ok()?;
    watcher
        .watch(&dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn token_format() {
        assert_eq!(validate_token(""), Ok(None));
        assert_eq!(validate_token(" \n\t"), Ok(None));
        assert_eq!(validate_token(&format!("  {GOOD}\n")), Ok(Some(GOOD)));
        assert!(validate_token("short").is_err());
        assert!(validate_token(&"a".repeat(MAX_TOKEN_LEN + 1)).is_err());
        assert!(validate_token(&"a".repeat(MAX_TOKEN_LEN)).is_ok());
        assert!(validate_token("abcdefgh ijklmnopq").is_err(), "inner space");
        assert!(
            validate_token("abcdefghijklmnop\u{e9}").is_err(),
            "non-ascii"
        );
        assert!(
            validate_token("abcdefghijklmnop\r\nx").is_err(),
            "header injection"
        );
        assert!(validate_token("aZ09-._~+/=aZ09-._~+/=").is_ok());
    }

    #[cfg(unix)]
    fn write_mode(path: &Path, contents: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_states_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env-token");
        assert_eq!(read_token_file(&path, false), TokenFileState::Empty);
        write_mode(&path, "", 0o600);
        assert_eq!(read_token_file(&path, false), TokenFileState::Empty);
        write_mode(&path, &format!("{GOOD}\n"), 0o600);
        assert_eq!(
            read_token_file(&path, false),
            TokenFileState::Token(GOOD.into())
        );
        write_mode(&path, GOOD, 0o640);
        assert!(read_token_file(&path, false).token().is_some(), "group ok");
        write_mode(&path, GOOD, 0o644);
        match read_token_file(&path, false) {
            TokenFileState::Refused(reason) => {
                assert!(reason.contains("0644"), "{reason}");
                assert!(!reason.contains(GOOD));
            }
            other => panic!("{other:?}"),
        }
        assert!(read_token_file(&path, true).token().is_some());
        write_mode(&path, "not a token!", 0o600);
        assert!(matches!(
            read_token_file(&path, false),
            TokenFileState::Refused(_)
        ));
        write_mode(&path, &"a".repeat(10_000), 0o600);
        assert!(matches!(
            read_token_file(&path, false),
            TokenFileState::Refused(_)
        ));
        let sub = dir.path().join("dir");
        std::fs::create_dir(&sub).unwrap();
        assert!(matches!(
            read_token_file(&sub, true),
            TokenFileState::Refused(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_is_0600_and_can_empty() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env-token");
        write_token_atomically(&path, Some(GOOD), None).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(read_token_file(&path, false).token(), Some(GOOD));
        write_token_atomically(&path, None, None).unwrap();
        assert_eq!(read_token_file(&path, false), TokenFileState::Empty);
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn sync_mirrors_install_rotate_revoke_and_refusals() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("src-token");
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let to = out.join("env-token");
        let sync = TokenFileSync {
            from: from.clone(),
            to: to.clone(),
            owner: None,
            interval: DEFAULT_POLL_INTERVAL,
            allow_world_readable: false,
        };
        // Missing source: the target exists and is empty.
        let s = sync.sync_once(None).unwrap();
        assert_eq!(s, TokenFileState::Empty);
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "");
        write_token_atomically(&from, Some(GOOD), None).unwrap();
        let s = sync.sync_once(Some(&s)).unwrap();
        assert_eq!(read_token_file(&to, false).token(), Some(GOOD));
        let mode = std::fs::metadata(&to).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let rotated = "rotated-0123456789abcdef";
        write_token_atomically(&from, Some(rotated), None).unwrap();
        let s = sync.sync_once(Some(&s)).unwrap();
        assert_eq!(read_token_file(&to, false).token(), Some(rotated));
        // An insecure source empties the target (fail closed).
        std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o644)).unwrap();
        let s = sync.sync_once(Some(&s)).unwrap();
        assert!(matches!(s, TokenFileState::Refused(_)));
        assert_eq!(read_token_file(&to, false), TokenFileState::Empty);
        write_token_atomically(&from, None, None).unwrap();
        let s = sync.sync_once(Some(&s)).unwrap();
        assert_eq!(s, TokenFileState::Empty);
        assert_eq!(read_token_file(&to, false), TokenFileState::Empty);
    }

    #[test]
    fn access_events_do_not_wake_the_poll() {
        use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind};
        use notify::{Event, EventKind};
        let wakes = |kind| wakes_poll(&Ok(Event::new(kind)));
        assert!(!wakes(EventKind::Access(AccessKind::Open(AccessMode::Any))));
        assert!(!wakes(EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!wakes(EventKind::Access(AccessKind::Read)));
        assert!(wakes(EventKind::Create(CreateKind::File)));
        assert!(wakes(EventKind::Modify(ModifyKind::Any)));
        assert!(wakes(EventKind::Remove(RemoveKind::File)));
        assert!(wakes(EventKind::Any));
        assert!(wakes_poll(&Err(notify::Error::generic("x"))));
    }

    /// Reading the watched file must not wake the loop (each wake reads it
    /// again, so that was a self-sustaining busy loop), while a write still
    /// does. inotify reports opens; other backends never did.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reading_the_watched_file_does_not_wake_the_poll() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env-token");
        write_token_atomically(&path, Some(GOOD), None).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
        let _watcher = watch_parent(&path, tx).expect("inotify watcher");
        tokio::time::sleep(Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {}
        for _ in 0..20 {
            assert_eq!(read_token_file(&path, false).token(), Some(GOOD));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(rx.try_recv().is_err(), "a read woke the poll loop");
        write_token_atomically(&path, None, None).unwrap();
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a write wakes the poll loop");
    }

    #[tokio::test]
    async fn settled_read_sees_final_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("env-token");
        write_token_atomically(&path, Some(GOOD), None).unwrap();
        assert_eq!(read_settled(&path, false).await.token(), Some(GOOD));
    }
}
