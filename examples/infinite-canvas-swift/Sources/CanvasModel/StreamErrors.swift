// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// A stream failure as a tile shows it: a short headline, then a short
/// human detail. The raw error stays available (tooltip) but is never the
/// headline.
public struct StreamErrorMessage: Equatable, Sendable {
    public var headline: String
    public var detail: String
    public var raw: String

    /// Map the SDK's error text to something a person can act on.
    public static func describe(_ raw: String) -> StreamErrorMessage {
        let s = raw.lowercased()
        func m(_ d: String) -> StreamErrorMessage { StreamErrorMessage(headline: "Stream unavailable", detail: d, raw: raw) }
        if s.contains("stale_target") || s.contains("target closed") || s.contains("window is minimized")
            || s.contains("no such window") || s.contains("not_found") {
            return m("The window closed or is hidden.")
        }
        if s.contains("unauthenticated") || s.contains("401") || s.contains("4401") || s.contains("ticket")
            || s.contains("permission denied") || s.contains("token") {
            return m("The Space refused the token.")
        }
        if s.contains("connection refused") || s.contains("not available") || s.contains("nothing answered")
            || s.contains("timed out") || s.contains("timeout") || s.contains("unreachable") {
            return m("The Space is not reachable.")
        }
        if s.contains("stream no longer needed") || s.contains("transport error") || s.contains("h2 protocol")
            || s.contains("broken pipe") || s.contains("reset") || s.contains("connection closed") {
            return m("The Space closed the stream.")
        }
        if s.contains("codec") || s.contains("h264") {
            return m("The Space offers no H.264 stream.")
        }
        if s.contains("screen recording") || s.contains("capture") {
            return m("The Space cannot capture this window.")
        }
        let firstLine = raw.split(whereSeparator: \.isNewline).first.map(String.init) ?? raw
        return m(firstLine.count > 120 ? String(firstLine.prefix(117)) + "…" : firstLine)
    }
}
