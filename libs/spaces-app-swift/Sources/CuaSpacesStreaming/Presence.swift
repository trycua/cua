// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation

/// Multi-participant cursor presence, as the protocol already defines it.
///
/// Presence is the Space's `PresenceService` (join, roster, cursors), reached
/// through the cua SDK's `SpacePresence`. This file is the SDK-side model:
/// the roster diff, the send cadence, and the conversion from the SDK's
/// participants.
///
/// The layering this file settles:
///
/// * **Broadcast, identity and disconnect are the spacesd's.**
/// * **The participant model and the event stream are the SDK's** — this file.
///   It keeps the roster, applies the ~30 Hz send cadence, and reconciles
///   `presence` against `remote_cursor` so a departed participant's cursor
///   actually goes away.
/// * **Drawing a cursor is the app's.** Nothing here draws.
///
/// One protocol gap worth naming rather than papering over: `user_id` is
/// `user-<connection_id>`, minted per socket, so it is *not* stable across a
/// reconnect and is not tied to any account. `Participant.id` says exactly that
/// much and no more.

// MARK: - Wire payloads

/// One participant in a Space's presence roster.
///
/// The colour is server-assigned: a client sends `color: null` on `join` and
/// the daemon allocates round-robin, which is the only way two clients that
/// never meet get different colours.
public struct PresenceUser: Codable, Hashable, Sendable, Identifiable {
    public var user_id: String
    public var name: String
    public var color: String
    /// `human` or `agent`, when the server said.
    public var kind: String? = nil
    /// The identity it joined with (`PresenceIdentity.id`), when known.
    public var principal_id: String? = nil

    public var id: String { user_id }

    public init(user_id: String, name: String, color: String, kind: String? = nil,
                principal_id: String? = nil) {
        self.user_id = user_id
        self.name = name
        self.color = color
        self.kind = kind
        self.principal_id = principal_id
    }

    /// An SDK participant.
    public init(_ p: PresenceParticipant) {
        self.init(user_id: p.participantId, name: p.displayName,
                  color: p.color.isEmpty ? PresenceRoster.fallbackColor : p.color,
                  kind: p.kind.isEmpty ? nil : p.kind,
                  principal_id: p.principalId.isEmpty ? nil : p.principalId)
    }
}

/// The pointer shape a participant is showing.
///
/// macOS capture deliberately excludes the cursor from the captured pixels —
/// the origin desktop has one physical pointer and presence cursors are
/// per-participant overlays, so a baked-in pointer cannot be one. The shape
/// therefore rides as metadata. A remote participant's cursor arrives as
/// `.unknown`, because it does not own the physical pointer.
public enum CursorShape: Sendable, Hashable {
    case `default`
    case text
    case verticalText
    case pointer
    case grab
    case grabbing
    case crosshair
    case wait
    case notAllowed
    case resize(axis: String)
    /// A custom bitmap, capped by the protocol at 64 KiB.
    case custom(png: Data, hotspot: CGPoint, scale: Double)
    case unknown
    case unsupported

    /// From a presence shape name (`cua.env.v1.CursorShape`, the shared-art
    /// key). `progress` reads as `wait` and `move` as an all-axis resize in
    /// this older vocabulary; keep `CursorState.shapeName` for the exact one.
    public static func fromPresence(_ name: String) -> CursorShape {
        switch name {
        case "arrow": return .default
        case "text": return .text
        case "pointer": return .pointer
        case "grab": return .grab
        case "grabbing": return .grabbing
        case "crosshair": return .crosshair
        case "wait", "progress": return .wait
        case "not_allowed": return .notAllowed
        case "resize_ns": return .resize(axis: "north_south")
        case "resize_ew": return .resize(axis: "east_west")
        case "resize_nesw": return .resize(axis: "north_east_south_west")
        case "resize_nwse": return .resize(axis: "north_west_south_east")
        case "move": return .resize(axis: "all")
        default: return .default
        }
    }

    static func decode(_ raw: [String: Any]?) -> CursorShape {
        switch raw?["kind"] as? String {
        case "default", nil: return .default
        case "text": return .text
        case "vertical_text": return .verticalText
        case "pointer": return .pointer
        case "grab": return .grab
        case "grabbing": return .grabbing
        case "crosshair": return .crosshair
        case "wait": return .wait
        case "not_allowed": return .notAllowed
        case "resize": return .resize(axis: raw?["axis"] as? String ?? "all")
        case "custom":
            let png = (raw?["png"] as? String).flatMap { Data(base64Encoded: $0) } ?? Data()
            return .custom(png: png,
                           hotspot: CGPoint(x: (raw?["hotspot_x"] as? NSNumber)?.doubleValue ?? 0,
                                            y: (raw?["hotspot_y"] as? NSNumber)?.doubleValue ?? 0),
                           scale: (raw?["scale"] as? NSNumber)?.doubleValue ?? 1)
        case "unsupported": return .unsupported
        default: return .unknown
        }
    }
}

/// One participant's cursor over one streamed window.
///
/// `point` is in **window pixels**, the same space the window's
/// `SurfaceGeometry` describes, so an overlay is placed with the same
/// `StreamGeometry` round trip that places the local cursor — and cannot drift
/// from it the way two independently-computed scales did (see `StreamGeometry`).
public struct CursorState: Sendable, Hashable {
    public var userID: String
    public var name: String
    public var color: String
    public var window: TargetHandle?
    public var point: CGPoint
    public var isVisible: Bool
    public var isPressed: Bool
    public var shape: CursorShape
    /// The shared-art name of the shape (`arrow`, `text`, `pointer`,
    /// `resize_ew`, ...; see `PresenceCursorArt`), as the Space's
    /// cua-spacesd computed it. `arrow` when the Space reports no shapes.
    public var shapeName: String = "arrow"
    /// The point as the protocol sent it, normalized to `0...1`, when known.
    /// Kept because a cursor can arrive before the first frame says how big
    /// the surface is.
    public var normalized: CGPoint?

    public init(userID: String, name: String, color: String, window: TargetHandle?,
                point: CGPoint, isVisible: Bool, isPressed: Bool, shape: CursorShape,
                normalized: CGPoint? = nil, shapeName: String = "arrow") {
        self.normalized = normalized
        self.shapeName = shapeName
        self.userID = userID
        self.name = name
        self.color = color
        self.window = window
        self.point = point
        self.isVisible = isVisible
        self.isPressed = isPressed
        self.shape = shape
    }

    /// The point normalized against a window's geometry, for a caller that
    /// draws in a view whose size is not the window's.
    public func normalizedPoint(in geometry: SurfaceGeometry) -> CGPoint {
        CGPoint(x: geometry.width_px > 0 ? point.x / CGFloat(geometry.width_px) : 0,
                y: geometry.height_px > 0 ? point.y / CGFloat(geometry.height_px) : 0)
    }

    static func decode(_ payload: [String: Any]) -> CursorState {
        let handle = payload["window"] as? String
        return CursorState(
            userID: payload["user_id"] as? String ?? "",
            name: payload["name"] as? String ?? "",
            color: payload["color"] as? String ?? PresenceRoster.fallbackColor,
            window: (handle?.isEmpty == false) ? TargetHandle(handle!) : nil,
            point: CGPoint(x: (payload["x"] as? NSNumber)?.doubleValue ?? 0,
                           y: (payload["y"] as? NSNumber)?.doubleValue ?? 0),
            isVisible: payload["visible"] as? Bool ?? true,
            isPressed: payload["pressed"] as? Bool ?? false,
            shape: CursorShape.decode(payload["shape"] as? [String: Any]))
    }
}

// MARK: - The participant model

/// A participant, with whatever is currently known about their cursor.
public struct Participant: Sendable, Hashable, Identifiable {
    /// The daemon's `user-<connection_id>`, or `host`, or `cua-agent`.
    ///
    /// **Not stable across a reconnect** — it is minted per socket. A caller
    /// that needs a durable identity must supply its own and match on `name`.
    public let id: String
    public var name: String
    /// `#RRGGBB`, assigned by the daemon.
    public var color: String
    /// The last cursor this participant reported, if any.
    public var cursor: CursorState?
    /// `human` or `agent`, when the server said.
    public var kind: String?
    /// The identity it joined with (`PresenceIdentity.id`), when known.
    public var principalID: String?

    /// The person physically at the machine the Space runs on, which the daemon
    /// synthesises rather than any client announcing.
    public var isHost: Bool { id == PresenceRoster.hostUserID }
    /// The CUA agent's own cursor, fed into presence by the driver so that what
    /// the agent does is visible as a participant rather than as a mystery.
    /// Also true for any participant that joined as an agent.
    public var isAgent: Bool { id == PresenceRoster.agentUserID || kind == "agent" }

    public init(id: String, name: String, color: String, cursor: CursorState? = nil,
                kind: String? = nil, principalID: String? = nil) {
        self.id = id
        self.name = name
        self.color = color
        self.cursor = cursor
        self.kind = kind
        self.principalID = principalID
    }

    init(_ user: PresenceUser) {
        self.init(id: user.user_id, name: user.name, color: user.color, kind: user.kind,
                  principalID: user.principal_id)
    }

    /// The cursor normalized to `0...1` over a surface of `surfaceSize`
    /// pixels, for an overlay drawn at any size; `nil` when hidden or unknown.
    public func normalizedCursor(in surfaceSize: CGSize) -> CGPoint? {
        guard let c = cursor, c.isVisible else { return nil }
        if let n = c.normalized { return n }
        guard surfaceSize.width > 0, surfaceSize.height > 0 else { return nil }
        return CGPoint(x: c.point.x / surfaceSize.width, y: c.point.y / surfaceSize.height)
    }
}

/// What changed in the roster.
public enum PresenceEvent: Sendable, Hashable {
    /// This client's own identity, as the daemon assigned it, plus the roster
    /// as it stood at join.
    case joined(me: Participant, roster: [Participant])
    /// A participant appeared.
    case participantJoined(Participant)
    /// A participant went away — a socket close on their side. The daemon has
    /// no explicit leave message; departure is a shrinking `presence` roster,
    /// which is why the SDK diffs it rather than waiting for one.
    case participantLeft(Participant)
    /// A participant's cursor moved, appeared, or hid.
    case cursorMoved(Participant)
    /// The whole roster, after any change.
    case rosterChanged([Participant])
}

/// The roster, and the diff that turns `presence` snapshots into events.
///
/// Pure and synchronous: it is the piece that decides *what happened*, and it
/// is testable with no daemon, no Space and no view.
public struct PresenceRoster: Sendable {
    public static let hostUserID = "host"
    public static let agentUserID = "cua-agent"
    public static let fallbackColor = "#3b82f6"

    public private(set) var participants: [String: Participant] = [:]
    public private(set) var me: Participant?
    /// The shape of this client's own cursor, as the Space computed it for
    /// the local pointer's position (`shape_changed` for `me`). Draw your
    /// own cursor at the local pointer with this shape.
    public private(set) var myShape: String = "arrow"

    public init() {}

    /// Sorted for a stable render order: host first, then agent, then everyone
    /// else by id. An overlay that reorders every tick flickers.
    public var ordered: [Participant] {
        participants.values.sorted { a, b in
            if a.isHost != b.isHost { return a.isHost }
            if a.isAgent != b.isAgent { return a.isAgent }
            return a.id < b.id
        }
    }

    /// Everyone but this client — the ones whose cursors an app should draw.
    /// The local pointer is already on screen; drawing a second marker for it
    /// is the one presence bug a user notices immediately.
    public var others: [Participant] {
        ordered.filter { $0.id != me?.id }
    }

    public mutating func apply(joined user: PresenceUser, roster: [PresenceUser]) -> [PresenceEvent] {
        me = Participant(user)
        var events = apply(roster: roster)
        events.insert(.joined(me: Participant(user), roster: ordered), at: 0)
        return events
    }

    /// Reconcile a full roster snapshot, preserving each surviving
    /// participant's last cursor. The daemon sends the whole list on every
    /// change, so arrivals and departures are both a diff.
    public mutating func apply(roster: [PresenceUser]) -> [PresenceEvent] {
        let incoming = Dictionary(roster.map { ($0.user_id, $0) }, uniquingKeysWith: { a, _ in a })
        var events: [PresenceEvent] = []

        for (id, participant) in participants where incoming[id] == nil {
            participants.removeValue(forKey: id)
            events.append(.participantLeft(participant))
        }
        for user in roster {
            if var existing = participants[user.user_id] {
                existing.name = user.name
                existing.color = user.color
                if user.kind != nil { existing.kind = user.kind }
                if user.principal_id != nil { existing.principalID = user.principal_id }
                participants[user.user_id] = existing
            } else {
                let fresh = Participant(user)
                participants[user.user_id] = fresh
                events.append(.participantJoined(fresh))
            }
        }
        if !events.isEmpty { events.append(.rosterChanged(ordered)) }
        return events
    }

    /// Fold in a cursor update.
    ///
    /// A cursor can arrive for a participant no `presence` has named yet — the
    /// host and the CUA agent are both broadcast by the daemon itself and never
    /// appear as a joining client. Rather than dropping those (which is what
    /// hiding behind the roster would do), the roster admits the participant
    /// the cursor describes.
    public mutating func apply(cursor: CursorState) -> [PresenceEvent] {
        guard !cursor.userID.isEmpty else { return [] }
        var events: [PresenceEvent] = []
        var participant = participants[cursor.userID]
            ?? {
                let fresh = Participant(id: cursor.userID,
                                        name: cursor.name.isEmpty ? cursor.userID : cursor.name,
                                        color: cursor.color)
                events.append(.participantJoined(fresh))
                return fresh
            }()
        participant.cursor = cursor
        if !cursor.color.isEmpty { participant.color = cursor.color }
        if !cursor.name.isEmpty { participant.name = cursor.name }
        participants[cursor.userID] = participant
        events.append(.cursorMoved(participant))
        return events
    }
}

extension PresenceRoster {
    /// The color an agent shows as, for its avatar and its cursor alike: the
    /// color the server assigned once it is present (a requested color is
    /// kept unless another participant already holds it), else its stable
    /// `PresenceColors.color(for:)`. Keyed by the id it joined with.
    public func colorOf(principalID: String) -> String {
        participants.values.first { $0.principalID == principalID }?.color
            ?? PresenceColors.color(for: principalID)
    }

    /// Seed from a freshly joined SDK session: `me` plus the members at join.
    public mutating func apply(me: PresenceParticipant, members: [PresenceMember],
                               surfaceSize: CGSize = CGSize(width: 1, height: 1)) -> [PresenceEvent] {
        var events = apply(joined: PresenceUser(me), roster: members.map { PresenceUser($0.participant) })
        for m in members where m.participant.participantId != me.participantId {
            if let c = m.cursor {
                events += apply(event: CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil,
                                                            participantId: m.participant.participantId,
                                                            cursor: c),
                                surfaceSize: surfaceSize)
            }
        }
        return events
    }

    /// Fold one event from the cua SDK's `SpacePresence.nextEvent` into the
    /// roster. `surfaceSize` is the streamed surface in pixels: SDK cursors are
    /// normalized, `CursorState.point` is in window pixels (pass 1x1 to keep
    /// the normalized value). Keep-alives and unknown kinds change nothing.
    public mutating func apply(event: CuaSDK.PresenceEvent,
                               surfaceSize: CGSize = CGSize(width: 1, height: 1)) -> [PresenceEvent] {
        var users = participants.values.map {
            PresenceUser(user_id: $0.id, name: $0.name, color: $0.color, kind: $0.kind,
                         principal_id: $0.principalID)
        }
        switch event.kind {
        case "joined":
            guard let p = event.participant else { return [] }
            users.removeAll { $0.user_id == p.participantId }
            users.append(PresenceUser(p))
            return apply(roster: users)
        case "left":
            guard let id = event.participantId else { return [] }
            users.removeAll { $0.user_id == id }
            return apply(roster: users)
        case "heartbeat":
            // Anyone the server no longer lists has left, even if its leave
            // was missed.
            guard let live = event.participantIds.map(Set.init) else { return [] }
            users.removeAll { $0.user_id != me?.id && !live.contains($0.user_id) }
            return apply(roster: users)
        case "shape_changed":
            guard let id = event.participantId, let name = event.shape else { return [] }
            if id == me?.id {
                myShape = name
                return []
            }
            guard var p = participants[id], var c = p.cursor else { return [] }
            c.shapeName = name
            c.shape = CursorShape.fromPresence(name)
            p.cursor = c
            participants[id] = p
            return [.cursorMoved(p)]
        case "cursor_moved":
            guard let id = event.participantId, let c = event.cursor else { return [] }
            let known = participants[id]
            let reported = c.shapeSource != "unspecified"
            let name = reported ? c.shape : (known?.cursor?.shapeName ?? "arrow")
            return apply(cursor: CursorState(
                userID: id, name: known?.name ?? "", color: known?.color ?? "",
                window: c.windowId.map(TargetHandle.init),
                point: CGPoint(x: c.x * surfaceSize.width, y: c.y * surfaceSize.height),
                isVisible: c.visible, isPressed: c.pressed,
                shape: reported ? CursorShape.fromPresence(name) : (known?.cursor?.shape ?? .unknown),
                normalized: CGPoint(x: c.x, y: c.y), shapeName: name))
        default:
            return []
        }
    }
}

extension SpacePresenceProtocol {
    /// Read events until one matches, folding each into `roster`. Bounded
    /// twice: `timeoutMs` in total and `maxEvents` events. Throws
    /// `StreamError.noStream` when neither bound finds a match.
    public func waitFor(timeoutMs: UInt64, maxEvents: Int = 50,
                        roster: inout PresenceRoster,
                        where matches: (CuaSDK.PresenceEvent) -> Bool) async throws -> CuaSDK.PresenceEvent {
        let deadline = ContinuousClock.now + .milliseconds(Int64(timeoutMs))
        for _ in 0..<max(0, maxEvents) {
            let left = deadline - ContinuousClock.now
            guard left > .zero else { break }
            let ms = Double(left.components.seconds) * 1_000
                + Double(left.components.attoseconds) / 1e15
            guard let event = try await nextEvent(timeoutMs: UInt64(max(1, ms))) else { break }
            _ = roster.apply(event: event)
            if matches(event) { return event }
        }
        throw StreamError.noStream("presence: no matching event within \(timeoutMs) ms / \(maxEvents) events")
    }

    /// `waitFor` without a roster.
    public func waitFor(timeoutMs: UInt64, maxEvents: Int = 50,
                        where matches: (CuaSDK.PresenceEvent) -> Bool) async throws -> CuaSDK.PresenceEvent {
        var scratch = PresenceRoster()
        return try await waitFor(timeoutMs: timeoutMs, maxEvents: maxEvents, roster: &scratch, where: matches)
    }
}

// MARK: - Send cadence

/// The ~30 Hz gate on outbound cursor updates.
///
/// The protocol does not rate-limit `cursor` at all, and the daemon's own host
/// watcher polls at 50 ms; a client that forwards raw `mouseMoved` sends at the
/// display's refresh rate for no visible benefit. Movement is throttled;
/// **state edges — press, release, hide — are never dropped**, because a
/// swallowed `visible: false` leaves a stranded arrow on every other viewer.
public struct CursorBroadcastGate: Sendable {
    public var interval: Duration
    private var lastSent: ContinuousClock.Instant?
    private var lastPressed = false
    private var lastVisible = false

    public init(interval: Duration = .milliseconds(33)) {
        self.interval = interval
    }

    /// Whether this update should go on the wire.
    public mutating func shouldSend(visible: Bool, pressed: Bool,
                                    now: ContinuousClock.Instant = ContinuousClock.now) -> Bool {
        defer {
            lastPressed = pressed
            lastVisible = visible
        }
        if pressed != lastPressed || visible != lastVisible {
            lastSent = now
            return true
        }
        guard let lastSent else {
            self.lastSent = now
            return true
        }
        guard now - lastSent >= interval else { return false }
        self.lastSent = now
        return true
    }
}

// MARK: - Stable agent colors

/// One color per agent for its cursor **and** its avatar, from the
/// Rust core (`presence_color`), so every SDK and both surfaces agree.
public enum PresenceColors {
    /// The stable presence color (`#rrggbb`) of an agent or other principal.
    public static func color(for id: String) -> String { presenceColor(id: id) }

    /// Black or white, whichever reads better on `background`.
    public static func textColor(on background: String) -> String {
        presenceTextColor(background: background)
    }

    /// An agent's presence identity, requesting its stable color.
    public static func agentIdentity(id: String, displayName: String) -> PresenceIdentity {
        PresenceIdentity(id: id, displayName: displayName, color: color(for: id), agent: true)
    }
}
