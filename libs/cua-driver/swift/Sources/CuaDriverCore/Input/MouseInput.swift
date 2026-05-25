import AppKit
import CoreGraphics
import Darwin
import Foundation

/// Mouse synthesis via `NSEvent.mouseEvent(...).cgEvent`, posted
/// through SkyLight's per-pid event-post path AND the system HID
/// event tap (the two are complementary — SkyLight reaches a
/// backgrounded target's mach port, HID tap delivers to the
/// frontmost app the normal way).
///
/// Architectural note: the driver is AX-only for mouse wherever an
/// AX path exists — left-click and show-menu on an AX-addressable
/// element dispatch via `AXUIElementPerformAction` against a cached
/// element handle. The coordinate-addressed paths here are the
/// deliberate carve-out: some surfaces (web content, native canvases,
/// games) don't expose an AX-responsive element, so the only
/// remaining option is synthesized `mouseDown` / `mouseUp` pairs at a
/// pixel. For AX-addressable elements the element_index path is
/// strictly better — it's pure RPC and works on backgrounded /
/// hidden windows.
///
/// Why NSEvent-bridge (was: raw CGEvent): AppKit-style construction
/// via `+[NSEvent mouseEventWithType:location:...
/// windowNumber:context:eventNumber:clickCount:pressure:]` bridged to
/// CGEvent via `-[NSEvent CGEvent]` produces events that Chromium's
/// renderer accepts as trusted — raw-CGEvent-built events are
/// silently filtered at the renderer IPC boundary. Switching to the
/// NSEvent-bridged path fixed Chromium web-content hit-tests on
/// backgrounded targets.
///
/// Coordinate convention: `NSEvent.mouseEvent(location:)` expects
/// AppKit / Cocoa coordinates — screen-bottom-left origin, y-up.
/// Our public API takes the `CGPoint` / screen-point convention used
/// everywhere else in the driver (screen-top-left origin, y-down).
/// We flip y against the main screen's height before handing to
/// `NSEvent.mouseEvent`; the `.cgEvent` bridge then re-flips back to
/// the Quartz top-left convention when emitting the `CGEvent` — so
/// the posted event ends up at the original screen-point the caller
/// asked for. Verified by comparing `CGEvent.location` before and
/// after the bridge against a raw-CGEvent-built event at the same
/// point.
///
/// WindowNumber: we pass 0 ("not associated with any window") rather
/// than the result of a `CGWindowListCopyWindowInfo` hit-test.
/// Empirically, passing a non-zero `windowNumber` causes the
/// resulting event to be rejected by the HID-tap dispatcher even
/// when the window genuinely owns the click point. The event still
/// carries its screen-space location, so WindowServer's own
/// hit-test picks up the right target on post.
///
/// Post paths, in order of preference:
///
///   1. `SkyLightEventPost.postToPid` (auth-signed). Reaches a
///      backgrounded target's queue directly, no cursor movement.
///      Works cleanly for keyboard; for mouse it delivers to
///      Chromium-family targets but is unreliable against ordinary
///      AppKit targets.
///   2. `CGEvent.postToPid(pid)` (public API). Delivers to the pid's
///      mach port directly. Does NOT move the real cursor, does NOT
///      activate the target. A separate code path from SkyLight's,
///      often lands where SkyLight silently drops for AppKit mouse.
///
/// We fire (1) and (2) in sequence and accept the cost of each event
/// possibly arriving at the target twice — neither path moves the
/// user's real cursor, so the only observable cost is extra mach
/// traffic. We deliberately do NOT fall back to `.cghidEventTap`:
/// the HID tap routes through WindowServer with the real cursor,
/// which warps the user's pointer to the click point. That's a
/// focus-steal-equivalent behavior we reject — pixel click's whole
/// value is it's invisible to the user.
///
/// `count` and `modifiers` support:
/// - `count: 2` synthesizes a double-click (two down/up pairs ~80ms
///   apart, with each pair's `clickCount` set to the click index so
///   AppKit / Chromium coalesce them into a double-click).
/// - `modifiers` maps the same `cmd/shift/option/ctrl/fn` vocabulary
///   as `KeyboardInput` onto the event's flags, enabling ctrl-click
///   and cmd-click from the pixel path.
///
/// Known limitation: pixel-addressed right-click into Chromium *web
/// content* is observed to land as a left-click — the event reaches
/// the renderer, but Chromium's web-content hit-test appears to coerce
/// the button back to primary for pointer events that weren't
/// originated by the real HID. Observed in every non-HID-tap recipe
/// we've tried. For context menus on AX-addressable elements (browser
/// chrome, links, buttons that advertise `AXShowMenu`), the
/// `element_index` path remains the reliable option.
public enum MouseInput {
    public enum Button: String, Sendable {
        case left
        case right
        case middle
    }

    /// Synthesize a full `mouseDown` / `mouseUp` pair (or multiple
    /// pairs for `count > 1` double / triple-click) at `point` in
    /// screen points (top-left origin) and deliver them to `pid`.
    /// `modifiers` accepts the same names as `KeyboardInput`
    /// (`cmd` / `command`, `shift`, `option` / `alt`, `ctrl` /
    /// `control`, `fn`); unknown names are ignored.
    ///
    /// When `useFrontmostHIDPath` is true (the default) and `pid` is
    /// frontmost, events are posted through the public HID tap, which
    /// can move the global cursor and is required for some viewport
    /// apps. When false, the function skips that path and uses only
    /// pid-routed delivery, preserving the system cursor for callers
    /// that rely on background-style dispatch.
    public static func click(
        at point: CGPoint,
        toPid pid: pid_t,
        button: Button,
        count: Int = 1,
        modifiers: [String] = [],
        useFrontmostHIDPath: Bool = true
    ) throws {
        // When the target is frontmost, route via the public HID tap
        // (`CGEventPost(tap: .cghidEventTap)`) with a preceding
        // `mouseMoved`. This is the only path that reaches OpenGL /
        // GHOST-style viewports (Blender, Unity, some games) —
        // per-pid delivery paths (`SLEventPostToPid` /
        // `CGEventPostToPid`) are filtered by those viewports at the
        // event-source level. Empirically verified 2026-04-19 with
        // Blender: pid-routed clicks produce zero visible change,
        // cghidEventTap + leading mouseMoved selects the cube. The
        // real cursor visibly moves — unavoidable, but acceptable
        // when the user is already looking at the target. Pid-routed
        // paths remain the right choice for backgrounded targets
        // (Chrome/Slack/etc); they just don't work on viewports.
        let targetIsFrontmost =
            NSRunningApplication(processIdentifier: pid)?.isActive ?? false
        if useFrontmostHIDPath && targetIsFrontmost {
            try clickFrontmostViaHIDTap(
                at: point, button: button, count: count, modifiers: modifiers)
            return
        }

        // Default path for un-modified left single / double clicks is
        // the auth-signed recipe (see `clickViaAuthSignedPost`). The
        // primer prologue opens Chromium's user-activation gate, then
        // `count` target pairs fire with `clickState` stepped 1…count
        // so the renderer coalesces pair N+1 into a double-click.
        // Modifier-held and triple+ clicks stay on the NSEvent-bridge
        // double-post path below.
        if button == .left, count == 1 || count == 2, modifiers.isEmpty {
            try clickViaAuthSignedPost(
                at: point, toPid: pid, count: count, modifiers: modifiers)
            return
        }

        let clamped = max(1, min(3, count))
        let (downType, upType) = nsEventTypes(for: button)
        let modifierFlags = modifierMask(for: modifiers)
        let cocoaPoint = cocoaLocation(fromScreenPoint: point)
        let winNum = Int(
            WindowEnumerator.frontmostWindow(forPid: pid)
                .map { Int64(CGWindowID($0.id)) } ?? 0)

        for clickIndex in 1...clamped {
            let down = try buildCGEvent(
                type: downType,
                location: cocoaPoint,
                modifierFlags: modifierFlags,
                clickCount: clickIndex,
                button: button,
                windowNumber: winNum
            )
            let up = try buildCGEvent(
                type: upType,
                location: cocoaPoint,
                modifierFlags: modifierFlags,
                clickCount: clickIndex,
                button: button,
                windowNumber: winNum
            )

            // Belt + suspenders — NSEvent sets `kCGMouseEventClickState`
            // when built via `mouseEvent(...)`, but re-stamp on the
            // bridged CGEvent so an SDK behaviour drift still ends up
            // with the right click count on the CGEvent side. Field id
            // 1 matches `CGEventField.mouseEventClickState`; reference
            // the enum directly so a future SDK rename surfaces as a
            // compiler error.
            down.setIntegerValueField(
                .mouseEventClickState, value: Int64(clickIndex))
            up.setIntegerValueField(
                .mouseEventClickState, value: Int64(clickIndex))

            postBoth(down, toPid: pid)
            // Small intra-pair gap so AppKit / Chromium's hit-test
            // treats the down+up as a discrete click rather than
            // collapsing them. 30 ms mirrors the keyboard path's
            // inter-key spacing and is well under the double-click
            // threshold.
            usleep(30_000)
            postBoth(up, toPid: pid)

            if clickIndex < clamped {
                // ~80ms between pairs — under the system
                // double-click threshold but clear of coalescing.
                usleep(80_000)
            }
        }
    }

    /// Left-click recipe — yabai focus-without-raise + off-screen
    /// primer + single target click. Replaces an older
    /// `CGEventSetFlags(0x100000)` recipe which visibly raised the
    /// target window. The new recipe keeps Chrome (and any AppKit
    /// target) at its current z-rank and avoids the "switch to a
    /// Space with open windows for the application" follow behavior
    /// on multi-Space setups.
    ///
    /// Sequence (see `project_noraise_click_recipe.md` memory entry
    /// + `FocusWithoutRaise.swift` for the activation half):
    ///  1. `FocusWithoutRaise.activateWithoutRaise(targetPid, targetWid)` —
    ///     posts yabai-style defocus/focus event records so the target
    ///     becomes AppKit-active without a WindowServer restack.
    ///  2. Sleep 50 ms (let the focus event settle).
    ///  3. Stamped `mouseMoved` at target coords.
    ///  4. Stamped left-down/up pair at off-screen `(-1, 1441)` — this
    ///     satisfies Chromium's user-activation gate (no DOM hit at
    ///     that coord, so no visible side-effect) without being a
    ///     dblclick.
    ///  5. Stamped left-down/up pair at target coords — the real click.
    ///
    /// All click events carry `clickState=1` (single click), SkyLight
    /// field 40 stamped with the target pid, and are posted through
    /// `SLEventPostToPid` without an auth message (same as the older
    /// recipe — the mouse path needs the IOHIDPostEvent route, not the
    /// direct-mach route that auth messaging forks onto).
    ///
    /// Explicit field writes (on top of `NSEvent.cgEvent`'s
    /// auto-fills):
    ///   - f0 (`kCGMouseEventNumber`): gesture-phase marker. The
    ///     empirically-verified pattern is a small finite alphabet
    ///     (move=2, primerDown=1, primerUp=2, targetDown=3,
    ///     targetUp=3) rather than a monotonic counter; we stamp the
    ///     literal values via SkyLight's raw-field SPI because the
    ///     NSEvent bridge leaves it at 0.
    ///   - f3 (`kCGMouseEventButtonNumber`) = 0 (left).
    ///   - f7 (`kCGMouseEventSubtype`) = 3 (NSEventSubtypeTouch).
    ///   - f51 / f91 / f92: target window CGWindowID. f51 has no
    ///     public enum case; stamped via the Skylight raw-field SPI.
    ///   - f58 ("click-group ID"): constant across all 5 events so
    ///     WindowServer / Chromium treat them as one gesture. The
    ///     NSEvent bridge stamps different per-event timestamps here,
    ///     so we overwrite.
    ///
    /// We deliberately do NOT re-stamp NSEvent's other auto-filled
    /// fields (f1, f2, f41, f43, f44, f50, f55, f59, f102, f108) —
    /// double-writing can trip WindowServer's sanity check.
    ///
    /// Window-local point via the private `CGEventSetWindowLocation`
    /// SPI (resolved through `dlsym`). Per-event so the primer carries
    /// its off-screen sentinel and the real click carries the real
    /// window-local point.
    ///
    /// Event sequence with empirically-tuned timing: mouseMoved (→target),
    /// 25 ms, primer down (@ off-screen sentinel), 1 ms, primer up,
    /// 108 ms, target down, 1 ms, target up.
    ///
    /// Known limitation — Chromium `<video>` play/pause: Chrome sees
    /// the CMD flag we stamp (see below) as a Cmd-click, and HTML5
    /// video's click-to-play handler rejects Cmd-clicks. Callers that
    /// need video controls should drive them via `press_key` (`k` or
    /// `space`) instead of a pixel click. Ordinary DOM elements
    /// (Subscribe, links, menu items) still trigger normally under a
    /// Cmd-click.
    private static func clickViaAuthSignedPost(
        at point: CGPoint,
        toPid pid: pid_t,
        count: Int = 1,
        modifiers: [String]
    ) throws {
        // Caller contract: count is 1 or 2 (guarded at the click()
        // entry). Anything else falls through to the NSEvent-bridge
        // path, which doesn't share the primer prologue.
        let clickPairs = max(1, min(2, count))
        // Resolve target window — CGWindowID for field stamps + window-
        // local point + PSN lookup for the focus-without-raise step.
        let targetWindow = WindowEnumerator.frontmostWindow(forPid: pid)
        let windowID = Int64(targetWindow.map { CGWindowID($0.id) } ?? 0)
        let winNum = Int(windowID)

        let windowLocalTarget: CGPoint = {
            guard let bounds = targetWindow?.bounds else { return point }
            return CGPoint(x: point.x - bounds.x, y: point.y - bounds.y)
        }()

        // Step 1: activate without raise. Posts yabai-style defocus +
        // focus event records so the target is AppKit-active (events
        // route via SLEventPostToPid, user-activation gate opens) but
        // the window stays at its current z-rank.
        if windowID != 0 {
            _ = FocusWithoutRaise.activateWithoutRaise(
                targetPid: pid, targetWid: CGWindowID(windowID))
            // Let the focus record settle before the click stream.
            usleep(50_000)
        }

        // Builds an NSEvent-bridged CGEvent with the windowNumber that
        // makes AppKit route the event to the right window on the
        // receiving side. Fields are all overwritten by stamp() below.
        func makeEvent(_ type: NSEvent.EventType, clickCount: Int) throws -> CGEvent {
            guard
                let ns = NSEvent.mouseEvent(
                    with: type,
                    location: .zero,
                    modifierFlags: [],
                    timestamp: 0,
                    windowNumber: winNum,
                    context: nil,
                    eventNumber: 0,
                    clickCount: clickCount,
                    pressure: 1.0
                )
            else { throw MouseInputError.eventCreationFailed("\(type.rawValue)") }
            guard let cg = ns.cgEvent else {
                throw MouseInputError.eventCreationFailed(
                    "\(type.rawValue) cgEvent bridge")
            }
            return cg
        }

        // Stamp each event with the common fields:
        //  - screen location
        //  - mouseEventButtonNumber = 0 (left)
        //  - mouseEventSubtype = 3 (NSEventSubtypeMouseEvent)
        //  - mouseEventClickState = 1 (single click — Chrome's gate
        //    only treats clickState 1 as a real single click; 0 or 2+
        //    have different semantics that don't land as true clicks)
        //  - mouseEventWindowUnderMousePointer(+ThatCanHandleThisEvent)
        //  - CGEventSetWindowLocation (window-local coord)
        //  - SkyLight field 40 = target pid (Chromium's synthetic-event
        //    filter latches onto this — missing it = click dropped)
        func stamp(
            _ event: CGEvent,
            screenPt: CGPoint,
            windowLocalPt: CGPoint,
            clickState: Int64 = 1
        ) {
            event.location = screenPt
            event.setIntegerValueField(.mouseEventButtonNumber, value: 0)
            event.setIntegerValueField(.mouseEventSubtype, value: 3)
            // clickState = 1 for the first target pair; 2 for the second
            // pair when count == 2 so Chromium's renderer coalesces the
            // two down/up pairs into a `dblclick` rather than two
            // independent clicks. The primer events always ship as 1 —
            // they're there to open the user-activation gate, not to
            // participate in the click sequence the target sees.
            event.setIntegerValueField(.mouseEventClickState, value: clickState)
            if windowID != 0 {
                event.setIntegerValueField(
                    .mouseEventWindowUnderMousePointer, value: windowID)
                event.setIntegerValueField(
                    .mouseEventWindowUnderMousePointerThatCanHandleThisEvent,
                    value: windowID)
            }
            _ = SkyLightEventPost.setWindowLocation(event, windowLocalPt)
            _ = SkyLightEventPost.setIntegerField(
                event, field: 40, value: Int64(pid))
        }

        // Step 3: mouseMoved at target (cursor-state primer).
        let move = try makeEvent(.mouseMoved, clickCount: 0)
        stamp(move, screenPt: point, windowLocalPt: windowLocalTarget)

        // Step 4: off-screen primer click — satisfies Chromium's
        // user-activation gate without hitting any DOM element. Any
        // point that lies outside every window works; `(-1, -1)` is
        // the simplest choice: both axes negative means no window
        // (including top-of-screen menubar strips) can claim the
        // coord. Chrome discards the click but the gesture-timestamp
        // counter still ticks forward.
        let offScreenPrimer = CGPoint(x: -1, y: -1)
        let primerDown = try makeEvent(.leftMouseDown, clickCount: 1)
        let primerUp   = try makeEvent(.leftMouseUp,   clickCount: 1)
        stamp(primerDown, screenPt: offScreenPrimer, windowLocalPt: offScreenPrimer)
        stamp(primerUp,   screenPt: offScreenPrimer, windowLocalPt: offScreenPrimer)

        // Step 5: target click pair(s). For count == 2 we emit two
        // consecutive down/up pairs with clickState stepped 1 → 2. The
        // verified timing for Chromium dblclick coalescing is
        // ~80 ms between pairs — under the system double-click
        // threshold but clear of coalescing the second pair back into
        // the first. `clickCount` on NSEvent-bridge and clickState on
        // the stamped CGEvent both get the pair index.
        var targetPairs: [(down: CGEvent, up: CGEvent)] = []
        for pairIndex in 1...clickPairs {
            let down = try makeEvent(.leftMouseDown, clickCount: pairIndex)
            let up   = try makeEvent(.leftMouseUp,   clickCount: pairIndex)
            let state = Int64(pairIndex)
            stamp(down, screenPt: point, windowLocalPt: windowLocalTarget,
                  clickState: state)
            stamp(up,   screenPt: point, windowLocalPt: windowLocalTarget,
                  clickState: state)
            targetPairs.append((down, up))
        }

        func post(_ event: CGEvent) {
            event.timestamp = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
            _ = SkyLightEventPost.postToPid(pid, event: event, attachAuthMessage: false)
        }

        post(move)
        usleep(15_000)  // One frame+ after mouseMoved for cursor state.
        post(primerDown)
        usleep(1_000)
        post(primerUp)
        usleep(100_000)  // ≥1 frame so Chromium sees primer + target as
                         // separate gestures, not a run-on.
        for (pairIndex, pair) in targetPairs.enumerated() {
            post(pair.down)
            usleep(1_000)
            post(pair.up)
            if pairIndex < targetPairs.count - 1 {
                usleep(80_000)  // Below system double-click threshold,
                                // clear of coalescing with pair N.
            }
        }
    }

    /// Convenience wrapper — synthesize a right-click at `point` and
    /// deliver it to `pid`. Kept as a thin pass-through so existing
    /// callers don't have to thread `Button.right` through.
    public static func rightClick(
        at point: CGPoint,
        toPid pid: pid_t,
        modifiers: [String] = []
    ) throws {
        try click(
            at: point,
            toPid: pid,
            button: .right,
            count: 1,
            modifiers: modifiers
        )
    }

    /// Synthesize a press-drag-release gesture from `start` to `end` in
    /// screen points. Emits one `mouseDown` at `start`, `steps`
    /// linearly-interpolated `mouseDragged` events along the path, and
    /// one `mouseUp` at `end`. `durationMs` is the wall-clock budget
    /// for the path between down and up; the time is split evenly
    /// across the drag steps.
    ///
    /// Frontmost target: posts via `.cghidEventTap` with a leading
    /// `mouseMoved` so the recipient sees a real HID-origin gesture
    /// (matches what AppKit drag sources, Finder selection, and
    /// canvas-backed viewports expect). The user's real cursor
    /// follows the drag path — unavoidable, since `cghidEventTap` is
    /// the system input stream.
    ///
    /// Backgrounded target: posts via `postBoth` (auth-signed
    /// SkyLight + public `CGEvent.postToPid`). Cursor-neutral. Some
    /// surfaces filter pid-routed mouseDragged events at the
    /// event-source level (same OpenGL/GHOST-style filter that
    /// affects pid-routed clicks); those targets need to be frontmost
    /// for drags to land.
    ///
    /// `modifiers` are held across every event in the gesture (down,
    /// every dragged step, up), enabling option-drag (duplicate),
    /// shift-drag (constrained axis), etc.
    public static func drag(
        from start: CGPoint,
        to end: CGPoint,
        toPid pid: pid_t,
        button: Button = .left,
        durationMs: Int = 500,
        steps: Int = 20,
        modifiers: [String] = []
    ) throws {
        let clampedSteps = max(1, min(200, steps))
        let clampedDuration = max(0, min(10_000, durationMs))
        // Split the wall-clock budget across the dragged-step gaps.
        // `clampedSteps` intermediate points produce `clampedSteps`
        // gaps between down → first-drag → … → last-drag → up.
        let perStepUs = useconds_t((clampedDuration * 1_000) / clampedSteps)

        let targetIsFrontmost =
            NSRunningApplication(processIdentifier: pid)?.isActive ?? false
        if targetIsFrontmost {
            try dragFrontmostViaHIDTap(
                from: start,
                to: end,
                button: button,
                steps: clampedSteps,
                perStepUs: perStepUs,
                modifiers: modifiers
            )
            return
        }

        let (downType, upType) = nsEventTypes(for: button)
        let draggedType = nsDraggedType(for: button)
        let modifierFlags = modifierMask(for: modifiers)
        let winNum = Int(
            WindowEnumerator.frontmostWindow(forPid: pid)
                .map { Int64(CGWindowID($0.id)) } ?? 0)

        let down = try buildCGEvent(
            type: downType,
            location: cocoaLocation(fromScreenPoint: start),
            modifierFlags: modifierFlags,
            clickCount: 1,
            button: button,
            windowNumber: winNum
        )
        down.setIntegerValueField(.mouseEventClickState, value: 1)
        postBoth(down, toPid: pid)

        for step in 1...clampedSteps {
            let progress = Double(step) / Double(clampedSteps)
            let point = CGPoint(
                x: start.x + (end.x - start.x) * progress,
                y: start.y + (end.y - start.y) * progress
            )
            let drag = try buildCGEvent(
                type: draggedType,
                location: cocoaLocation(fromScreenPoint: point),
                modifierFlags: modifierFlags,
                clickCount: 1,
                button: button,
                windowNumber: winNum
            )
            drag.setIntegerValueField(.mouseEventClickState, value: 1)
            usleep(perStepUs)
            postBoth(drag, toPid: pid)
        }

        let up = try buildCGEvent(
            type: upType,
            location: cocoaLocation(fromScreenPoint: end),
            modifierFlags: modifierFlags,
            clickCount: 1,
            button: button,
            windowNumber: winNum
        )
        up.setIntegerValueField(.mouseEventClickState, value: 1)
        usleep(perStepUs)
        postBoth(up, toPid: pid)
    }

    /// Frontmost-target drag: route through `.cghidEventTap` so the
    /// gesture originates from the system input stream — matches what
    /// AppKit drag sources / Finder selection rect / canvas viewports
    /// expect. The real cursor visibly traces the drag path; we
    /// accept that for frontmost gestures.
    private static func dragFrontmostViaHIDTap(
        from start: CGPoint,
        to end: CGPoint,
        button: Button,
        steps: Int,
        perStepUs: useconds_t,
        modifiers: [String]
    ) throws {
        let (downType, upType) = cgEventTypes(for: button)
        let draggedType = cgDraggedType(for: button)
        let mouseButton: CGMouseButton = {
            switch button {
            case .left: return .left
            case .right: return .right
            case .middle: return .center
            }
        }()
        let modifierFlags = cgEventFlags(for: modifiers)
        let src = CGEventSource(stateID: .hidSystemState)

        guard
            let move = CGEvent(
                mouseEventSource: src,
                mouseType: .mouseMoved,
                mouseCursorPosition: start,
                mouseButton: mouseButton
            )
        else { throw MouseInputError.eventCreationFailed("drag hid-tap move") }
        move.flags = modifierFlags
        move.post(tap: .cghidEventTap)
        usleep(30_000)

        guard
            let down = CGEvent(
                mouseEventSource: src,
                mouseType: downType,
                mouseCursorPosition: start,
                mouseButton: mouseButton
            )
        else { throw MouseInputError.eventCreationFailed("drag hid-tap down") }
        down.flags = modifierFlags
        down.setIntegerValueField(.mouseEventClickState, value: 1)
        down.post(tap: .cghidEventTap)

        for step in 1...steps {
            let progress = Double(step) / Double(steps)
            let point = CGPoint(
                x: start.x + (end.x - start.x) * progress,
                y: start.y + (end.y - start.y) * progress
            )
            guard
                let drag = CGEvent(
                    mouseEventSource: src,
                    mouseType: draggedType,
                    mouseCursorPosition: point,
                    mouseButton: mouseButton
                )
            else { throw MouseInputError.eventCreationFailed("drag hid-tap step") }
            drag.flags = modifierFlags
            drag.setIntegerValueField(.mouseEventClickState, value: 1)
            usleep(perStepUs)
            drag.post(tap: .cghidEventTap)
        }

        guard
            let up = CGEvent(
                mouseEventSource: src,
                mouseType: upType,
                mouseCursorPosition: end,
                mouseButton: mouseButton
            )
        else { throw MouseInputError.eventCreationFailed("drag hid-tap up") }
        up.flags = modifierFlags
        up.setIntegerValueField(.mouseEventClickState, value: 1)
        usleep(perStepUs)
        up.post(tap: .cghidEventTap)
    }

    private static func nsDraggedType(for button: Button) -> NSEvent.EventType {
        switch button {
        case .left: return .leftMouseDragged
        case .right: return .rightMouseDragged
        case .middle: return .otherMouseDragged
        }
    }

    private static func cgDraggedType(for button: Button) -> CGEventType {
        switch button {
        case .left: return .leftMouseDragged
        case .right: return .rightMouseDragged
        case .middle: return .otherMouseDragged
        }
    }

    // MARK: - Private helpers

    private static func buildCGEvent(
        type: NSEvent.EventType,
        location: CGPoint,
        modifierFlags: NSEvent.ModifierFlags,
        clickCount: Int,
        button: Button,
        windowNumber: Int = 0
    ) throws -> CGEvent {
        // NSEvent's mouseEvent constructor encodes which button in the
        // event type itself (leftMouseDown vs rightMouseDown vs
        // otherMouseDown), so we don't need a separate button
        // argument. Pressure 1.0 matches what a real trackpad / mouse
        // reports for a resting click; 0.0 occasionally trips
        // pressure-sensitive handlers into thinking the event was
        // dropped mid-flight.
        //
        // windowNumber: pass the actual CGWindowID when targeting a
        // specific backgrounded window. When 0, NSApp.sendEvent falls
        // back to a screen-location hit-test that skips non-key windows,
        // so backgrounded AppKit targets never receive the event. The
        // actual window ID routes via NSApplication.window(with:)
        // directly, bypassing the key-window restriction.
        guard
            let ns = NSEvent.mouseEvent(
                with: type,
                location: location,
                modifierFlags: modifierFlags,
                timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: windowNumber,
                context: nil,
                eventNumber: 0,
                clickCount: clickCount,
                pressure: 1.0
            )
        else {
            throw MouseInputError.eventCreationFailed(
                "\(button.rawValue) \(type.rawValue)"
            )
        }
        guard let cg = ns.cgEvent else {
            throw MouseInputError.eventCreationFailed(
                "\(button.rawValue) \(type.rawValue) cgEvent bridge"
            )
        }
        return cg
    }

    /// Post `event` via two pid-routed paths — SkyLight's auth-signed
    /// SPI and the public `CGEvent.postToPid`. Neither moves the
    /// user's real cursor. See the file-level doc for why we fire
    /// both and why we deliberately skip `.cghidEventTap`.
    /// Frontmost-target click path: use `CGEventPost(tap:
    /// .cghidEventTap)` with a leading `mouseMoved`. This is the
    /// only route we've found that reaches OpenGL / GHOST-style
    /// viewports (Blender, Unity, various games) — those viewports
    /// filter out events delivered via per-pid routes (SLEventPostToPid,
    /// CGEventPostToPid) at the event-source level.
    ///
    /// Side effect: the real cursor visibly moves to the click point.
    /// That's unavoidable — `cghidEventTap` is the system-wide HID
    /// event stream and repositioning the cursor is part of how
    /// events find their target at dispatch time. Safe for frontmost
    /// targets (the user is already looking at that app anyway);
    /// Auth-signed pid-routed paths are what we use when the target
    /// is backgrounded and we can't afford the cursor motion.
    private static func clickFrontmostViaHIDTap(
        at point: CGPoint,
        button: Button,
        count: Int,
        modifiers: [String]
    ) throws {
        let clamped = max(1, min(3, count))
        let (downType, upType) = cgEventTypes(for: button)
        let mouseButton: CGMouseButton = {
            switch button {
            case .left: return .left
            case .right: return .right
            case .middle: return .center
            }
        }()
        let modifierFlags = cgEventFlags(for: modifiers)

        // hidSystemState mimics hardware origin — required by some
        // strict input-source filters (GHOST checks this).
        let src = CGEventSource(stateID: .hidSystemState)

        guard
            let move = CGEvent(
                mouseEventSource: src,
                mouseType: .mouseMoved,
                mouseCursorPosition: point,
                mouseButton: mouseButton
            )
        else { throw MouseInputError.eventCreationFailed("frontmost hid-tap") }
        move.flags = modifierFlags
        move.post(tap: .cghidEventTap)
        // 30 ms lets the OS propagate the cursor position before we
        // post the mouseDown. Blender's viewport specifically needs
        // this gap: without it, the mouseDown arrives while the
        // internal cursor-tracking cache still reports the old
        // position and the click is filtered out.
        usleep(30_000)

        for clickIndex in 1...clamped {
            guard
                let down = CGEvent(
                    mouseEventSource: src,
                    mouseType: downType,
                    mouseCursorPosition: point,
                    mouseButton: mouseButton
                ),
                let up = CGEvent(
                    mouseEventSource: src,
                    mouseType: upType,
                    mouseCursorPosition: point,
                    mouseButton: mouseButton
                )
            else { throw MouseInputError.eventCreationFailed("frontmost hid-tap") }
            down.flags = modifierFlags
            up.flags = modifierFlags
            down.setIntegerValueField(.mouseEventClickState, value: Int64(clickIndex))
            up.setIntegerValueField(.mouseEventClickState, value: Int64(clickIndex))
            down.post(tap: .cghidEventTap)
            usleep(20_000)
            up.post(tap: .cghidEventTap)
            if clickIndex < clamped {
                usleep(80_000)
            }
        }
    }

    private static func cgEventTypes(
        for button: Button
    ) -> (down: CGEventType, up: CGEventType) {
        switch button {
        case .left: return (.leftMouseDown, .leftMouseUp)
        case .right: return (.rightMouseDown, .rightMouseUp)
        case .middle: return (.otherMouseDown, .otherMouseUp)
        }
    }

    /// Map our `modifiers: [String]` to `CGEventFlags`. Separate from
    /// the NSEvent variant because CG and NS have different raw
    /// values; the right path uses the right type rather than forcing
    /// a cross-API cast.
    private static func cgEventFlags(for modifiers: [String]) -> CGEventFlags {
        var flags: CGEventFlags = []
        for raw in modifiers {
            switch raw.lowercased() {
            case "cmd", "command": flags.insert(.maskCommand)
            case "shift": flags.insert(.maskShift)
            case "option", "alt", "opt": flags.insert(.maskAlternate)
            case "ctrl", "control": flags.insert(.maskControl)
            case "fn", "function": flags.insert(.maskSecondaryFn)
            default: break
            }
        }
        return flags
    }

    private static func postBoth(_ event: CGEvent, toPid pid: pid_t) {
        // SkyLight path first — the auth-message factory selects the
        // mouse envelope automatically. Returns `false` when the SPI
        // isn't resolvable; in that case the public CGEvent path
        // below is the only delivery we'll attempt (still cursor-
        // neutral, still pid-routed).
        _ = SkyLightEventPost.postToPid(pid, event: event)
        // Public pid-routed post. Delivers to the target's mach port
        // without warping the on-screen cursor. Empirically lands on
        // AppKit targets where SkyLight's mouse delivery drops.
        event.postToPid(pid)
    }

    private static func nsEventTypes(
        for button: Button
    ) -> (down: NSEvent.EventType, up: NSEvent.EventType) {
        switch button {
        case .left: return (.leftMouseDown, .leftMouseUp)
        case .right: return (.rightMouseDown, .rightMouseUp)
        case .middle: return (.otherMouseDown, .otherMouseUp)
        }
    }

    /// Convert a screen point in the Quartz convention (top-left
    /// origin, y-down) to the AppKit convention NSEvent expects
    /// (bottom-left of the main screen, y-up). Flipping against the
    /// main screen works even for multi-monitor layouts because
    /// AppKit's "screen-zero" reference frame is shared across all
    /// screens — they just extend above/below/beside it.
    private static func cocoaLocation(
        fromScreenPoint point: CGPoint
    ) -> CGPoint {
        let mainScreenHeight = NSScreen.main?.frame.height
            ?? NSScreen.screens.first?.frame.height
            ?? 0
        return CGPoint(x: point.x, y: mainScreenHeight - point.y)
    }

    /// Modifier-name → `NSEvent.ModifierFlags` mapping, kept in sync
    /// with `KeyboardInput.modifierMask(for:)` (which produces
    /// `CGEventFlags`). Four lines of duplication is cheaper than
    /// lifting to a shared helper that crosses the CGEvent/NSEvent
    /// type boundary.
    private static func modifierMask(
        for modifiers: [String]
    ) -> NSEvent.ModifierFlags {
        var mask: NSEvent.ModifierFlags = []
        for raw in modifiers {
            switch raw.lowercased() {
            case "cmd", "command": mask.insert(.command)
            case "shift": mask.insert(.shift)
            case "option", "alt": mask.insert(.option)
            case "ctrl", "control": mask.insert(.control)
            case "fn": mask.insert(.function)
            default: break
            }
        }
        return mask
    }
}

public enum MouseInputError: Error, CustomStringConvertible, Sendable {
    case eventCreationFailed(String)

    public var description: String {
        switch self {
        case .eventCreationFailed(let phase):
            return "Failed to create CGEvent for \(phase)."
        }
    }
}
