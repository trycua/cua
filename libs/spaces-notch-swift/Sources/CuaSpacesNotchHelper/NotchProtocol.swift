// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesNotchUI
import Foundation

/// The notch protocol between the Electron app and this helper: one JSON
/// object per line on the helper's stdin (from Electron) and stdout (to
/// Electron), each with a `type`. The helper exits when stdin closes (the
/// app quit or crashed), so it never outlives the app.
///
/// Electron to helper:
///  * `hello` `{v, motion, radii}`: the protocol version and the core's
///    motion and radii. First, once.
///  * `state` `{view, query, layout?, shown, dragging, icons, highlight?}`:
///    the core's notch view, the search text, the layout for the notch
///    screen (none until the helper reported its screens), whether the
///    notch shows (the setting), whether a window drag runs, the OS icons
///    the tiles use, a forced look (debug).
///  * `thumbnail` `{id, image?}`: a Space's latest thumbnail (base64 PNG or
///    JPEG; none clears it).
///  * `ghost` `{image?}`: the dragged window's image.
///  * `quit`: exit now.
///
/// Helper to Electron:
///  * `hello` `{v, pid}`: once, at start.
///  * `screens` `{notch?, primary?}`: the notch screen (the notched display,
///    else the main one) and the primary display, at start and on every
///    display change.
///  * `event` `{event}`: hover, click, Escape, dismiss, a file drag over the
///    notch, the search text (`NotchData.Event`), for the core's reducer.
///  * `action` `{action, spaceId?, pane?, paths?}`: `openSpace`, `openMain`,
///    `openSettings`, `openAccess`, `dismissAccess`,
///    `openPermissionSettings`, `drop`.
///  * `stage` `{open}`: the panel finished opening or closing (Electron
///    keeps the tiles' thumbnails fresh while open).
///  * `error` `{message}`: why the helper is about to exit.
public enum NotchProtocol {
    /// Bumped on any incompatible change; both sides refuse another.
    public static let version = 1
    /// Exit status for a version mismatch (Electron does not restart it).
    public static let mismatchExit: Int32 = 3
    /// The longest line read (a thumbnail is well under this).
    public static let maxLine = 32 * 1024 * 1024
}

/// A message from Electron.
public enum HostMessage: Equatable {
    case hello(version: Int, motion: NotchData.Motion, radii: NotchData.RadiiPair)
    case state(HostState)
    case thumbnail(spaceId: String, image: Data?)
    case ghost(image: Data?)
    case quit
}

/// The `state` message.
public struct HostState: Equatable, Decodable {
    public var view: NotchData.View
    public var query: String
    public var layout: NotchData.Layout?
    public var shown: Bool
    public var dragging: Bool
    public var icons: [String: NotchData.OsIcon]
    public var highlight: String?

    public init(view: NotchData.View, query: String = "", layout: NotchData.Layout? = nil, shown: Bool = true,
                dragging: Bool = false, icons: [String: NotchData.OsIcon] = [:], highlight: String? = nil) {
        self.view = view
        self.query = query
        self.layout = layout
        self.shown = shown
        self.dragging = dragging
        self.icons = icons
        self.highlight = highlight
    }

    private enum Key: String, CodingKey { case view, query, layout, shown, dragging, icons, highlight }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        view = try c.decode(NotchData.View.self, forKey: .view)
        query = try c.decodeIfPresent(String.self, forKey: .query) ?? ""
        layout = try c.decodeIfPresent(NotchData.Layout.self, forKey: .layout)
        shown = try c.decodeIfPresent(Bool.self, forKey: .shown) ?? true
        dragging = try c.decodeIfPresent(Bool.self, forKey: .dragging) ?? false
        icons = try c.decodeIfPresent([String: NotchData.OsIcon].self, forKey: .icons) ?? [:]
        highlight = try c.decodeIfPresent(String.self, forKey: .highlight)
    }
}

extension HostMessage: Decodable {
    private enum Key: String, CodingKey { case type, v, motion, radii, id, image }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        switch try c.decode(String.self, forKey: .type) {
        case "hello":
            self = .hello(version: try c.decode(Int.self, forKey: .v),
                          motion: try c.decode(NotchData.Motion.self, forKey: .motion),
                          radii: try c.decode(NotchData.RadiiPair.self, forKey: .radii))
        case "state":
            self = .state(try HostState(from: decoder))
        case "thumbnail":
            self = .thumbnail(spaceId: try c.decode(String.self, forKey: .id),
                              image: try Self.image(c))
        case "ghost":
            self = .ghost(image: try Self.image(c))
        case "quit":
            self = .quit
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "unknown message \(other)")
        }
    }

    private static func image(_ c: KeyedDecodingContainer<Key>) throws -> Data? {
        guard let b64 = try c.decodeIfPresent(String.self, forKey: .image) else { return nil }
        guard let data = Data(base64Encoded: b64) else {
            throw DecodingError.dataCorruptedError(forKey: .image, in: c, debugDescription: "image is not base64")
        }
        return data
    }

    /// Decodes one line (without its newline).
    public static func decode(_ line: Data) throws -> HostMessage {
        try JSONDecoder().decode(HostMessage.self, from: line)
    }
}

/// What the helper asks Electron to do.
public enum HelperAction: Equatable {
    case openSpace(spaceId: String)
    case openMain
    case openSettings
    case openAccess
    case dismissAccess
    case openPermissionSettings(pane: String)
    case drop(spaceId: String, paths: [String])
}

/// A message to Electron.
public enum HelperMessage: Equatable {
    case hello(version: Int, pid: Int32)
    case screens(notch: NotchData.ScreenFacts?, primary: NotchData.ScreenFacts?)
    case event(NotchData.Event)
    case action(HelperAction)
    case stage(open: Bool)
    case error(message: String)
}

extension HelperMessage: Encodable {
    private enum Key: String, CodingKey { case type, v, pid, notch, primary, event, action, spaceId, pane, paths, open, message }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        switch self {
        case .hello(let version, let pid):
            try c.encode("hello", forKey: .type)
            try c.encode(version, forKey: .v)
            try c.encode(pid, forKey: .pid)
        case .screens(let notch, let primary):
            try c.encode("screens", forKey: .type)
            try c.encodeIfPresent(notch, forKey: .notch)
            try c.encodeIfPresent(primary, forKey: .primary)
        case .event(let event):
            try c.encode("event", forKey: .type)
            try c.encode(event, forKey: .event)
        case .action(let action):
            try c.encode("action", forKey: .type)
            switch action {
            case .openSpace(let id):
                try c.encode("openSpace", forKey: .action)
                try c.encode(id, forKey: .spaceId)
            case .openMain: try c.encode("openMain", forKey: .action)
            case .openSettings: try c.encode("openSettings", forKey: .action)
            case .openAccess: try c.encode("openAccess", forKey: .action)
            case .dismissAccess: try c.encode("dismissAccess", forKey: .action)
            case .openPermissionSettings(let pane):
                try c.encode("openPermissionSettings", forKey: .action)
                try c.encode(pane, forKey: .pane)
            case .drop(let id, let paths):
                try c.encode("drop", forKey: .action)
                try c.encode(id, forKey: .spaceId)
                try c.encode(paths, forKey: .paths)
            }
        case .stage(let open):
            try c.encode("stage", forKey: .type)
            try c.encode(open, forKey: .open)
        case .error(let message):
            try c.encode("error", forKey: .type)
            try c.encode(message, forKey: .message)
        }
    }

    /// One line, newline included.
    public func line() -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        // Every case encodes (plain values only).
        var data = (try? encoder.encode(self)) ?? Data()
        data.append(0x0A)
        return data
    }
}

/// Splits a byte stream into lines, bounded: a line longer than
/// `NotchProtocol.maxLine` is dropped whole.
public struct LineSplitter {
    private var buffer = Data()
    private var skipping = false

    public init() {}

    /// The complete lines in `chunk` (and before it), without newlines.
    public mutating func push(_ chunk: Data) -> [Data] {
        var lines: [Data] = []
        var rest = chunk[...]
        while let nl = rest.firstIndex(of: 0x0A) {
            if skipping {
                skipping = false
            } else {
                buffer.append(rest[rest.startIndex..<nl])
                if !buffer.isEmpty { lines.append(buffer) }
            }
            buffer = Data()
            rest = rest[(nl + 1)...]
        }
        if !skipping { buffer.append(rest) }
        if buffer.count > NotchProtocol.maxLine {
            buffer = Data()
            skipping = true
        }
        return lines
    }
}
