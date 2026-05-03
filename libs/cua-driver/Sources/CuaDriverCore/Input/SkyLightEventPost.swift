import CoreGraphics
import Darwin
import Foundation
import ObjectiveC

/// Bridge to SkyLight's private per-pid event-post path, with the
/// `SLSEventAuthenticationMessage` envelope Chromium requires.
///
/// Two-layer story:
///
/// 1. **Post path** — `SLEventPostToPid` in `SkyLight.framework` wraps
///    `SLEventPostToPSN` → `CGSTickleActivityMonitor` →
///    `SLSUpdateSystemActivityWithLocation` → `IOHIDPostEvent`. The
///    public `CGEventPostToPid` skips the activity-monitor tickle, so
///    events delivered through it reach the target's mach port but
///    don't register as "live input" — which Chromium needs to enter
///    its keyboard pipeline.
///
/// 2. **Authentication** — on macOS 14+, WindowServer gates synthetic
///    keyboard events against Chromium-like targets on an attached
///    `SLSEventAuthenticationMessage`. The message is constructed
///    per-event via `+[SLSEventAuthenticationMessage
///    messageWithEventRecord:pid:version:]`, which reads the
///    `SLSEventRecord *` embedded in a `CGEvent`, wraps it with the
///    target pid + version (0 is accepted for the unsigned path),
///    and returns an appropriate subclass instance (e.g.
///    `SLSSkyLightKeyEventAuthenticationMessage` for keyboard
///    events). Attached via `SLEventSetAuthenticationMessage`,
///    Chrome accepts the event as authentic input and the omnibox
///    commits the URL — backgrounded, no focus-steal.
///
/// `CGEventRef` and `SLEventRef` share the same opaque
/// `__CGEvent *` representation, so events built with the public
/// `CGEventCreateKeyboardEvent` go through this path untouched.
///
/// All symbols + classes are resolved once at first use via `dlopen`
/// + `dlsym` + `NSClassFromString`. If anything fails to resolve the
/// bridge returns `false` from `postToPid` and callers fall back to
/// the public `CGEventPostToPid`.
public enum SkyLightEventPost {
    // MARK: - Function-pointer typedefs

    /// `void SLEventPostToPid(pid_t, CGEventRef)`
    private typealias PostToPidFn = @convention(c) (pid_t, CGEvent) -> Void
    /// `void SLEventSetAuthenticationMessage(CGEventRef, id)`
    private typealias SetAuthMessageFn = @convention(c) (CGEvent, AnyObject) -> Void
    /// `void SLEventSetIntegerValueField(CGEventRef, CGEventField, int64_t)`.
    /// SkyLight's raw-field SPI — the public `CGEventSetIntegerValueField`
    /// is restricted to the standard field indexes, whereas this writes
    /// any raw Skylight field index (including the private f51/f91/f92
    /// session-id fields).
    private typealias SetIntFieldFn = @convention(c) (CGEvent, UInt32, Int64) -> Void
    /// `CGSConnectionID CGSMainConnectionID(void)` — Skylight's main
    /// connection handle for the current process. Candidate source for
    /// the "session ID" value stamped into mouse-event fields
    /// f51/f91/f92 (observed value: 24093).
    private typealias ConnectionIDFn = @convention(c) () -> UInt32
    /// `void CGEventSetWindowLocation(CGEventRef, CGPoint)` — private
    /// SPI that stamps a window-local point onto the event alongside
    /// the screen-space location set via the public
    /// `CGEventSetLocation`. Stamped when the target is backgrounded
    /// so WindowServer's hit-test uses the window-local point
    /// directly instead of re-projecting from screen space.
    private typealias SetWindowLocationFn = @convention(c) (CGEvent, CGPoint) -> Void
    /// `objc_msgSend` specialised for
    /// `+[SLSEventAuthenticationMessage messageWithEventRecord:pid:version:]`:
    /// `(Class, SEL, SLSEventRecord *, int32_t, uint32_t) -> id`.
    private typealias FactoryMsgSendFn = @convention(c) (
        AnyObject, Selector, UnsafeMutableRawPointer, Int32, UInt32
    ) -> AnyObject?

    // MARK: - Cached resolved handles

    private struct Resolved {
        let postToPid: PostToPidFn
        let setAuthMessage: SetAuthMessageFn
        let msgSendFactory: FactoryMsgSendFn
        let messageClass: AnyClass
        let factorySelector: Selector
    }

    private static let resolved: Resolved? = {
        // dlopen SkyLight so its ObjC classes register + the symbols
        // resolve. Without this, NSClassFromString misses the classes
        // on processes that don't otherwise link SkyLight transitively.
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)

        func fn<T>(_ name: String, as _: T.Type) -> T? {
            guard
                let p = dlsym(
                    UnsafeMutableRawPointer(bitPattern: -2), name)
            else { return nil }
            return unsafeBitCast(p, to: T.self)
        }

        guard
            let postToPid = fn("SLEventPostToPid", as: PostToPidFn.self),
            let setAuth = fn(
                "SLEventSetAuthenticationMessage", as: SetAuthMessageFn.self),
            let msgSend = fn("objc_msgSend", as: FactoryMsgSendFn.self),
            let messageClass = NSClassFromString(
                "SLSEventAuthenticationMessage")
        else { return nil }

        return Resolved(
            postToPid: postToPid,
            setAuthMessage: setAuth,
            msgSendFactory: msgSend,
            messageClass: messageClass,
            factorySelector: NSSelectorFromString(
                "messageWithEventRecord:pid:version:")
        )
    }()

    /// `SLEventSetIntegerValueField` resolved separately so its
    /// absence doesn't disable the auth-signed post path. Used by
    /// the 5-event backgrounded-click recipe to stamp private raw
    /// fields (f51/f91/f92) that the public CGEvent SPI can't reach.
    private static let setIntField: SetIntFieldFn? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        guard
            let p = dlsym(
                UnsafeMutableRawPointer(bitPattern: -2),
                "SLEventSetIntegerValueField")
        else { return nil }
        return unsafeBitCast(p, to: SetIntFieldFn.self)
    }()

    /// `CGSMainConnectionID` resolved once. Returns the Skylight main
    /// connection handle for the current process (the session-ID
    /// candidate).
    private static let connectionIDFn: ConnectionIDFn? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        guard
            let p = dlsym(
                UnsafeMutableRawPointer(bitPattern: -2),
                "CGSMainConnectionID")
        else { return nil }
        return unsafeBitCast(p, to: ConnectionIDFn.self)
    }()

    /// `CGEventSetWindowLocation` resolved once. Private SPI — ships
    /// in SkyLight/CoreGraphics but is not in the public headers.
    /// `RTLD_DEFAULT` = bitPattern -2 matches the rest of this file's
    /// `dlsym` sites.
    private static let setWindowLocationFn: SetWindowLocationFn? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        guard
            let p = dlsym(
                UnsafeMutableRawPointer(bitPattern: -2),
                "CGEventSetWindowLocation")
        else { return nil }
        return unsafeBitCast(p, to: SetWindowLocationFn.self)
    }()

    // MARK: - Focus-without-raise SPIs
    //
    // Used by `FocusWithoutRaise` to put a target app into AppKit-active
    // state without asking WindowServer to reorder its windows. Recipe
    // ported from yabai's `window_manager_focus_window_without_raise`.

    /// `OSStatus SLPSPostEventRecordTo(ProcessSerialNumber *psn, uint8_t *bytes)`.
    /// Posts a 248-byte synthetic event record into the target process's
    /// Carbon event queue. We build the buffer with
    /// `bytes[0x04]=0xf8, bytes[0x08]=0x0d` plus a focus/defocus marker at
    /// `bytes[0x8a]` (0x01 = focus, 0x02 = defocus) and the target window
    /// id at bytes 0x3c-0x3f.
    private typealias PostEventRecordToFn = @convention(c) (
        UnsafeRawPointer, UnsafePointer<UInt8>
    ) -> Int32

    /// `OSStatus _SLPSGetFrontProcess(ProcessSerialNumber *psn)`. Writes
    /// the current frontmost process's PSN into the 8 bytes at `psn`.
    private typealias GetFrontProcessFn = @convention(c) (
        UnsafeMutableRawPointer
    ) -> Int32

    /// `OSStatus GetProcessForPID(pid_t, ProcessSerialNumber *)`.
    /// Deprecated but still resolves via dlsym. Writes the target pid's
    /// PSN into the 8 bytes at `psn`.
    private typealias GetProcessForPIDFn = @convention(c) (
        pid_t, UnsafeMutableRawPointer
    ) -> Int32

    private static let postEventRecordToFn: PostEventRecordToFn? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        guard
            let p = dlsym(
                UnsafeMutableRawPointer(bitPattern: -2),
                "SLPSPostEventRecordTo")
        else { return nil }
        return unsafeBitCast(p, to: PostEventRecordToFn.self)
    }()

    private static let getFrontProcessFn: GetFrontProcessFn? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        guard
            let p = dlsym(
                UnsafeMutableRawPointer(bitPattern: -2),
                "_SLPSGetFrontProcess")
        else { return nil }
        return unsafeBitCast(p, to: GetFrontProcessFn.self)
    }()

    private static let getProcessForPIDFn: GetProcessForPIDFn? = {
        guard
            let p = dlsym(
                UnsafeMutableRawPointer(bitPattern: -2),
                "GetProcessForPID")
        else { return nil }
        return unsafeBitCast(p, to: GetProcessForPIDFn.self)
    }()

    /// `true` when the full auth-signed post path resolves. Useful
    /// for callers that want to log which route they took.
    public static var isAvailable: Bool { resolved != nil }

    // MARK: - Public entry point

    /// Post `event` to `pid` via `SLEventPostToPid`.
    ///
    /// `attachAuthMessage` controls whether an `SLSEventAuthenticationMessage`
    /// is attached before posting:
    ///
    /// - `true` (default, keyboard path): attaches the auth message so
    ///   Chromium accepts synthetic keyboard events as trusted input and
    ///   the omnibox commits URLs. Required for keyboard on macOS 14+.
    ///
    /// - `false` (mouse path): skips the auth message so the event
    ///   routes via `SLEventPostToPid → SLEventPostToPSN → IOHIDPostEvent`,
    ///   which flows through the `cgAnnotatedSessionEventTap` pipeline that
    ///   Chromium's window event handler subscribes to. Attaching the auth
    ///   message forks `SLEventPostToPid` onto a direct-mach delivery path
    ///   that bypasses `IOHIDPostEvent` — mouse events posted with the
    ///   auth message don't appear in `cgAnnotatedSessionEventTap`
    ///   captures (verified by comparing tap streams with/without the
    ///   auth envelope), so Chromium's window event handler never sees
    ///   them. Mouse path skips the envelope for that reason.
    ///
    /// Returns `true` when the SPI resolved and the post was attempted;
    /// `false` when anything in the chain is missing (caller falls back
    /// to the public `CGEvent.postToPid`).
    @discardableResult
    public static func postToPid(
        _ pid: pid_t, event: CGEvent, attachAuthMessage: Bool = true
    ) -> Bool {
        guard let r = resolved else { return false }
        if attachAuthMessage {
            if let record = extractEventRecord(from: event) {
                if let msg = r.msgSendFactory(
                    r.messageClass as AnyObject,
                    r.factorySelector,
                    record,
                    pid,
                    0
                ) {
                    r.setAuthMessage(event, msg)
                }
                // On nil auth message we still attempt the post — the
                // unsigned path is valid on older OS releases and worth a
                // try before falling back to the public CGEvent.postToPid.
            }
        }
        r.postToPid(pid, event)
        return true
    }

    /// Stamp `value` onto `event` at the raw Skylight field index
    /// `field` using `SLEventSetIntegerValueField`. Returns `false`
    /// when the SPI didn't resolve so callers can fall back to
    /// `CGEvent.setIntegerValueField(_:value:)` for the public fields.
    @discardableResult
    public static func setIntegerField(
        _ event: CGEvent, field: UInt32, value: Int64
    ) -> Bool {
        guard let fn = setIntField else { return false }
        fn(event, field, value)
        return true
    }

    /// Skylight main connection ID for the current process, when
    /// `CGSMainConnectionID` resolves. Appears to be the per-session
    /// stamp consumed by private mouse-event fields f51/f91/f92.
    public static var mainConnectionID: UInt32? {
        guard let fn = connectionIDFn else { return nil }
        return fn()
    }

    /// Stamp a window-local `point` onto `event` via the private
    /// `CGEventSetWindowLocation` SPI. Returns `true` when the SPI
    /// resolved and was called; `false` when the symbol wasn't
    /// available (caller decides whether to continue with just the
    /// screen-space location set via `CGEventSetLocation`). The SPI
    /// is `void`-returning so "called" is the strongest claim — the
    /// caller must verify effect at the application level.
    @discardableResult
    public static func setWindowLocation(
        _ event: CGEvent, _ point: CGPoint
    ) -> Bool {
        guard let fn = setWindowLocationFn else { return false }
        fn(event, point)
        return true
    }

    /// `true` when `CGEventSetWindowLocation` resolved. Useful for
    /// log-lines that note whether the window-local path is active.
    public static var isWindowLocationAvailable: Bool {
        setWindowLocationFn != nil
    }

    /// Copy the current frontmost process's PSN into the provided
    /// 8-byte buffer. Returns `false` when `_SLPSGetFrontProcess` isn't
    /// resolvable.
    public static func getFrontProcess(
        _ psnBuffer: UnsafeMutableRawPointer
    ) -> Bool {
        guard let fn = getFrontProcessFn else { return false }
        return fn(psnBuffer) == 0
    }

    /// Resolve `pid` to its PSN via the deprecated `GetProcessForPID`
    /// SPI, writing 8 bytes into `psnBuffer`. Returns `false` when the
    /// SPI isn't resolvable or the call failed.
    public static func getProcessPSN(
        forPid pid: pid_t, into psnBuffer: UnsafeMutableRawPointer
    ) -> Bool {
        guard let fn = getProcessForPIDFn else { return false }
        return fn(pid, psnBuffer) == 0
    }

    /// Post a 248-byte synthetic event record via
    /// `SLPSPostEventRecordTo`. Caller is responsible for building the
    /// buffer with the right defocus/focus marker and target window id.
    /// Returns `false` when the SPI isn't resolvable.
    @discardableResult
    public static func postEventRecordTo(
        psn: UnsafeRawPointer, bytes: UnsafePointer<UInt8>
    ) -> Bool {
        guard let fn = postEventRecordToFn else { return false }
        return fn(psn, bytes) == 0
    }

    /// `true` when all three focus-without-raise SPIs
    /// (`_SLPSGetFrontProcess`, `GetProcessForPID`, `SLPSPostEventRecordTo`)
    /// resolved — i.e. `FocusWithoutRaise.activateWithoutRaise` has its
    /// prerequisites.
    public static var isFocusWithoutRaiseAvailable: Bool {
        getFrontProcessFn != nil && getProcessForPIDFn != nil
            && postEventRecordToFn != nil
    }

    // MARK: - Event-record extraction

    /// Extract the embedded `SLSEventRecord *` from a `CGEvent`. The
    /// layout of `__CGEvent` exposed by SkyLight's ObjC type
    /// encodings is `{CFRuntimeBase, uint32_t, SLSEventRecord *}` —
    /// on 64-bit that puts the record pointer at offset 24
    /// (CFRuntimeBase=16 + uint32=4 + 4 bytes padding for
    /// pointer alignment). We probe a few adjacent offsets for
    /// resilience across OS revisions.
    private static func extractEventRecord(from event: CGEvent)
        -> UnsafeMutableRawPointer?
    {
        let base = Unmanaged.passUnretained(event).toOpaque()
        for offset in [24, 32, 16] {
            let slot = base.advanced(by: offset).assumingMemoryBound(
                to: UnsafeMutableRawPointer?.self)
            if let p = slot.pointee { return p }
        }
        return nil
    }

    // MARK: - Window alpha (stealth minimize/deminiaturize)

    private typealias SetWindowAlphaFn = @convention(c) (
        UInt32, UInt32, Float
    ) -> CGError
    private typealias GetWindowAlphaFn = @convention(c) (
        UInt32, UInt32, UnsafeMutablePointer<Float>
    ) -> CGError

    private static let setWindowAlphaFn: SetWindowAlphaFn? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        guard let p = dlsym(
            UnsafeMutableRawPointer(bitPattern: -2),
            "SLSSetWindowAlpha")
        else { return nil }
        return unsafeBitCast(p, to: SetWindowAlphaFn.self)
    }()

    /// Set the compositor-level alpha of a CGWindowID. 0.0 = invisible,
    /// 1.0 = opaque. Used for stealth deminiaturize: set alpha=0 before
    /// un-minimizing, perform AX action, re-minimize, then restore alpha.
    /// The user never sees the window flash.
    public static func setWindowAlpha(windowID: CGWindowID, alpha: Float) -> Bool {
        guard let fn = setWindowAlphaFn, let cid = connectionIDFn?() else {
            return false
        }
        return fn(cid, UInt32(windowID), alpha) == .success
    }

    /// Get all CGWindowIDs belonging to `pid` (all layers, including
    /// off-screen / minimized).
    public static func windowIDs(forPid pid: pid_t) -> [CGWindowID] {
        guard let all = CGWindowListCopyWindowInfo(
            [.optionAll], kCGNullWindowID
        ) as? [[String: Any]] else { return [] }
        return all.compactMap { info -> CGWindowID? in
            guard (info[kCGWindowOwnerPID as String] as? Int32) == pid
            else { return nil }
            return CGWindowID(info[kCGWindowNumber as String] as! Int)
        }
    }
}
