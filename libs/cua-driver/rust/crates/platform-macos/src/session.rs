//! Security-session capability and lock-state lifecycle support.
//!
//! AppKit's `+[NSApplication sharedApplication]` registers the process with
//! the Window Server (`_RegisterApplication`). When the calling process has
//! no graphic-session access — `cua-driver mcp` spawned as a stdio child from
//! an SSH session, a `LaunchDaemon`, or a headless CI runner — that
//! registration **aborts the whole process with SIGABRT** before any error
//! reaches the caller (issue #1724).
//!
//! `SessionGetInfo` answers "can this session talk to the Window Server?"
//! *without* touching AppKit, so we can gate overlay init on it and degrade
//! to a headless MCP server instead of crashing. This is the macOS analogue
//! of the Session-0 short-circuit guard used on Windows.

use core_foundation::base::TCFType;
use core_foundation::boolean::{kCFBooleanTrue, CFBoolean};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

// `SecuritySessionId` and `SessionAttributeBits` are both 32-bit ints in
// <Security/AuthSession.h>.
type SecuritySessionId = i32;
type SessionAttributeBits = u32;

/// `callerSecuritySession` — the session of the calling process.
const CALLER_SECURITY_SESSION: SecuritySessionId = -1;
/// `sessionHasGraphicAccess` — the session can use the Window Server.
const SESSION_HAS_GRAPHIC_ACCESS: SessionAttributeBits = 0x0010;

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SessionGetInfo(
        session: SecuritySessionId,
        session_id: *mut SecuritySessionId,
        attributes: *mut SessionAttributeBits,
    ) -> i32; // OSStatus; errSecSuccess == 0
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

/// Whether the current security session can use the Window Server (and thus
/// safely initialize AppKit). On any failure we return `false` — skipping the
/// overlay is always recoverable, but a wrong `true` re-introduces the
/// `_RegisterApplication` SIGABRT (issue #1724).
pub fn has_graphic_access() -> bool {
    let mut session_id: SecuritySessionId = 0;
    let mut attributes: SessionAttributeBits = 0;
    let status =
        unsafe { SessionGetInfo(CALLER_SECURITY_SESSION, &mut session_id, &mut attributes) };
    status == 0 && (attributes & SESSION_HAS_GRAPHIC_ACCESS) != 0
}

/// Read the Window Server's current lock flag without touching AppKit.
///
/// `None` means the current process cannot inspect the login session. Callers
/// must fail closed in that case instead of assuming an unlocked desktop.
pub fn current_screen_locked() -> Option<bool> {
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return None;
    }
    let session: CFDictionary<CFString, CFBoolean> =
        unsafe { TCFType::wrap_under_create_rule(raw) };
    let key = CFString::new("CGSSessionScreenIsLocked");
    Some(
        session
            .find(&key)
            .map(|value| value.as_concrete_TypeRef() == unsafe { kCFBooleanTrue })
            .unwrap_or(false),
    )
}

/// Token proving that the caller observed an unlocked session at one point in
/// the current lock lifecycle. Tokens become permanently stale on either a
/// lock or unlock transition, even if both happen between two checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionLockEpoch(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionLockError {
    Locked,
    EpochChanged,
}

type LockHook = Arc<dyn Fn() + Send + Sync>;

/// Event-driven login-session guardian shared by Computer Use callers.
///
/// The atomics make checks cheap at native-dispatch boundaries. `transition`
/// serializes notification delivery with cleanup hooks so an unlock cannot
/// race ahead of the cleanup caused by its preceding lock.
pub struct SessionLockGuardian {
    locked: AtomicBool,
    epoch: AtomicU64,
    transition: Mutex<()>,
    lock_hooks: Mutex<Vec<LockHook>>,
}

impl SessionLockGuardian {
    fn new(initially_locked: bool) -> Self {
        Self {
            locked: AtomicBool::new(initially_locked),
            epoch: AtomicU64::new(1),
            transition: Mutex::new(()),
            lock_hooks: Mutex::new(Vec::new()),
        }
    }

    /// Capture the current epoch only when the session is unlocked.
    pub fn begin(&self) -> Result<SessionLockEpoch, SessionLockError> {
        loop {
            let before = self.epoch.load(Ordering::SeqCst);
            if self.locked.load(Ordering::SeqCst) {
                return Err(SessionLockError::Locked);
            }
            let after = self.epoch.load(Ordering::SeqCst);
            if before == after {
                return Ok(SessionLockEpoch(after));
            }
        }
    }

    /// Reject a token if the screen is locked or the lifecycle changed since
    /// the token was issued.
    pub fn validate(&self, token: SessionLockEpoch) -> Result<(), SessionLockError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(SessionLockError::Locked);
        }
        if self.epoch.load(Ordering::SeqCst) != token.0 {
            return Err(SessionLockError::EpochChanged);
        }
        if self.locked.load(Ordering::SeqCst) {
            return Err(SessionLockError::Locked);
        }
        Ok(())
    }

    /// Reconcile an event or direct Window Server probe with the guardian.
    /// Repeated observations of the same state are intentionally idempotent.
    pub fn observe(&self, locked: bool) {
        let _transition = self.transition.lock().unwrap();
        if self.locked.load(Ordering::SeqCst) == locked {
            return;
        }
        self.locked.store(locked, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if locked {
            for hook in self.lock_hooks.lock().unwrap().iter() {
                hook();
            }
        }
    }

    /// Register state cleanup that must run on every unlocked-to-locked edge.
    /// A subscriber added while locked is invoked once immediately.
    pub fn register_lock_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        let hook: LockHook = Arc::new(hook);
        let already_locked = {
            let _transition = self.transition.lock().unwrap();
            self.lock_hooks.lock().unwrap().push(hook.clone());
            self.locked.load(Ordering::SeqCst)
        };
        if already_locked {
            hook();
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(initially_locked: bool) -> Arc<Self> {
        Arc::new(Self::new(initially_locked))
    }
}

/// Process-wide guardian. The first access installs push-based AppKit and
/// distributed lock observers when a graphic session is available. Direct
/// `CGSessionCopyCurrentDictionary` checks remain available to callers as a
/// fallback when notifications are unavailable or delayed.
pub fn lock_guardian() -> &'static Arc<SessionLockGuardian> {
    static GUARDIAN: OnceLock<Arc<SessionLockGuardian>> = OnceLock::new();
    GUARDIAN.get_or_init(|| {
        let guardian = Arc::new(SessionLockGuardian::new(
            current_screen_locked().unwrap_or(true),
        ));
        if has_graphic_access() {
            install_lock_observers(&guardian);
        }
        guardian
    })
}

/// Re-read the authoritative Window Server flag after an unlock-like event.
/// If the probe is unavailable, retain the locked state and fail closed.
fn reconcile_after_unlock_event(guardian: &SessionLockGuardian) {
    if let Some(locked) = current_screen_locked() {
        guardian.observe(locked);
    }
}

/// Observe both the public workspace session lifecycle and loginwindow's
/// screen-lock distributed notifications. The latter covers ordinary Lock
/// Screen transitions that do not switch to another fast-user-switch session.
fn install_lock_observers(guardian: &Arc<SessionLockGuardian>) {
    use block2::RcBlock;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceSessionDidBecomeActiveNotification,
        NSWorkspaceSessionDidResignActiveNotification,
    };
    use objc2_foundation::{
        NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSString,
    };
    use std::ptr::NonNull;

    let queue = unsafe { NSOperationQueue::new() };
    unsafe { queue.setMaxConcurrentOperationCount(1) };

    let lock_guardian = guardian.clone();
    let lock_block = RcBlock::new(move |_note: NonNull<NSNotification>| {
        lock_guardian.observe(true);
    });
    let unlock_guardian = guardian.clone();
    let unlock_block = RcBlock::new(move |_note: NonNull<NSNotification>| {
        reconcile_after_unlock_event(&unlock_guardian);
    });

    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let workspace_center = unsafe { workspace.notificationCenter() };
    let session_resign = unsafe {
        workspace_center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceSessionDidResignActiveNotification),
            None,
            Some(&queue),
            &lock_block,
        )
    };
    let session_become = unsafe {
        workspace_center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceSessionDidBecomeActiveNotification),
            None,
            Some(&queue),
            &unlock_block,
        )
    };

    let distributed_center = unsafe { NSDistributedNotificationCenter::defaultCenter() };
    let screen_locked_name = NSString::from_str("com.apple.screenIsLocked");
    let screen_unlocked_name = NSString::from_str("com.apple.screenIsUnlocked");
    let screen_locked = unsafe {
        distributed_center.addObserverForName_object_queue_usingBlock(
            Some(&screen_locked_name),
            None,
            Some(&queue),
            &lock_block,
        )
    };
    let screen_unlocked = unsafe {
        distributed_center.addObserverForName_object_queue_usingBlock(
            Some(&screen_unlocked_name),
            None,
            Some(&queue),
            &unlock_block,
        )
    };

    // The observer set is process-lifetime. Retaining the queue and tokens
    // also avoids placing Objective-C objects with thread-affinity constraints
    // inside the Send + Sync guardian.
    std::mem::forget(session_resign);
    std::mem::forget(session_become);
    std::mem::forget(screen_locked);
    std::mem::forget(screen_unlocked);
    std::mem::forget(queue);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe must resolve the `Security` symbol and return a `bool`
    /// without aborting. We can't assert the value: it's `true` on a dev GUI
    /// session and `false` on a headless CI runner — which is exactly the
    /// distinction the guard relies on (issue #1724).
    #[test]
    fn graphic_access_probe_does_not_abort() {
        let _ = has_graphic_access();
    }

    #[test]
    fn lock_epoch_changes_and_cleanup_runs_once_per_lock_edge() {
        let guardian = SessionLockGuardian::for_test(false);
        let cleanup_count = Arc::new(AtomicU64::new(0));
        let count = cleanup_count.clone();
        guardian.register_lock_hook(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });

        let first = guardian.begin().unwrap();
        guardian.observe(true);
        assert_eq!(guardian.validate(first), Err(SessionLockError::Locked));
        guardian.observe(true);
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);

        guardian.observe(false);
        assert_eq!(
            guardian.validate(first),
            Err(SessionLockError::EpochChanged)
        );
        let second = guardian.begin().unwrap();
        guardian.observe(true);
        assert_eq!(guardian.validate(second), Err(SessionLockError::Locked));
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 2);
    }
}
