// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// An sRGB color parsed from the hex string presence hands out.
public struct RGB: Equatable, Hashable, Sendable {
    public var r: Double
    public var g: Double
    public var b: Double

    public init(r: Double, g: Double, b: Double) {
        self.r = r
        self.g = g
        self.b = b
    }

    /// `#rrggbb` or `rrggbb`. Nil for anything else.
    public init?(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        r = Double((v >> 16) & 0xFF) / 255
        g = Double((v >> 8) & 0xFF) / 255
        b = Double(v & 0xFF) / 255
    }

    /// WCAG 2 relative luminance.
    public var luminance: Double {
        func lin(_ c: Double) -> Double { c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    public func contrast(with other: RGB) -> Double {
        let a = luminance, b = other.luminance
        return (max(a, b) + 0.05) / (min(a, b) + 0.05)
    }

    public static let white = RGB(r: 1, g: 1, b: 1)
    public static let black = RGB(r: 0, g: 0, b: 0)
    /// Used before presence has told us a color: neutral gray.
    public static let unassigned = RGB(r: 0.56, g: 0.57, b: 0.6)
}

/// How one agent is drawn everywhere: its cursor on the canvas and the
/// avatar in its thread window (message bubbles stay neutral).
///
/// One source of truth for the color: the participant's `color` as the
/// Space's presence service assigned it, authoritative once the agent has
/// joined; before that, the SDK's stable `presence_color` for the id. Text
/// on the color is the SDK's `presence_text_color`. Both come in through
/// `PresenceColorSource`, so this module never invents a palette.
public struct AgentStyle: Equatable, Hashable, Sendable {
    public let fill: RGB
    public let text: RGB
    public let name: String

    public init(fill: RGB, text: RGB, name: String) {
        self.fill = fill
        self.text = text
        self.name = name
    }

    public var textContrast: Double { fill.contrast(with: text) }
}

/// The SDK's color functions (`PresenceColors` in CuaSpacesStreaming),
/// injected so the model stays SDK-free and testable.
public struct PresenceColorSource: Sendable {
    /// Stable `#rrggbb` for a principal id.
    public var stable: @Sendable (String) -> String
    /// `#000000` or `#ffffff` for text on a `#rrggbb` background.
    public var textOn: @Sendable (String) -> String

    public init(stable: @escaping @Sendable (String) -> String, textOn: @escaping @Sendable (String) -> String) {
        self.stable = stable
        self.textOn = textOn
    }

    public func style(color hex: String, name: String) -> AgentStyle {
        let fill = RGB(hex: hex) ?? .unassigned
        let text = RGB(hex: textOn(hex)) ?? (fill.luminance > 0.4 ? .black : .white)
        return AgentStyle(fill: fill, text: text, name: name)
    }
}

/// One participant as the canvas knows it.
public struct PresenceMember: Equatable, Sendable {
    public var participantID: String
    public var principalID: String
    public var name: String
    public var color: String
    public var isAgent: Bool

    public init(participantID: String, principalID: String, name: String, color: String, isAgent: Bool) {
        self.participantID = participantID
        self.principalID = principalID
        self.name = name
        self.color = color
        self.isAgent = isAgent
    }

}

/// Every Space's participants, and which agent participant acts for which
/// thread.
///
/// Binding a thread to a participant: an agent's cursor joins presence when
/// its first driver action lands, keyed by the driver's cursor id. The thread
/// that had a turn running in that Space at that moment is the one acting, so
/// it is bound to the new agent participant. A thread without a binding draws
/// its bubbles in the unassigned gray, exactly as its (not yet existing)
/// cursor would be: nothing invents a color presence did not assign.
public struct PresenceDirectory: Sendable {
    public let colors: PresenceColorSource
    /// spaceID -> participantID -> member
    public private(set) var members: [String: [String: PresenceMember]] = [:]
    /// threadID -> (spaceID, participantID)
    public private(set) var bindings: [String: Binding] = [:]

    public struct Binding: Equatable, Hashable, Sendable {
        public var spaceID: String
        public var participantID: String
    }

    public init(colors: PresenceColorSource) { self.colors = colors }

    public mutating func join(_ m: PresenceMember, in spaceID: String) {
        members[spaceID, default: [:]][m.participantID] = m
    }

    public mutating func leave(_ participantID: String, in spaceID: String) {
        members[spaceID]?[participantID] = nil
    }

    public func member(_ participantID: String, in spaceID: String) -> PresenceMember? {
        members[spaceID]?[participantID]
    }

    public func agents(in spaceID: String) -> [PresenceMember] {
        (members[spaceID] ?? [:]).values.filter(\.isAgent).sorted { $0.participantID < $1.participantID }
    }

    /// The style of a participant's cursor: the server-assigned color,
    /// else the stable color of its principal.
    public func cursorStyle(participantID: String, in spaceID: String) -> AgentStyle {
        let m = member(participantID, in: spaceID)
        let hex = (m?.color).flatMap { $0.isEmpty ? nil : $0 } ?? colors.stable(m?.principalID ?? participantID)
        return colors.style(color: hex, name: m?.name ?? "Agent")
    }

    /// The style of a thread's avatar: its bound participant's cursor style;
    /// before the agent has joined presence, the stable color of the thread.
    public func avatarStyle(threadID: String, fallbackName: String = "Agent") -> AgentStyle {
        guard let b = bindings[threadID], member(b.participantID, in: b.spaceID) != nil else {
            return colors.style(color: colors.stable(threadID), name: fallbackName)
        }
        return cursorStyle(participantID: b.participantID, in: b.spaceID)
    }

    public mutating func bind(threadID: String, to participantID: String, in spaceID: String) {
        bindings[threadID] = Binding(spaceID: spaceID, participantID: participantID)
    }

    /// An agent participant appeared in `spaceID`. If exactly one unbound
    /// thread is mid-turn in that Space, it is the one acting: bind it.
    /// Returns the bound thread.
    @discardableResult
    public mutating func agentAppeared(_ participantID: String, in spaceID: String,
                                       activeThreads: [String]) -> String? {
        if bindings.values.contains(Binding(spaceID: spaceID, participantID: participantID)) {
            return nil
        }
        let unbound = activeThreads.filter { bindings[$0] == nil }
        guard unbound.count == 1, let thread = unbound.first else { return nil }
        bind(threadID: thread, to: participantID, in: spaceID)
        return thread
    }

    public func thread(boundTo participantID: String, in spaceID: String) -> String? {
        bindings.first { $0.value == Binding(spaceID: spaceID, participantID: participantID) }?.key
    }
}

/// A cursor that glides to where presence last put it.
///
/// Presence coalesces cursors to 20 Hz; drawing them at those positions looks
/// like teleporting. This is a critically damped spring toward the latest
/// target, stepped once per display frame, so the cursor moves smoothly at
/// the display's rate and settles without overshoot.
public struct CursorGlide: Equatable, Sendable {
    public private(set) var position: CGPoint
    public private(set) var velocity: CGVector = .zero
    public var target: CGPoint
    /// Spring stiffness (1/s). Higher follows more tightly.
    public var omega: Double = 18

    public init(at p: CGPoint) {
        position = p
        target = p
    }

    public var isSettled: Bool {
        abs(position.x - target.x) < 0.25 && abs(position.y - target.y) < 0.25
            && abs(velocity.dx) < 1 && abs(velocity.dy) < 1
    }

    /// Advance `dt` seconds (exact solution of the critically damped spring,
    /// stable at any frame time).
    public mutating func step(_ dt: TimeInterval) {
        let dt = min(max(dt, 0), 0.1)
        func axis(_ x: Double, _ v: Double, _ t: Double) -> (Double, Double) {
            let d = x - t
            let e = exp(-omega * dt)
            let c = v + omega * d
            let nd = (d + c * dt) * e
            let nv = (v - omega * c * dt) * e
            return (t + nd, nv)
        }
        let (x, vx) = axis(position.x, velocity.dx, target.x)
        let (y, vy) = axis(position.y, velocity.dy, target.y)
        position = CGPoint(x: x, y: y)
        velocity = CGVector(dx: vx, dy: vy)
        if isSettled {
            position = target
            velocity = .zero
        }
    }
}

/// Map a display-normalized agent cursor onto the window it is over.
///
/// Driver cursors are reported against the whole display (`[0, 1]` of the
/// primary display). A window tile needs the position within that window:
/// the front-most window whose bounds contain the point wins.
public struct WindowPlacement: Equatable, Sendable {
    public var windowID: String
    /// Window bounds on the display, in the display's points.
    public var bounds: CGRect
    public var z: Int

    public init(windowID: String, bounds: CGRect, z: Int) {
        self.windowID = windowID
        self.bounds = bounds
        self.z = z
    }

    /// Returns the window and the point inside it as `[0, 1]` fractions.
    public static func locate(_ normalized: CGPoint, display: CGSize,
                              windows: [WindowPlacement]) -> (windowID: String, point: CGPoint)? {
        let p = CGPoint(x: normalized.x * display.width, y: normalized.y * display.height)
        guard let w = windows.sorted(by: { $0.z > $1.z }).first(where: { $0.bounds.contains(p) }),
              w.bounds.width > 0, w.bounds.height > 0 else { return nil }
        return (w.windowID, CGPoint(x: (p.x - w.bounds.minX) / w.bounds.width,
                                    y: (p.y - w.bounds.minY) / w.bounds.height))
    }
}

/// A person's pointer path between two points: a gently curved cubic
/// bezier, minimum-jerk timing (fast in the middle, slow at the ends), a
/// small overshoot that settles back, and a slight tremor. Deterministic
/// for a seed, so a scripted coworker moves the same way every run.
public struct HumanPath: Sendable {
    public struct Sample: Equatable, Sendable {
        public var t: TimeInterval
        public var point: CGPoint
    }

    /// Fitts-like duration: longer for longer moves, clamped.
    public static func duration(from a: CGPoint, to b: CGPoint, scale: CGFloat) -> TimeInterval {
        let d = Double(hypot(b.x - a.x, b.y - a.y) / max(scale, 1))
        return min(max(0.28 + 0.12 * log2(1 + d * 12), 0.35), 1.1)
    }

    /// Samples at `rate` Hz from `a` to `b`. `scale` is the surface size
    /// the points are in (1 for normalized coordinates).
    public static func samples(from a: CGPoint, to b: CGPoint, seed: UInt64, rate: Double = 60,
                               scale: CGFloat = 1) -> [Sample] {
        var rng = SplitMix(seed: seed)
        let dx = b.x - a.x, dy = b.y - a.y
        let dist = hypot(dx, dy)
        guard dist > 1e-6 else { return [Sample(t: 0, point: b)] }
        // Perpendicular bow, 8-18% of the distance, either side.
        let n = CGPoint(x: -dy / dist, y: dx / dist)
        let bow = dist * CGFloat(0.08 + 0.10 * rng.unit()) * (rng.unit() < 0.5 ? -1 : 1)
        let c1 = CGPoint(x: a.x + dx * 0.3 + n.x * bow, y: a.y + dy * 0.3 + n.y * bow)
        let c2 = CGPoint(x: a.x + dx * 0.75 + n.x * bow * 0.6, y: a.y + dy * 0.75 + n.y * bow * 0.6)
        // Overshoot a little past the target along the direction of travel.
        let over = CGFloat(0.02 + 0.025 * rng.unit())
        let end = CGPoint(x: b.x + dx * over, y: b.y + dy * over)
        let dur = duration(from: a, to: b, scale: scale)
        let settle = 0.12
        var out: [Sample] = []
        let steps = max(Int(dur * rate), 2)
        for i in 0 ... steps {
            let u = Double(i) / Double(steps)
            let s = CGFloat(10 * pow(u, 3) - 15 * pow(u, 4) + 6 * pow(u, 5))
            let p = cubic(a, c1, c2, end, s)
            let jitter = CGFloat(rng.unit() - 0.5) * scale * 0.0015 * (1 - s)
            out.append(Sample(t: u * dur, point: CGPoint(x: p.x + jitter, y: p.y - jitter)))
        }
        let settleSteps = max(Int(settle * rate), 2)
        for i in 1 ... settleSteps {
            let u = CGFloat(Double(i) / Double(settleSteps))
            let e = 1 - (1 - u) * (1 - u)
            out.append(Sample(t: dur + settle * Double(u),
                              point: CGPoint(x: end.x + (b.x - end.x) * e, y: end.y + (b.y - end.y) * e)))
        }
        return out
    }

    static func cubic(_ p0: CGPoint, _ p1: CGPoint, _ p2: CGPoint, _ p3: CGPoint, _ t: CGFloat) -> CGPoint {
        let u = 1 - t
        let a = u * u * u, b = 3 * u * u * t, c = 3 * u * t * t, d = t * t * t
        return CGPoint(x: a * p0.x + b * p1.x + c * p2.x + d * p3.x, y: a * p0.y + b * p1.y + c * p2.y + d * p3.y)
    }

    /// Delay before each character when a person types `text`: 70-190 ms,
    /// longer after a space or punctuation.
    public static func typingDelays(_ text: String, seed: UInt64) -> [TimeInterval] {
        var rng = SplitMix(seed: seed)
        var prev: Character = " "
        return text.map { ch in
            defer { prev = ch }
            var d = 0.07 + 0.12 * rng.unit()
            if prev == " " { d += 0.05 + 0.1 * rng.unit() }
            if ",.!?".contains(prev) { d += 0.18 + 0.2 * rng.unit() }
            return d
        }
    }
}

/// A tiny deterministic generator (SplitMix64).
public struct SplitMix: Sendable {
    private var state: UInt64
    public init(seed: UInt64) { state = seed }
    public mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
    public mutating func unit() -> Double { Double(next() >> 11) / Double(1 << 53) }
}

/// Which way an agent's cursor points.
///
/// The same convention as Cua Driver's overlay (`cursor-overlay`,
/// `RenderStateCore.heading`): the heading is the direction of travel plus
/// π, the resting heading is π/4 (the tip up and to the left, the classic
/// pointer), and the artwork is drawn rotated by `heading - π/4`. The driver
/// assigns the path tangent directly while it follows a planned path; the
/// canvas only sees positions, so it low-passes the heading toward the
/// velocity's direction (fast while moving, slower back to rest) to keep a
/// 20 Hz presence stream from making the arrow twitch.
public struct CursorHeading: Equatable, Sendable {
    public static let rest = Double.pi / 4
    public private(set) var heading = rest
    /// Below this speed (points per second) the cursor is at rest.
    public var minSpeed: Double = 60
    /// Time constants (seconds): turning toward travel, and settling back.
    public var moveTau: Double = 0.08
    public var restTau: Double = 0.35

    public init() {}

    /// The heading the velocity asks for (before smoothing).
    public func target(for velocity: CGVector) -> Double {
        let speed = hypot(velocity.dx, velocity.dy)
        return speed >= minSpeed ? atan2(Double(velocity.dy), Double(velocity.dx)) + .pi : Self.rest
    }

    public mutating func step(velocity: CGVector, dt: TimeInterval) {
        guard dt > 0 else { return }
        let moving = hypot(velocity.dx, velocity.dy) >= minSpeed
        let goal = target(for: velocity)
        let tau = moving ? moveTau : restTau
        let k = 1 - exp(-min(dt, 0.1) / tau)
        heading = Self.wrap(heading + Self.shortest(from: heading, to: goal) * k)
    }

    /// How far to rotate the artwork from its drawn orientation (radians).
    public var rotation: Double { Self.shortest(from: Self.rest, to: heading) }

    /// The signed smallest angle from `a` to `b`, in `(-π, π]`.
    public static func shortest(from a: Double, to b: Double) -> Double {
        var d = (b - a).truncatingRemainder(dividingBy: 2 * .pi)
        if d > .pi { d -= 2 * .pi }
        if d <= -.pi { d += 2 * .pi }
        return d
    }

    static func wrap(_ a: Double) -> Double {
        var r = a.truncatingRemainder(dividingBy: 2 * .pi)
        if r < 0 { r += 2 * .pi }
        return r
    }
}
