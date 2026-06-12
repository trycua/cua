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
}
