import Foundation

public struct WindowIdentityMetadata: Sendable, Codable, Hashable {
    public let windowUID: String
    public let generation: Int
    public let firstSeen: String
    public let lastSeen: String

    public init(
        windowUID: String,
        generation: Int,
        firstSeen: String,
        lastSeen: String
    ) {
        self.windowUID = windowUID
        self.generation = generation
        self.firstSeen = firstSeen
        self.lastSeen = lastSeen
    }

    private enum CodingKeys: String, CodingKey {
        case windowUID = "window_uid"
        case generation
        case firstSeen = "first_seen"
        case lastSeen = "last_seen"
    }
}

public actor WindowIdentityStore {
    public static let shared = WindowIdentityStore()

    private struct Record {
        let fingerprint: String
        let generation: Int
        let firstSeen: String
        var lastSeen: String

        var uid: String {
            "cgwindow:\(Self.windowId(from: fingerprint)):pid:\(Self.pid(from: fingerprint)):gen:\(generation)"
        }

        private static func windowId(from fingerprint: String) -> String {
            fingerprint.split(separator: "|").first.map(String.init) ?? "unknown"
        }

        private static func pid(from fingerprint: String) -> String {
            let parts = fingerprint.split(separator: "|")
            guard parts.count > 1 else { return "unknown" }
            return String(parts[1])
        }
    }

    private var records: [Int: Record] = [:]

    public init() {}

    public func metadata(for windows: [WindowInfo]) -> [Int: WindowIdentityMetadata] {
        let now = Self.timestamp()
        var out: [Int: WindowIdentityMetadata] = [:]
        for window in windows {
            out[window.id] = metadata(for: window, observedAt: now)
        }
        return out
    }

    public func metadata(for window: WindowInfo) -> WindowIdentityMetadata {
        metadata(for: window, observedAt: Self.timestamp())
    }

    private func metadata(
        for window: WindowInfo,
        observedAt now: String
    ) -> WindowIdentityMetadata {
        let fingerprint = Self.fingerprint(for: window)
        let record: Record
        if var existing = records[window.id],
           existing.fingerprint == fingerprint
        {
            existing.lastSeen = now
            record = existing
        } else {
            let nextGeneration = (records[window.id]?.generation ?? 0) + 1
            record = Record(
                fingerprint: fingerprint,
                generation: nextGeneration,
                firstSeen: now,
                lastSeen: now
            )
        }
        records[window.id] = record
        return WindowIdentityMetadata(
            windowUID: record.uid,
            generation: record.generation,
            firstSeen: record.firstSeen,
            lastSeen: record.lastSeen
        )
    }

    private static func fingerprint(for window: WindowInfo) -> String {
        // Deliberately excludes title and bounds. Titles and bounds change
        // during normal work; a uid should remain stable for the OS window.
        "\(window.id)|\(window.pid)|\(window.owner)|\(window.layer)"
    }

    private static func timestamp() -> String {
        ISO8601DateFormatter().string(from: Date())
    }
}
