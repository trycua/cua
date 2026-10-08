// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What the notch views draw, as plain data: the app core's notch records
/// (`AppNotchView`, `AppNotchLayout`, `AppNotchMotion`, ...) field for field,
/// with the same names. The SwiftUI app maps the core's values onto these;
/// the helper decodes them from the notch protocol (JSON, the same field
/// names; enums as their case names).
public enum NotchData {
    /// A rectangle in AppKit points (bottom-left origin, global).
    public struct Rect: Codable, Equatable, Hashable, Sendable {
        public var x: Double
        public var y: Double
        public var width: Double
        public var height: Double

        public init(x: Double, y: Double, width: Double, height: Double) {
            self.x = x
            self.y = y
            self.width = width
            self.height = height
        }
    }

    /// What shows: the closed notch, the Space tiles, or the "Teleport to
    /// Cua" box during a window drag.
    public enum Phase: String, Codable, Equatable, Hashable, Sendable {
        case closed, tiles, prompt
    }

    /// The indicator left of the closed notch.
    public enum ActivityKind: String, Codable, Equatable, Hashable, Sendable {
        case transfer, remoteAccess, hotspot, provisioning, deleting, keyvault
    }

    /// The header's buttons.
    public enum ButtonId: String, Codable, Equatable, Hashable, Sendable {
        case list, settings
    }

    /// A tile's status (its dot).
    public enum TileStatus: String, Codable, Equatable, Hashable, Sendable {
        case local, running, approval, suspended, provisioning, deleting
    }

    /// The tab's two rows: the count over the word.
    public struct Tab: Codable, Equatable, Hashable, Sendable {
        public var count: String
        public var word: String

        public init(count: String, word: String) {
            self.count = count
            self.word = word
        }
    }

    /// The indicator: a symbol, or a progress ring (real progress in
    /// thousandths, else the core's estimate from `startedAt`).
    public struct Activity: Codable, Equatable, Hashable, Sendable {
        public var kind: ActivityKind
        public var label: String
        public var symbol: String?
        public var permille: UInt32?
        /// Unix milliseconds.
        public var startedAt: Int64?
        public var estimateMs: UInt32

        public init(kind: ActivityKind, label: String, symbol: String?, permille: UInt32?,
                    startedAt: Int64?, estimateMs: UInt32) {
            self.kind = kind
            self.label = label
            self.symbol = symbol
            self.permille = permille
            self.startedAt = startedAt
            self.estimateMs = estimateMs
        }
    }

    public struct Button: Codable, Equatable, Hashable, Sendable {
        public var id: ButtonId
        public var symbol: String
        public var label: String
        public var help: String

        public init(id: ButtonId, symbol: String, label: String, help: String) {
            self.id = id
            self.symbol = symbol
            self.label = label
            self.help = help
        }
    }

    /// The row inline with the notch while open: the search and the buttons.
    public struct Header: Codable, Equatable, Hashable, Sendable {
        public var query: String
        public var placeholder: String
        public var searchLabel: String
        public var matchCount: UInt32?
        public var buttons: [Button]

        public init(query: String, placeholder: String, searchLabel: String, matchCount: UInt32?, buttons: [Button]) {
            self.query = query
            self.placeholder = placeholder
            self.searchLabel = searchLabel
            self.matchCount = matchCount
            self.buttons = buttons
        }
    }

    /// The line asking for the window-drag permission.
    public struct Permission: Codable, Equatable, Hashable, Sendable {
        public var text: String
        public var action: String
        public var pane: String

        public init(text: String, action: String, pane: String) {
            self.text = text
            self.action = action
            self.pane = pane
        }
    }

    /// The live Keyvault sign-ins line.
    public struct Access: Codable, Equatable, Hashable, Sendable {
        public var text: String
        public var dismiss: String

        public init(text: String, dismiss: String) {
            self.text = text
            self.dismiss = dismiss
        }
    }

    /// A Space tile.
    public struct Tile: Codable, Equatable, Hashable, Sendable {
        public var id: String
        public var name: String
        public var status: TileStatus
        public var dim: Bool
        public var dropTarget: Bool
        public var targeted: Bool
        /// The OS icon id (`NotchOsIcon`).
        public var symbol: String
        public var label: String
        /// Where it runs ("This Mac", a machine's name).
        public var location: String
        public var progress: UInt32?
        public var progressLabel: String?
        public var signedIn: Bool

        public init(id: String, name: String, status: TileStatus, dim: Bool, dropTarget: Bool, targeted: Bool,
                    symbol: String, label: String, location: String, progress: UInt32?, progressLabel: String?,
                    signedIn: Bool) {
            self.id = id
            self.name = name
            self.status = status
            self.dim = dim
            self.dropTarget = dropTarget
            self.targeted = targeted
            self.symbol = symbol
            self.label = label
            self.location = location
            self.progress = progress
            self.progressLabel = progressLabel
            self.signedIn = signedIn
        }
    }

    /// The core's notch view (`appNotchView`).
    public struct View: Codable, Equatable, Hashable, Sendable {
        public var phase: Phase
        public var tiles: [Tile]
        public var dropMode: Bool
        public var prompt: String?
        public var label: String
        public var countLabel: String
        public var tab: Tab
        public var header: Header?
        public var empty: String?
        public var activity: Activity?
        public var hidden: Bool
        public var showTab: Bool
        public var hoverCue: Bool
        public var permission: Permission?
        public var access: Access?

        public init(phase: Phase, tiles: [Tile], dropMode: Bool, prompt: String?, label: String, countLabel: String,
                    tab: Tab, header: Header?, empty: String?, activity: Activity?, hidden: Bool, showTab: Bool,
                    hoverCue: Bool, permission: Permission?, access: Access?) {
            self.phase = phase
            self.tiles = tiles
            self.dropMode = dropMode
            self.prompt = prompt
            self.label = label
            self.countLabel = countLabel
            self.tab = tab
            self.header = header
            self.empty = empty
            self.activity = activity
            self.hidden = hidden
            self.showTab = showTab
            self.hoverCue = hoverCue
            self.permission = permission
            self.access = access
        }

        /// The closed notch with nothing in it (before the first state).
        public static let empty = View(
            phase: .closed, tiles: [], dropMode: false, prompt: nil, label: "Cua Spaces", countLabel: "",
            tab: Tab(count: "", word: ""), header: nil, empty: nil, activity: nil, hidden: true, showTab: false,
            hoverCue: false, permission: nil, access: nil)
    }

    /// The core's layout for one screen (`appNotchLayout`), global AppKit
    /// points.
    public struct Layout: Codable, Equatable, Hashable, Sendable {
        public var hasNotch: Bool
        public var notch: Rect
        public var closedFrame: Rect
        public var openFrame: Rect
        public var promptFrame: Rect
        public var tabFrame: Rect
        public var tabInsetNotch: Double
        public var tabInsetOuter: Double
        public var stageFrame: Rect
        public var notchStyle: Bool

        public init(hasNotch: Bool, notch: Rect, closedFrame: Rect, openFrame: Rect, promptFrame: Rect, tabFrame: Rect,
                    tabInsetNotch: Double, tabInsetOuter: Double, stageFrame: Rect, notchStyle: Bool) {
            self.hasNotch = hasNotch
            self.notch = notch
            self.closedFrame = closedFrame
            self.openFrame = openFrame
            self.promptFrame = promptFrame
            self.tabFrame = tabFrame
            self.tabInsetNotch = tabInsetNotch
            self.tabInsetOuter = tabInsetOuter
            self.stageFrame = stageFrame
            self.notchStyle = notchStyle
        }
    }

    /// The core's shared motion (`appNotchMotion`).
    public struct Motion: Codable, Equatable, Hashable, Sendable {
        public var hoverDwellMs: UInt32
        public var closeDelayMs: UInt32
        public var openResponse: Double
        public var openDamping: Double
        public var closeResponse: Double
        public var closeDamping: Double
        public var reducedDuration: Double
        public var hoverResponse: Double
        public var hoverDamping: Double
        public var hoverScale: Double
        public var hoverScaleY: Double
        public var contentDelayMs: UInt32
        public var contentIn: Double
        public var contentOut: Double
        public var contentScale: Double

        public init(hoverDwellMs: UInt32, closeDelayMs: UInt32, openResponse: Double, openDamping: Double,
                    closeResponse: Double, closeDamping: Double, reducedDuration: Double, hoverResponse: Double,
                    hoverDamping: Double, hoverScale: Double, hoverScaleY: Double, contentDelayMs: UInt32,
                    contentIn: Double, contentOut: Double, contentScale: Double) {
            self.hoverDwellMs = hoverDwellMs
            self.closeDelayMs = closeDelayMs
            self.openResponse = openResponse
            self.openDamping = openDamping
            self.closeResponse = closeResponse
            self.closeDamping = closeDamping
            self.reducedDuration = reducedDuration
            self.hoverResponse = hoverResponse
            self.hoverDamping = hoverDamping
            self.hoverScale = hoverScale
            self.hoverScaleY = hoverScaleY
            self.contentDelayMs = contentDelayMs
            self.contentIn = contentIn
            self.contentOut = contentOut
            self.contentScale = contentScale
        }
    }

    /// The shape's corner radii.
    public struct Radii: Codable, Equatable, Hashable, Sendable {
        public var top: Double
        public var bottom: Double

        public init(top: Double, bottom: Double) {
            self.top = top
            self.bottom = bottom
        }
    }

    /// The closed and open radii (`appNotchRadii`, in that order).
    public struct RadiiPair: Codable, Equatable, Hashable, Sendable {
        public var closed: Radii
        public var open: Radii

        public init(closed: Radii, open: Radii) {
            self.closed = closed
            self.open = open
        }
    }

    /// One screen, as the core's layout reads it (`AppScreenFacts`).
    public struct ScreenFacts: Codable, Equatable, Hashable, Sendable {
        public var frame: Rect
        public var visibleFrame: Rect
        public var safeAreaTop: Double
        public var auxLeftWidth: Double?
        public var auxRightWidth: Double?

        public init(frame: Rect, visibleFrame: Rect, safeAreaTop: Double, auxLeftWidth: Double?, auxRightWidth: Double?) {
            self.frame = frame
            self.visibleFrame = visibleFrame
            self.safeAreaTop = safeAreaTop
            self.auxLeftWidth = auxLeftWidth
            self.auxRightWidth = auxRightWidth
        }
    }

    /// An OS icon: the system symbol when the core names one, else its SVG.
    public struct OsIcon: Codable, Equatable, Hashable, Sendable {
        public var symbol: String?
        public var svg: String?

        public init(symbol: String?, svg: String?) {
            self.symbol = symbol
            self.svg = svg
        }
    }

    /// What the views send to the core's reducer (the subset of
    /// `AppNotchEvent` that comes from the panel itself).
    public enum Event: Equatable, Hashable, Sendable {
        case hoverEnter
        case hoverExit
        case click
        case dismiss
        case escape
        case dropTargeted(targeted: Bool)
        case search(query: String)
    }
}

extension NotchData.Event: Codable {
    private enum Key: String, CodingKey { case kind, targeted, query }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "hoverEnter": self = .hoverEnter
        case "hoverExit": self = .hoverExit
        case "click": self = .click
        case "dismiss": self = .dismiss
        case "escape": self = .escape
        case "dropTargeted": self = .dropTargeted(targeted: try c.decode(Bool.self, forKey: .targeted))
        case "search": self = .search(query: try c.decode(String.self, forKey: .query))
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown event \(other)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        switch self {
        case .hoverEnter: try c.encode("hoverEnter", forKey: .kind)
        case .hoverExit: try c.encode("hoverExit", forKey: .kind)
        case .click: try c.encode("click", forKey: .kind)
        case .dismiss: try c.encode("dismiss", forKey: .kind)
        case .escape: try c.encode("escape", forKey: .kind)
        case .dropTargeted(let targeted):
            try c.encode("dropTargeted", forKey: .kind)
            try c.encode(targeted, forKey: .targeted)
        case .search(let query):
            try c.encode("search", forKey: .kind)
            try c.encode(query, forKey: .query)
        }
    }
}
