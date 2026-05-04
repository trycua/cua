import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

/// Private SPI that maps an AXUIElement representing a window to its
/// CGWindowID. Used by every major AX automation tool (yabai, Hammerspoon,
/// Accessibility Inspector) and stable since at least macOS 10.9. Returns
/// `AXError.success` on windows that have a backing CGWindowID; errors on
/// non-window elements or elements whose window hasn't been composited yet.
@_silgen_name("_AXUIElementGetWindow")
func _AXUIElementGetWindow(_ element: AXUIElement, _ windowId: UnsafeMutablePointer<CGWindowID>) -> AXError

// ApplicationServices.AXUIElement is a CFType and is thread-safe for the
// read + AXUIElementPerformAction / AXUIElementSetAttributeValue operations we
// perform. Apple hasn't annotated it Sendable, so we conform it retroactively.
extension AXUIElement: @retroactive @unchecked Sendable {}

// AXObserver is the other CFType we hand around here — we hold it in an
// actor-isolated dictionary to keep the pid's accessibility-client
// registration alive. Same retroactive-Sendable story as AXUIElement.
extension AXObserver: @retroactive @unchecked Sendable {}

/// No-op callback for ``AXObserverCreateWithInfoCallback``. The observer only
/// exists to make CuaDriver visible to the target app as an AX client — the
/// mere existence of at least one registered notification is what nudges
/// Chromium's browser process into full-accessibility mode (VS Code, Chrome,
/// Edge, and every other Blink-based shell). We never need to react to the
/// events themselves, so the callback discards every argument.
private let cuaDriverObserverNoopCallback: AXObserverCallbackWithInfo = {
    _, _, _, _, _ in
}

public struct AppStateSnapshot: Sendable, Codable {
    public let pid: Int32
    public let bundleId: String?
    public let name: String?
    public let treeMarkdown: String
    public let elementCount: Int
    public let turnId: Int
    /// Base64-encoded PNG of the target app's frontmost window. Absent
    /// when the caller didn't request a screenshot or when no on-screen
    /// window could be resolved for the pid.
    public let screenshotPngBase64: String?
    public let screenshotWidth: Int?
    public let screenshotHeight: Int?
    /// Pixels-per-point used when capturing the screenshot. The LLM reads
    /// pixel coordinates off the image but our click tools take POINTS
    /// (AX / CGEvent convention), so it needs this factor to convert:
    /// `point = pixel / scale_factor`. Always present when the screenshot
    /// fields are present.
    public let screenshotScaleFactor: Double?
    /// Original width before maxImageDimension resize. nil = no resize.
    public let screenshotOriginalWidth: Int?
    /// Original height before maxImageDimension resize. nil = no resize.
    public let screenshotOriginalHeight: Int?
    /// File-system path to the saved screenshot (JPEG) when the caller
    /// passed `screenshot_out_file`. Absent when no path was written.
    public let screenshotFilePath: String?

    public init(
        pid: Int32,
        bundleId: String?,
        name: String?,
        treeMarkdown: String,
        elementCount: Int,
        turnId: Int,
        screenshotPngBase64: String? = nil,
        screenshotWidth: Int? = nil,
        screenshotHeight: Int? = nil,
        screenshotScaleFactor: Double? = nil,
        screenshotOriginalWidth: Int? = nil,
        screenshotOriginalHeight: Int? = nil,
        screenshotFilePath: String? = nil
    ) {
        self.pid = pid
        self.bundleId = bundleId
        self.name = name
        self.treeMarkdown = treeMarkdown
        self.elementCount = elementCount
        self.turnId = turnId
        self.screenshotPngBase64 = screenshotPngBase64
        self.screenshotWidth = screenshotWidth
        self.screenshotHeight = screenshotHeight
        self.screenshotScaleFactor = screenshotScaleFactor
        self.screenshotOriginalWidth = screenshotOriginalWidth
        self.screenshotOriginalHeight = screenshotOriginalHeight
        self.screenshotFilePath = screenshotFilePath
    }

    private enum CodingKeys: String, CodingKey {
        case pid, name
        case bundleId = "bundle_id"
        case treeMarkdown = "tree_markdown"
        case elementCount = "element_count"
        case turnId = "turn_id"
        case screenshotPngBase64 = "screenshot_png_b64"
        case screenshotWidth = "screenshot_width"
        case screenshotHeight = "screenshot_height"
        case screenshotScaleFactor = "screenshot_scale_factor"
        case screenshotOriginalWidth = "screenshot_original_width"
        case screenshotOriginalHeight = "screenshot_original_height"
        case screenshotFilePath = "screenshot_file_path"
    }

    /// Encodes the struct with ``screenshot_*`` keys omitted when nil.
    /// Default-synthesized encoders emit explicit ``null`` for optional
    /// fields, but the tool's contract is "when no screenshot is taken,
    /// don't emit the keys at all" — that lets structured output stay
    /// byte-identical to the pre-screenshot response shape.
    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(pid, forKey: .pid)
        try container.encodeIfPresent(bundleId, forKey: .bundleId)
        try container.encodeIfPresent(name, forKey: .name)
        try container.encode(treeMarkdown, forKey: .treeMarkdown)
        try container.encode(elementCount, forKey: .elementCount)
        try container.encode(turnId, forKey: .turnId)
        try container.encodeIfPresent(screenshotPngBase64, forKey: .screenshotPngBase64)
        try container.encodeIfPresent(screenshotWidth, forKey: .screenshotWidth)
        try container.encodeIfPresent(screenshotHeight, forKey: .screenshotHeight)
        try container.encodeIfPresent(screenshotScaleFactor, forKey: .screenshotScaleFactor)
        try container.encodeIfPresent(screenshotOriginalWidth, forKey: .screenshotOriginalWidth)
        try container.encodeIfPresent(screenshotOriginalHeight, forKey: .screenshotOriginalHeight)
        try container.encodeIfPresent(screenshotFilePath, forKey: .screenshotFilePath)
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        pid = try container.decode(Int32.self, forKey: .pid)
        bundleId = try container.decodeIfPresent(String.self, forKey: .bundleId)
        name = try container.decodeIfPresent(String.self, forKey: .name)
        treeMarkdown = try container.decode(String.self, forKey: .treeMarkdown)
        elementCount = try container.decode(Int.self, forKey: .elementCount)
        turnId = try container.decode(Int.self, forKey: .turnId)
        screenshotPngBase64 = try container.decodeIfPresent(String.self, forKey: .screenshotPngBase64)
        screenshotWidth = try container.decodeIfPresent(Int.self, forKey: .screenshotWidth)
        screenshotHeight = try container.decodeIfPresent(Int.self, forKey: .screenshotHeight)
        screenshotScaleFactor = try container.decodeIfPresent(Double.self, forKey: .screenshotScaleFactor)
        screenshotOriginalWidth = try container.decodeIfPresent(Int.self, forKey: .screenshotOriginalWidth)
        screenshotOriginalHeight = try container.decodeIfPresent(Int.self, forKey: .screenshotOriginalHeight)
        screenshotFilePath = try container.decodeIfPresent(String.self, forKey: .screenshotFilePath)
    }
}

public enum AppStateError: Error, CustomStringConvertible, Sendable {
    case notAuthorized
    case appNotFound(Int32)
    case noCachedState(pid: Int32, windowId: UInt32)
    case invalidElementIndex(pid: Int32, windowId: UInt32, index: Int)

    public var description: String {
        switch self {
        case .notAuthorized:
            return "Accessibility permission not granted."
        case .appNotFound(let pid):
            return "App with pid \(pid) is not running or not accessible."
        case .noCachedState(let pid, let windowId):
            return
                "No cached AX state for pid \(pid) window_id \(windowId). "
                + "Call `get_window_state({pid: \(pid), window_id: \(windowId)})` "
                + "first to snapshot the window before any element-indexed action. "
                + "Element indices from one window do not resolve against another."
        case .invalidElementIndex(let pid, let windowId, let index):
            return
                "Invalid element_index \(index) for pid \(pid) window_id \(windowId) — "
                + "out of range in the cached snapshot. Re-run "
                + "`get_window_state({pid: \(pid), window_id: \(windowId)})` and use "
                + "an index from the fresh tree."
        }
    }
}

/// Builds and caches per-app AX snapshots. Exposed element_index values are
/// monotonic within a snapshot; the cache for a given pid is replaced every
/// time its `snapshot(pid:)` is called.
public actor AppStateEngine {
    /// Default init that builds a fresh ``AXEnablementAssertion`` — use when
    /// the caller doesn't need to share the assertion cache with other
    /// components (e.g. a ``FocusGuard``).
    public init() {
        self.enablement = AXEnablementAssertion()
    }

    /// Inject a shared ``AXEnablementAssertion`` so the server's per-action
    /// focus guard and the engine don't maintain independent caches of
    /// "which pids have we already enabled accessibility on?".
    public init(enablement: AXEnablementAssertion) {
        self.enablement = enablement
    }

    /// Depth cap for AX walks. Menus and complex web views can nest deep;
    /// 25 covers realistic app chrome without exploding on pathological trees.
    public static let maxDepth = 25

    /// Shared assertion that tracks which pids accept AXManualAccessibility /
    /// AXEnhancedUserInterface. Extracted into its own actor so a call-site-
    /// level ``FocusGuard`` can consult and update the same caches.
    private let enablement: AXEnablementAssertion

    /// Per-(pid, window_id) element_index cache.
    ///
    /// Scoping by window_id matters because the pid's AX tree spans every
    /// top-level window the app owns plus the menu bar — an element_index
    /// pulled from a snapshot of window A can point at a completely
    /// unrelated element in window B's tree layout. Keying on the caller-
    /// specified window_id prevents that cross-window aliasing: a
    /// `click({pid, window_id: A, element_index: 5})` won't accidentally
    /// resolve the 5th element of a snapshot that was taken for window B.
    private var sessions: [SessionKey: SessionState] = [:]
    private var nextTurnId: Int = 0

    private struct SessionKey: Hashable, Sendable {
        let pid: Int32
        let windowId: UInt32
    }
    /// Pids for which we've already completed the first-activation run-loop
    /// pump + observer registration. Separate from the enablement actor's
    /// caches: the enablement actor tracks "did attribute writes succeed",
    /// while this set tracks "did we also do the one-shot observer setup
    /// and run-loop pump that Chromium needs to populate its AX tree".
    private var pumpedPids: Set<Int32> = []
    /// Per-pid AXObserver registrations. We never read or remove these — the
    /// dictionary only exists so the observer and its run-loop source stay
    /// retained for the life of the process. Chromium detects the presence
    /// of at least one AXObserver with at least one `AXObserverAddNotification`
    /// subscription and engages its full accessibility pipeline; without this
    /// second signal VS Code and Chrome proper ignore the attribute-set hints
    /// and only expose their native window chrome (~20–30 elements).
    /// Process exit cleans everything up — no explicit teardown.
    private var accessibilityObservers: [Int32: AXObserver] = [:]

    private struct SessionState {
        let turnId: Int
        let elements: [Int: AXUIElement]
    }

    /// Walk `pid`'s AX tree, assign element indices to every actionable node,
    /// render to Markdown, cache the index → element map keyed on
    /// `(pid, windowId)`, and return a snapshot.
    ///
    /// The walk is pid-scoped (`AXUIElementCreateApplication(pid)`) but
    /// filtered to only the `AXWindow` whose `CGWindowID` matches
    /// `windowId` (plus non-window children like the menu bar). This
    /// ensures callers get the tree for the window they asked about,
    /// not whichever window happens to be focused.
    public func snapshot(pid: Int32, windowId: UInt32) async throws -> AppStateSnapshot {
        guard AXIsProcessTrusted() else {
            throw AppStateError.notAuthorized
        }
        guard let running = NSRunningApplication(processIdentifier: pid) else {
            throw AppStateError.appNotFound(pid)
        }

        let root = AXUIElementCreateApplication(pid)

        // Cue Chromium/Electron apps to turn on their web accessibility tree.
        // Non-Chromium apps ignore these attribute writes — safe no-op.
        try await activateAccessibilityIfNeeded(pid: pid, root: root)

        nextTurnId += 1
        let turnId = nextTurnId

        var elements: [Int: AXUIElement] = [:]
        var nextIndex = 0
        var markdown = ""

        renderTree(
            root,
            depth: 0,
            targetWindowId: windowId,
            elements: &elements,
            nextIndex: &nextIndex,
            output: &markdown
        )

        sessions[SessionKey(pid: pid, windowId: windowId)] =
            SessionState(turnId: turnId, elements: elements)

        return AppStateSnapshot(
            pid: pid,
            bundleId: running.bundleIdentifier,
            name: running.localizedName,
            treeMarkdown: markdown,
            elementCount: elements.count,
            turnId: turnId
        )
    }

    /// Pid metadata only — no AX tree walk, no cache update. Used by
    /// `get_window_state` in `capture_mode: vision` so vision-only
    /// workflows pay zero AX cost per turn. Callers that later want
    /// element-indexed actions must first call
    /// `snapshot(pid:windowId:)` to populate the per-(pid, window_id)
    /// element map. Deliberately does not require Accessibility
    /// permission — vision mode is useful on hosts that have only
    /// Screen Recording granted.
    public func metadataOnly(pid: Int32) throws -> AppStateSnapshot {
        guard let running = NSRunningApplication(processIdentifier: pid) else {
            throw AppStateError.appNotFound(pid)
        }
        return AppStateSnapshot(
            pid: pid,
            bundleId: running.bundleIdentifier,
            name: running.localizedName,
            treeMarkdown: "",
            elementCount: 0,
            turnId: 0
        )
    }

    /// Resolve a previously assigned element_index back to its AXUIElement,
    /// scoped to the `(pid, windowId)` that produced the snapshot. Callers
    /// must pass the same `windowId` they passed to `snapshot(pid:windowId:)`
    /// — indices from one window's snapshot do not resolve against another.
    public func lookup(
        pid: Int32, windowId: UInt32, elementIndex: Int
    ) throws -> AXUIElement {
        let key = SessionKey(pid: pid, windowId: windowId)
        guard let session = sessions[key] else {
            throw AppStateError.noCachedState(pid: pid, windowId: windowId)
        }
        guard let element = session.elements[elementIndex] else {
            throw AppStateError.invalidElementIndex(
                pid: pid, windowId: windowId, index: elementIndex)
        }
        return element
    }

    // MARK: - Chromium/Electron accessibility activation

    /// Chromium-family apps (Slack, Discord, VS Code, Chrome, Edge, Notion,
    /// every other Electron/Blink shell) ship with their internal accessibility
    /// mode disabled as a power optimization. There are two independent signals
    /// that Chromium watches for to decide "an AX client is actually here":
    ///
    /// 1. A boolean AX attribute set on the application's root element:
    ///    - `AXManualAccessibility` — the modern Chromium-specific hint
    ///    - `AXEnhancedUserInterface` — the legacy AppleScript-era equivalent
    ///    (factored out into ``AXEnablementAssertion``).
    /// 2. At least one `AXObserver` registered against the pid with at least
    ///    one active `AXObserverAddNotification` subscription. This is the
    ///    signal VS Code and Chrome proper actually listen to — they largely
    ///    ignore the attribute hints on their own. VS Code will go so far as
    ///    to pop a "Screen reader usage detected" prompt in response.
    ///
    /// Slack reacts to (1); VS Code and Chrome need (2) too. We run the
    /// attribute-asserter every snapshot (cheap no-op on non-Chromium apps)
    /// and, on the first success per pid, register the observer and pump the
    /// run loop so the tree can populate before we walk it. Subsequent
    /// snapshots of the same pid skip both.
    ///
    /// The observer is stored in ``accessibilityObservers`` purely to keep it
    /// retained — we never fire a callback deliberately and never unregister.
    private func activateAccessibilityIfNeeded(
        pid: Int32,
        root: AXUIElement
    ) async throws {
        // Already did the one-shot pump + observer for this pid.
        if pumpedPids.contains(pid) { return }
        // Attribute-assertion — returns false for apps that reject both.
        let accepted = await enablement.assert(pid: pid, root: root)
        guard accepted else { return }

        pumpedPids.insert(pid)
        registerAccessibilityObserver(pid: pid)
        // First activation — let Chromium build its tree before we walk.
        // 500 ms is the difference between a first snapshot that shows
        // just the native window frame (~25 elements) and the full web
        // AX tree (500+ elements on a typical VS Code workspace).
        // The wait is a manual run-loop pump: Chromium's "AX client is
        // present" detection needs our observer registration to reach
        // its side of the AX XPC channel, and that delivery requires a
        // spinning CFRunLoop. The MCP server's main loop doesn't spin
        // on its own, so we drive the current thread's loop ourselves.
        pumpRunLoopForActivation(duration: 0.5)
    }

    /// Pump the current thread's CFRunLoop for roughly `duration` seconds.
    /// Sync wrapper — ``CFRunLoopRunInMode`` isn't available directly from
    /// async contexts under Swift 6 strict concurrency, so we keep the
    /// pump isolated in this non-async helper.
    private nonisolated func pumpRunLoopForActivation(duration: CFTimeInterval) {
        let endTime = CFAbsoluteTimeGetCurrent() + duration
        while CFAbsoluteTimeGetCurrent() < endTime {
            let remaining = endTime - CFAbsoluteTimeGetCurrent()
            _ = CFRunLoopRunInMode(
                CFRunLoopMode.defaultMode, remaining, false
            )
        }
    }

    /// Create an `AXObserver` for `pid`, attach its run-loop source to the
    /// current thread's run loop (which the caller then pumps via
    /// ``CFRunLoopRunInMode``), and subscribe to a small handful of cheap
    /// notifications so Chromium sees "an AX client is listening." Stores
    /// the observer in ``accessibilityObservers`` to keep it retained for
    /// the process lifetime.
    ///
    /// Every individual step (observer create, each `AddNotification`) may
    /// fail for any given app — some apps refuse specific notifications, some
    /// pids are sandboxed, etc. We treat each failure as a no-op and move on;
    /// the goal is signalling presence, not exhaustive coverage.
    private func registerAccessibilityObserver(pid: Int32) {
        var observer: AXObserver?
        let createResult = AXObserverCreateWithInfoCallback(
            pid, cuaDriverObserverNoopCallback, &observer
        )
        guard createResult == .success, let observer else { return }

        if let source = AXObserverGetRunLoopSource(observer) as CFRunLoopSource? {
            // Attach to the MAIN run loop. `AppKitBootstrap` now runs
            // `NSApplication.shared.run()` on the main thread (for the
            // agent cursor overlay), which means the main run loop is
            // continuously spinning for the life of the daemon / MCP
            // server. That's critical for Chromium: its AX pipeline
            // stays on only while at least one observer's run-loop
            // source is actively being serviced. Attaching to the
            // current (ephemeral async-executor) thread's run loop
            // worked for one-shot snapshots but collapsed Chrome's
            // tree the moment the daemon went idle — between
            // snapshots the observer was effectively dead.
            CFRunLoopAddSource(
                CFRunLoopGetMain(), source, CFRunLoopMode.defaultMode
            )
        }

        // Subscribe to a broad set of notifications. We don't care about
        // the events — only that enough subscriptions exist to convince
        // Chromium-family apps that an AX client is actively listening
        // and keep their renderer-side accessibility pipeline engaged
        // regardless of focus. Chrome in particular checks not just
        // "is there an observer" but "does it subscribe to the
        // notifications a screen reader would care about" — a single
        // focus-changed subscription isn't enough to keep the web tree
        // alive when the app is occluded. Per-app failures are expected
        // (some apps reject certain notifications entirely) and silently
        // ignored.
        let root = AXUIElementCreateApplication(pid)
        for notification in [
            kAXFocusedUIElementChangedNotification,
            kAXFocusedWindowChangedNotification,
            kAXApplicationActivatedNotification,
            kAXApplicationDeactivatedNotification,
            kAXApplicationHiddenNotification,
            kAXApplicationShownNotification,
            kAXWindowCreatedNotification,
            kAXWindowMovedNotification,
            kAXWindowResizedNotification,
            kAXValueChangedNotification,
            kAXTitleChangedNotification,
            kAXSelectedChildrenChangedNotification,
            kAXLayoutChangedNotification,
        ] {
            _ = addObserverNotificationPreferRemote(
                observer: observer,
                element: root,
                notification: notification as CFString
            )
        }
        accessibilityObservers[pid] = observer
    }

    /// Call the Apple-private `AXObserverAddNotificationAndCheckRemote`
    /// if available, otherwise fall back to the public
    /// `AXObserverAddNotification`. The private variant is the likely
    /// reason Chromium keeps its renderer-side AX pipeline alive for
    /// our observers even when the target is backgrounded — the
    /// `CheckRemote` path ensures the subscription is ACK'd by the
    /// target's AX server, not just locally registered.
    ///
    /// `dlsym` looks up the symbol in the already-loaded
    /// HIServices module (part of ApplicationServices.framework,
    /// always linked by any process that uses AX). Cached after the
    /// first lookup.
    private func addObserverNotificationPreferRemote(
        observer: AXObserver,
        element: AXUIElement,
        notification: CFString
    ) -> AXError {
        if let fn = Self.axObserverAddNotificationAndCheckRemote {
            return fn(observer, element, notification, nil)
        }
        return AXObserverAddNotification(observer, element, notification, nil)
    }

    /// One-shot `dlsym` for `AXObserverAddNotificationAndCheckRemote`.
    /// Resolved against the current process's already-loaded symbols
    /// (`RTLD_DEFAULT`). Returns `nil` on older macOS where the
    /// symbol doesn't exist, in which case the caller falls back to
    /// the public API.
    private static let axObserverAddNotificationAndCheckRemote:
        (@convention(c) (AXObserver, AXUIElement, CFString, UnsafeMutableRawPointer?) -> AXError)? = {
            guard
                let sym = dlsym(
                    UnsafeMutableRawPointer(bitPattern: -2),  // RTLD_DEFAULT
                    "AXObserverAddNotificationAndCheckRemote")
            else {
                return nil
            }
            return unsafeBitCast(
                sym,
                to: (@convention(c) (AXObserver, AXUIElement, CFString, UnsafeMutableRawPointer?) -> AXError).self
            )
        }()

    // MARK: - Walking

    private func renderTree(
        _ element: AXUIElement,
        depth: Int,
        targetWindowId: UInt32?,
        elements: inout [Int: AXUIElement],
        nextIndex: inout Int,
        output: inout String
    ) {
        guard depth <= AppStateEngine.maxDepth else { return }

        let role = attributeString(element, "AXRole") ?? "?"
        let title = attributeString(element, "AXTitle")
        let value = attributeString(element, "AXValue")
        let description = attributeString(element, "AXDescription")
        let identifier = attributeString(element, "AXIdentifier")
        let help = attributeString(element, "AXHelp")
        let enabled = attributeBool(element, "AXEnabled")
        let actions = actionNames(of: element)

        let indent = String(repeating: "  ", count: depth)
        let interactive = !actions.isEmpty

        var line = indent + "- "
        if interactive {
            let idx = nextIndex
            elements[idx] = element
            nextIndex += 1
            line += "[\(idx)] "
        }
        line += role
        if let t = title, !t.isEmpty { line += " \"\(t)\"" }
        if let v = value, !v.isEmpty, v.count < 120 { line += " = \"\(v)\"" }
        if let d = description, !d.isEmpty, d.count < 120 { line += " (\(d))" }
        if let h = help, !h.isEmpty, h.count < 160 { line += " help=\"\(h)\"" }
        if let id = identifier, !id.isEmpty { line += " id=\(id)" }
        // DISABLED is only meaningful on interactive elements — top-level
        // containers often report AXEnabled=false just because they have no
        // meaningful enabled state.
        if interactive, enabled == false { line += " DISABLED" }
        // AXPress is the default click; list additional actions so the LLM
        // knows what else this element supports (e.g. AXShowMenu for right-
        // click targets, AXIncrement for steppers, AXCopy for text areas).
        let secondary = actions.filter { $0 != "AXPress" }
        if !secondary.isEmpty {
            line += " actions=[\(secondary.joined(separator: ", "))]"
        }
        output += line + "\n"

        // Skip the contents of CLOSED AXMenu subtrees — every app's menu
        // bar lists every submenu and Recent Item macOS has ever seen,
        // which inflates the tree 10-100x. The menubar items themselves
        // stay visible and clickable.
        //
        // OPEN menus (those the caller has just AXPicked on a parent
        // menu bar item) DO get walked so their AXMenuItem children pick
        // up element_index values — that's what makes the canonical
        // "click Go → click Downloads" menu navigation pattern work.
        //
        // We detect "open" via `AXVisibleChildren`: on a closed AXMenu
        // this attribute is either missing or returns an empty array;
        // on an expanded menu it returns the visible AXMenuItem list.
        // This is the real public signal AppKit exposes for menu
        // expansion — not a heuristic.
        if role == "AXMenu" && !isMenuOpen(element) { return }

        var kids = (depth == 0 && role == "AXApplication")
            ? topLevelChildren(of: element)
            : children(of: element)

        // At the application root, filter to only the AXWindow matching
        // the caller's target windowId (plus non-window children like
        // the menu bar). Without this filter, the walk returns every
        // window's tree merged together, and the caller can't inspect
        // a specific window.
        if depth == 0 && role == "AXApplication", let targetWid = targetWindowId {
            kids = kids.filter { child in
                let childRole = attributeString(child, "AXRole")
                guard childRole == "AXWindow" else {
                    // Keep non-window children (menu bar, etc.)
                    return true
                }
                // Bridge AXUIElement → CGWindowID via _AXUIElementGetWindow.
                var cgWindowId: CGWindowID = 0
                let err = _AXUIElementGetWindow(child, &cgWindowId)
                guard err == .success else {
                    // Can't resolve — keep it to avoid silently dropping
                    // windows whose CGWindowID we can't read.
                    return true
                }
                return cgWindowId == targetWid
            }
        }

        for child in kids {
            renderTree(
                child,
                depth: depth + 1,
                targetWindowId: targetWindowId,
                elements: &elements,
                nextIndex: &nextIndex,
                output: &output
            )
        }
    }

    /// Union `AXChildren` with `AXWindows` on the app root.
    ///
    /// `AXChildren` omits windows when the app isn't frontmost — AppKit
    /// only exposes the menu bar through that attribute until the app
    /// activates. `AXWindows` exposes them regardless of activation but
    /// omits the menu bar. We need both: the menu bar is how automation
    /// drives app-level commands, and the windows carry every other
    /// actionable element.
    private func topLevelChildren(of appRoot: AXUIElement) -> [AXUIElement] {
        let fromChildren = children(of: appRoot)
        let fromWindows = windows(of: appRoot)
        var out = fromChildren
        for window in fromWindows where !out.contains(where: { CFEqual($0, window) }) {
            out.append(window)
        }
        return out
    }

    private func windows(of appRoot: AXUIElement) -> [AXUIElement] {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            appRoot, kAXWindowsAttribute as CFString, &value
        )
        guard result == .success, let array = value else { return [] }
        guard CFGetTypeID(array) == CFArrayGetTypeID() else { return [] }
        let cfArray = unsafeDowncast(array, to: CFArray.self)
        let count = CFArrayGetCount(cfArray)
        var out: [AXUIElement] = []
        out.reserveCapacity(count)
        for i in 0..<count {
            if let raw = CFArrayGetValueAtIndex(cfArray, i) {
                let element = Unmanaged<AXUIElement>.fromOpaque(raw).takeUnretainedValue()
                out.append(element)
            }
        }
        return out
    }

    /// Is this AXMenu currently expanded on-screen?
    ///
    /// `AXVisibleChildren` on an AXMenu is the public signal AppKit
    /// exposes for "this menu is actually open right now." A closed
    /// AXMenu — the inert child of every AXMenuBarItem that hasn't been
    /// picked — either returns `.attributeUnsupported` for the attribute
    /// or returns an empty array. An expanded menu returns the list of
    /// AXMenuItem children the user can actually see. This is exactly
    /// the "open vs. closed" bit we need.
    ///
    /// Returns `false` when the attribute is missing, unsupported, or
    /// empty — that matches the "stay-skipped" default for the menu
    /// subtrees we deliberately don't want to walk.
    private func isMenuOpen(_ menu: AXUIElement) -> Bool {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            menu, "AXVisibleChildren" as CFString, &value
        )
        guard result == .success, let array = value else { return false }
        guard CFGetTypeID(array) == CFArrayGetTypeID() else { return false }
        let cfArray = unsafeDowncast(array, to: CFArray.self)
        return CFArrayGetCount(cfArray) > 0
    }

    private func attributeString(_ element: AXUIElement, _ attribute: String) -> String? {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, attribute as CFString, &value)
        guard result == .success else { return nil }
        return value as? String
    }

    private func attributeBool(_ element: AXUIElement, _ attribute: String) -> Bool? {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, attribute as CFString, &value)
        guard result == .success, let v = value else { return nil }
        if CFGetTypeID(v) == CFBooleanGetTypeID() {
            return CFBooleanGetValue((v as! CFBoolean))
        }
        return nil
    }

    private func actionNames(of element: AXUIElement) -> [String] {
        var names: CFArray?
        let result = AXUIElementCopyActionNames(element, &names)
        guard result == .success, let names = names as? [String] else { return [] }
        return names.map { cleanActionName($0) }
    }

    /// Standard AX actions come through as simple strings like `AXPress`.
    /// Custom actions registered via `NSAccessibilityCustomAction` sometimes
    /// serialize as the type's default description dump:
    ///
    ///     Name:Copy
    ///     Target:0x0
    ///     Selector:(null)
    ///
    /// Extract just the `Name:` value so the rendered tree stays compact.
    /// Simple `AX...` names pass through unchanged.
    private func cleanActionName(_ raw: String) -> String {
        if raw.hasPrefix("AX") { return raw }
        for line in raw.split(whereSeparator: \.isNewline) {
            if let range = line.range(of: "Name:") {
                let name = line[range.upperBound...].trimmingCharacters(in: .whitespaces)
                if !name.isEmpty { return name }
            }
        }
        return raw
    }

    private func children(of element: AXUIElement) -> [AXUIElement] {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, "AXChildren" as CFString, &value)
        guard result == .success, let array = value else { return [] }
        guard CFGetTypeID(array) == CFArrayGetTypeID() else { return [] }
        let cfArray = unsafeDowncast(array, to: CFArray.self)
        let count = CFArrayGetCount(cfArray)
        var out: [AXUIElement] = []
        out.reserveCapacity(count)
        for i in 0..<count {
            if let raw = CFArrayGetValueAtIndex(cfArray, i) {
                let element = Unmanaged<AXUIElement>.fromOpaque(raw).takeUnretainedValue()
                out.append(element)
            }
        }
        return out
    }

}
