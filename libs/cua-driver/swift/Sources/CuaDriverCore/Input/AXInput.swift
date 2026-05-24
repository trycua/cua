import ApplicationServices
import CoreGraphics
import Foundation

public struct TargetedElement: Sendable, Codable, Hashable {
    public let role: String?
    public let subrole: String?
    public let title: String?
    public let description: String?

    public init(role: String?, subrole: String?, title: String?, description: String?) {
        self.role = role
        self.subrole = subrole
        self.title = title
        self.description = description
    }
}

public enum AXInputError: Error, CustomStringConvertible, Sendable {
    case notAuthorized
    case noElementAt(CGPoint)
    case noFocusedElement
    case actionFailed(action: String, code: Int32)
    case setAttributeFailed(attribute: String, code: Int32)

    public var description: String {
        switch self {
        case .notAuthorized:
            return "Accessibility permission not granted — call `check_permissions` with {\"prompt\": true}."
        case .noElementAt(let p):
            return "No AX element at (\(Int(p.x)), \(Int(p.y)))."
        case .noFocusedElement:
            return "No element currently has keyboard focus."
        case .actionFailed(let action, let code):
            return "AX action \(action) failed with code \(code)."
        case .setAttributeFailed(let attribute, let code):
            return "AX setAttribute \(attribute) failed with code \(code)."
        }
    }
}

public enum AXInput {
    /// Throws ``AXInputError.notAuthorized`` if the host process lacks the
    /// Accessibility TCC grant. Call this at the entry of every action tool.
    public static func requireAuthorized() throws {
        guard AXIsProcessTrusted() else {
            throw AXInputError.notAuthorized
        }
    }

    /// Resolve the AX element at the given screen point (points, top-left
    /// origin). Throws if no element is returned — note that desktop
    /// background, menubar, dock, etc. are all elements, so a plausible
    /// on-screen point will nearly always resolve to *something*.
    public static func elementAt(_ point: CGPoint) throws -> AXUIElement {
        try requireAuthorized()
        let system = AXUIElementCreateSystemWide()
        var element: AXUIElement?
        let result = AXUIElementCopyElementAtPosition(
            system,
            Float(point.x),
            Float(point.y),
            &element
        )
        guard result == .success, let resolved = element else {
            throw AXInputError.noElementAt(point)
        }
        return resolved
    }

    /// Resolve the system-wide focused UI element.
    public static func focusedElement() throws -> AXUIElement {
        try requireAuthorized()
        return try resolveFocused(on: AXUIElementCreateSystemWide())
    }

    /// Resolve the focused element within `pid`'s own AX tree.
    /// Prefer this over the system-wide variant when the caller knows
    /// which app should receive the input — avoids routing to whatever
    /// happens to be frontmost.
    public static func focusedElement(pid: pid_t) throws -> AXUIElement {
        try requireAuthorized()
        return try resolveFocused(on: AXUIElementCreateApplication(pid))
    }

    private static func resolveFocused(on root: AXUIElement) throws -> AXUIElement {
        var focused: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            root,
            "AXFocusedUIElement" as CFString,
            &focused
        )
        guard
            result == .success,
            let element = focused,
            CFGetTypeID(element) == AXUIElementGetTypeID()
        else {
            throw AXInputError.noFocusedElement
        }
        return unsafeBitCast(element, to: AXUIElement.self)
    }

    public static func performAction(_ action: String, on element: AXUIElement) throws {
        let result = AXUIElementPerformAction(element, action as CFString)
        guard result == .success else {
            throw AXInputError.actionFailed(action: action, code: result.rawValue)
        }
    }

    /// The AX action names the element advertises via
    /// `AXUIElementCopyActionNames`. Empty when the element has no
    /// actions or the copy call fails. Used by click tools to warn
    /// when a caller-requested action (e.g. `AXPress`) isn't in the
    /// advertised list — `AXUIElementPerformAction` still returns
    /// `success` in that case but the element is a no-op, and the
    /// caller deserves to know the action likely did nothing.
    public static func advertisedActionNames(of element: AXUIElement) -> [String] {
        var names: CFArray?
        let result = AXUIElementCopyActionNames(element, &names)
        guard result == .success, let names = names as? [String] else { return [] }
        return names
    }

    public static func setAttribute(
        _ attribute: String,
        on element: AXUIElement,
        value: CFTypeRef
    ) throws {
        let result = AXUIElementSetAttributeValue(element, attribute as CFString, value)
        guard result == .success else {
            throw AXInputError.setAttributeFailed(attribute: attribute, code: result.rawValue)
        }
    }

    /// Snapshot the element's identifying attributes for logging/return values.
    public static func describe(_ element: AXUIElement) -> TargetedElement {
        TargetedElement(
            role: attributeString(element, "AXRole"),
            subrole: attributeString(element, "AXSubrole"),
            title: attributeString(element, "AXTitle"),
            description: attributeString(element, "AXDescription")
        )
    }

    /// The element's screen-point bounding rect (top-left origin).
    /// Returns nil when the element has no position / size.
    /// Public entry point used by ClickTool for the focus-rect overlay.
    public static func screenBoundingRect(of element: AXUIElement) -> CGRect? {
        return boundingRect(of: element)
    }

    /// Read the element's `AXChildren` attribute. Returns an empty array on
    /// any error (element has no children, AX permission denied, etc.).
    public static func children(of element: AXUIElement) -> [AXUIElement] {
        var ref: CFTypeRef?
        guard
            AXUIElementCopyAttributeValue(element, "AXChildren" as CFString, &ref) == .success,
            let arr = ref as? [AXUIElement]
        else { return [] }
        return arr
    }

    /// Read a string attribute from an element. Returns nil when the attribute
    /// is absent, unreadable, or not a string.
    public static func stringAttribute(_ name: String, of element: AXUIElement) -> String? {
        return attributeString(element, name)
    }

    /// Read a boolean attribute from an element. Returns nil when the attribute
    /// is absent or unreadable.
    public static func boolAttribute(_ name: String, of element: AXUIElement) -> Bool? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success,
              let v = value else { return nil }
        guard CFGetTypeID(v) == CFBooleanGetTypeID() else { return nil }
        return CFBooleanGetValue((v as! CFBoolean))
    }

    /// The element's on-screen center in screen-point coordinates
    /// (top-left origin). Returns nil when the element has no
    /// position / size (menus, offscreen elements, hidden rows).
    ///
    /// Used to animate the agent cursor to the target BEFORE firing
    /// an AX action — purely a visual/telemetry concern; the AX
    /// action itself doesn't need coordinates.
    ///
    /// **Hit-test verified**: some composite controls (e.g. Voice
    /// Memos' bottom Record bar) report `AXPosition` / `AXSize` that
    /// describe a parent container whose center sits in empty space
    /// rather than over the visible control. The overlay cursor
    /// glides to an off-control point even though the AX dispatch
    /// lands correctly on the element. To fix the visual mismatch,
    /// we pixel-hit-test the computed center and scan a grid of
    /// fallback points inside the rect if the center doesn't
    /// resolve back to the target (or one of its descendants).
    public static func screenCenter(of element: AXUIElement) -> CGPoint? {
        guard let rect = boundingRect(of: element) else { return nil }

        let center = CGPoint(x: rect.midX, y: rect.midY)
        if hitTestResolves(to: element, at: center) { return center }

        // AX center fell outside the visible control — scan a small
        // grid of fallback points inside the rect and return the
        // first that hit-tests back to the target (or a descendant).
        // 5×5 grid skipping corners (corners often land on padding /
        // neighbouring hit-regions). 17 points total; each hit-test
        // is a ~sub-ms AX roundtrip.
        let cols = 5
        let rows = 5
        for r in 0..<rows {
            for c in 0..<cols {
                if (r == 0 || r == rows - 1)
                    && (c == 0 || c == cols - 1) { continue }
                let fx = (CGFloat(c) + 0.5) / CGFloat(cols)
                let fy = (CGFloat(r) + 0.5) / CGFloat(rows)
                let point = CGPoint(
                    x: rect.minX + rect.width * fx,
                    y: rect.minY + rect.height * fy
                )
                if hitTestResolves(to: element, at: point) { return point }
            }
        }

        // Nothing inside the rect hit-tests back — the element is
        // fully occluded or its bounds are wrong. Return the AX
        // center anyway; the overlay glide will be wrong but the AX
        // dispatch still lands correctly on the element.
        return center
    }

    /// Read an element's screen-point bounding rect from
    /// `AXPosition` + `AXSize`. Returns nil when either attribute
    /// is missing or the size is non-positive.
    private static func boundingRect(of element: AXUIElement) -> CGRect? {
        var posValue: CFTypeRef?
        var sizeValue: CFTypeRef?
        guard
            AXUIElementCopyAttributeValue(element, "AXPosition" as CFString, &posValue)
                == .success,
            AXUIElementCopyAttributeValue(element, "AXSize" as CFString, &sizeValue)
                == .success,
            let posValue, let sizeValue,
            CFGetTypeID(posValue) == AXValueGetTypeID(),
            CFGetTypeID(sizeValue) == AXValueGetTypeID()
        else { return nil }

        var origin = CGPoint.zero
        var size = CGSize.zero
        AXValueGetValue(posValue as! AXValue, .cgPoint, &origin)
        AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
        guard size.width > 0, size.height > 0 else { return nil }
        return CGRect(origin: origin, size: size)
    }

    /// Does a hit-test at `point` resolve to `target` or to one of
    /// its descendants? "Descendant" = walking up `AXParent` from
    /// the hit result eventually returns the target. Used by
    /// `screenCenter(of:)` to validate that the glide target sits
    /// over the visible control, not in empty padding.
    ///
    /// Silently returns false on any AX error (hit-test at an
    /// off-screen point, non-AX surface, etc.) — this is a
    /// best-effort visual correctness check, not a correctness gate
    /// on the click itself.
    private static func hitTestResolves(
        to target: AXUIElement, at point: CGPoint
    ) -> Bool {
        guard let hit = try? elementAt(point) else { return false }
        // Walk up from hit checking for equality with target. Cap
        // at 16 hops — pathological deeply-nested trees shouldn't
        // stall a click. Elements form a tree; in practice the
        // chain from a leaf to any ancestor is <10 hops.
        var current: AXUIElement? = hit
        for _ in 0..<16 {
            guard let node = current else { return false }
            if CFEqual(node, target) { return true }
            var parent: CFTypeRef?
            let result = AXUIElementCopyAttributeValue(
                node, "AXParent" as CFString, &parent
            )
            guard
                result == .success,
                let parent,
                CFGetTypeID(parent) == AXUIElementGetTypeID()
            else { return false }
            current = unsafeBitCast(parent, to: AXUIElement.self)
        }
        return false
    }

    private static func attributeString(_ element: AXUIElement, _ attribute: String) -> String? {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, attribute as CFString, &value)
        guard result == .success else { return nil }
        return value as? String
    }
}
