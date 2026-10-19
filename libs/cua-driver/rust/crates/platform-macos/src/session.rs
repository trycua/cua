//! Security-session capability probe.
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

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> core_foundation::dictionary::CFDictionaryRef;
}

/// Whether the login session currently reports its screen as locked
/// (`CGSSessionScreenIsLocked` in `CGSessionCopyCurrentDictionary`).
///
/// Read fresh on every call; lock state changes at any time. On macOS 14 and
/// later the screen saver and display sleep put the session behind the lock
/// screen (`loginwindow` frontmost) even before a password is required, and
/// while that holds every app's `AXWindows` reads as empty. `false` when the
/// session dictionary is unavailable (no window-server session) or the key is
/// absent, which is how an unlocked session reports itself.
pub fn screen_locked() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return false;
    }
    // SAFETY: the "Copy" function returns a +1 reference we now own.
    let dict: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(raw) };
    let key = CFString::from_static_string("CGSSessionScreenIsLocked");
    let Some(value) = dict.find(key.as_CFTypeRef() as *const _) else {
        return false;
    };
    let value = unsafe { core_foundation::base::CFType::wrap_under_get_rule(*value as _) };
    if let Some(flag) = value.downcast::<CFBoolean>() {
        return flag.into();
    }
    value
        .downcast::<CFNumber>()
        .and_then(|number| number.to_i64())
        .is_some_and(|number| number != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves the CoreGraphics symbol and reads the session dictionary
    /// without crashing, with or without a window-server session.
    #[test]
    fn screen_lock_probe_does_not_abort() {
        let _ = screen_locked();
    }

    /// The probe must resolve the `Security` symbol and return a `bool`
    /// without aborting. We can't assert the value: it's `true` on a dev GUI
    /// session and `false` on a headless CI runner — which is exactly the
    /// distinction the guard relies on (issue #1724).
    #[test]
    fn graphic_access_probe_does_not_abort() {
        let _ = has_graphic_access();
    }
}
