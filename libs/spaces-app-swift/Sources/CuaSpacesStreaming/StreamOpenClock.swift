// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import os

/// Where the time of one stream open goes: the `stream.open` span.
///
/// Each hop stamps the clock once (`ProcessInfo.systemUptime`, s). The span
/// is logged at debug level in the unified log (subsystem `ai.cua.spaces`,
/// category `stream.timing`) when the first frame is painted, and read back
/// by tests and the bench. It carries times only, never frame content.
///
///   slot       the page's slot asked for the stream (the click)
///   resolved   the Space's handle and stream source are ready
///   begun      the session starts opening its transport
///   presence   presence joined (or skipped)
///   opened     the Space answered OpenMedia and the media socket is attached
///   decoded    the first frame is decoded
///   painted    the first frame reached the layer
public struct StreamOpenClock: Equatable, Sendable {
    public enum Hop: String, CaseIterable, Sendable {
        case slot, resolved, begun, presence, opened, decoded, painted
    }

    public private(set) var marks: [Hop: TimeInterval] = [:]

    public init() {}

    /// Stamps `hop` once; a later stamp of the same hop is ignored.
    public mutating func mark(_ hop: Hop, at time: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        if marks[hop] == nil { marks[hop] = time }
    }

    /// Milliseconds from `from` (or the first stamp) to `hop`, nil if either is missing.
    public func ms(to hop: Hop, from: Hop? = nil) -> Double? {
        guard let end = marks[hop] else { return nil }
        let start = from.flatMap { marks[$0] } ?? marks.values.min()
        return start.map { max(0, (end - $0) * 1000) }
    }

    /// The span as one line: hop offsets from the first stamp, in ms.
    public var summary: String {
        let start = marks.values.min() ?? 0
        return Hop.allCases.compactMap { hop in
            marks[hop].map { "\(hop.rawValue)=\(Int(((($0 - start) * 1000).rounded())))" }
        }.joined(separator: " ")
    }

    private static let log = Logger(subsystem: "ai.cua.spaces", category: "stream.timing")

    /// Writes the span to the unified log (debug level).
    public func report(space: String, tier: String, note: String = "") {
        let line = summary
        Self.log.debug("stream.open space=\(space, privacy: .public) tier=\(tier, privacy: .public) \(line, privacy: .public) \(note, privacy: .public)")
    }
}
