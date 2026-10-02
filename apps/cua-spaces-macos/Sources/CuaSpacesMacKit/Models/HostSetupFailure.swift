// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// A failed "Set up for access", in words a person can act on: a short
/// title, one plain message (never a URL or a raw error), and the raw error
/// kept unchanged for the Details disclosure.
public struct HostSetupFailure: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// The host service (cua-spacesd) could not be downloaded.
        case download
        /// No network, a timeout, or a server that could not be reached.
        case network
        /// No one is signed in at this Mac's screen (launchd's Aqua session).
        case guiSession
        /// The Cua account is signed out or its session expired.
        case signedOut
        /// macOS did not start the host service (launchd).
        case service
        case other
    }

    public let kind: Kind
    public let title: String
    public let message: String
    /// The raw error, as the setup reported it.
    public let details: String

    public static let retryLabel = "Retry"
    public static let retryingLabel = "Retrying\u{2026}"
    public static let detailsLabel = "Details"
    public static let copyLabel = "Copy"

    /// Maps the raw setup error to what the sheet shows.
    public static func presenting(_ raw: String) -> HostSetupFailure {
        let kind = classify(raw)
        let (title, message) = words(kind)
        return HostSetupFailure(kind: kind, title: title, message: message, details: raw)
    }

    static func classify(_ raw: String) -> Kind {
        let s = raw.lowercased()
        func any(_ needles: [String]) -> Bool { needles.contains { s.contains($0) } }
        func word(_ pattern: String) -> Bool {
            s.range(of: "\\b\(pattern)\\b", options: .regularExpression) != nil
        }

        let downloading = any(["download:", "download failed", "cua-spacesd"])
        let unavailable = word("404") || any(["not found", "no published", "not available", "unavailable"])
        if downloading && unavailable { return .download }

        if any([
            "timed out", "timeout", "offline", "not connected to the internet", "network is unreachable",
            "network unreachable", "connection refused", "connection reset", "connection closed",
            "could not resolve", "couldn't resolve", "failed to lookup", "dns error", "dns lookup",
            "error sending request", "host is unreachable", "no route to host", "tls handshake",
        ]) { return .network }

        if any(["aqua", "no gui session", "gui session", "not logged in at the console", "console user",
                "no console"]) { return .guiSession }

        if any(["launchctl", "launchd", "host service", "bootstrap failed"]) { return .service }

        if word("401") || any([
            "signed out", "not signed in", "sign in again", "please sign in", "unauthenticated", "unauthorized",
            "login required", "not logged in", "session expired", "token expired", "invalid token",
            "no account token", "missing account token",
        ]) { return .signedOut }

        if any(["download:", "download failed"]) { return .download }
        return .other
    }

    static func words(_ kind: Kind) -> (String, String) {
        switch kind {
        case .download:
            return ("Couldn\u{2019}t download the Cua host service",
                    "The service this Mac needs to accept connections isn\u{2019}t available right now. "
                        + "Check your internet connection and try again; if it keeps failing, update Cua Spaces.")
        case .network:
            return ("Couldn\u{2019}t connect",
                    "Cua Spaces couldn\u{2019}t reach the internet. Check your connection and try again.")
        case .guiSession:
            return ("Sign in to this Mac first",
                    "Setting up access needs someone signed in at this Mac\u{2019}s own screen, not only over "
                        + "SSH or Screen Sharing. Sign in at the Mac, then try again.")
        case .signedOut:
            return ("Sign in to Cua",
                    "Your Cua account isn\u{2019}t signed in on this Mac. Sign in, then try again.")
        case .service:
            return ("Couldn\u{2019}t start the Cua host service",
                    "macOS didn\u{2019}t start the service your other devices connect to. Try again; "
                        + "if it keeps failing, restart this Mac.")
        case .other:
            return ("Couldn\u{2019}t set up this Mac for access",
                    "Something went wrong. Try again, or open Details to see what happened.")
        }
    }
}
