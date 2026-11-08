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
        /// The server is throttling this Mac (HTTP 429): too many attempts.
        case throttled
        /// No one is signed in at this Mac's screen (launchd's Aqua session).
        case guiSession
        /// The Cua account is signed out or its session expired.
        case signedOut
        /// macOS did not start the host service (launchd).
        case service
        /// The account already has computers: a new one is added from one
        /// of them, or with a pairing code (the relay's rule for an
        /// account with phone support).
        case addFromAnotherComputer
        /// The account already has other computers, and this one came with
        /// no proof that it is the owner adding it (the relay's
        /// `new_computer_needs_proof`): a code from one of them adds it.
        case needsProof
        case other

        /// The kind's name on the bridge (`HostFailure.kind` on the page).
        public var wireName: String {
            switch self {
            case .download: "download"
            case .network: "network"
            case .throttled: "throttled"
            case .guiSession: "guiSession"
            case .signedOut: "signedOut"
            case .service: "service"
            case .addFromAnotherComputer: "addFromAnotherComputer"
            case .needsProof: "needsProof"
            case .other: "other"
            }
        }
    }

    public let kind: Kind
    public let title: String
    public let message: String
    /// The raw error, as the setup reported it.
    public let details: String
    /// The relay's code for the refusal (`[code: …]` in the raw error), when
    /// it gave one: what the page matches on, whatever the words.
    public var refusalCode: String? { Self.refusalCode(details) }

    public static let retryLabel = "Retry"
    public static let retryingLabel = "Retrying\u{2026}"
    public static let detailsLabel = "Details"
    public static let copyLabel = "Copy"
    public static let signInLabel = "Sign In"
    public static let useCodeLabel = "Use a code from another computer"

    /// The relay's code `cua_host::refusal_code` reads: the raw error ends
    /// with `[code: <lowercase_word>]`.
    static func refusalCode(_ raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let open = trimmed.range(of: " [code: ", options: .backwards), trimmed.hasSuffix("]") else { return nil }
        let code = trimmed[open.upperBound..<trimmed.index(before: trimmed.endIndex)]
        guard !code.isEmpty, code.allSatisfy({ $0 == "_" || ("a"..."z").contains($0) }) else { return nil }
        return String(code)
    }

    /// The relay's code for a new computer that needs proof it is the owner
    /// adding it (`cua_host::NEW_COMPUTER_NEEDS_PROOF`).
    public static let newComputerNeedsProof = "new_computer_needs_proof"

    /// A failure whose kind the error itself says (the account refused,
    /// whatever its words), with the raw error as details.
    public static func presenting(_ raw: String, as kind: Kind) -> HostSetupFailure {
        let (title, message) = words(kind, raw: raw)
        return HostSetupFailure(kind: kind, title: title, message: message, details: raw)
    }

    /// The failure's button: Sign In when the account is what is missing
    /// (it signs in inline, then sets up), Retry otherwise.
    public var actionLabel: String {
        switch kind {
        case .signedOut: Self.signInLabel
        case .needsProof: Self.useCodeLabel
        default: Self.retryLabel
        }
    }

    /// Maps the raw setup error to what the sheet shows.
    public static func presenting(_ raw: String) -> HostSetupFailure {
        let kind = classify(raw)
        let (title, message) = words(kind, raw: raw)
        return HostSetupFailure(kind: kind, title: title, message: message, details: raw)
    }

    /// Seconds to wait when the raw error is the server telling this Mac to
    /// slow down: the host's own sentence, or a bare HTTP 429 "Too Many
    /// Requests" from an older host (`.some(nil)`, no number). `nil` when it
    /// is not that. A 429 that says why (a limit, a quota) is not a throttle.
    static func throttledWait(_ raw: String) -> Int?? {
        let own = #"too many attempts right now\. wait (\d+) seconds?"#
        if let r = raw.range(of: own, options: [.regularExpression, .caseInsensitive]) {
            let digits = raw[r].filter(\.isNumber)
            return .some(Int(digits))
        }
        let bare = #"\bhttp 429\b[:\s]*too many requests\s*$"#
        if raw.trimmingCharacters(in: .whitespacesAndNewlines)
            .range(of: bare, options: [.regularExpression, .caseInsensitive]) != nil {
            return .some(nil)
        }
        return nil
    }

    /// Seconds before the app tries again by itself, when the server is
    /// throttling this Mac and said how long to wait.
    public var throttleSeconds: Int? {
        kind == .throttled ? (Self.throttledWait(details) ?? nil) : nil
    }

    /// The line once the one automatic retry is used up.
    static func manualWait(_ f: HostSetupFailure) -> String {
        guard let s = f.throttleSeconds else { return f.message }
        return "Wait \(s) \(s == 1 ? "second" : "seconds"), then try again."
    }

    static func classify(_ raw: String) -> Kind {
        // The relay's own code first: it says which refusal it is, whatever
        // the words around it.
        if refusalCode(raw) == newComputerNeedsProof { return .needsProof }
        if throttledWait(raw) != nil { return .throttled }
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

        if any(["add computers from a computer you already added"]) { return .addFromAnotherComputer }

        if word("401") || any([
            "signed out", "not signed in", "sign in again", "please sign in", "unauthenticated", "unauthorized",
            "login required", "not logged in", "session expired", "token expired", "invalid token",
            "no account token", "missing account token",
        ]) { return .signedOut }

        if any(["download:", "download failed"]) { return .download }
        return .other
    }

    static func words(_ kind: Kind, raw: String = "") -> (String, String) {
        switch kind {
        case .download:
            return ("Couldn\u{2019}t set up sharing for this Mac",
                    "Check your internet connection and try again.")
        case .network:
            return ("Couldn\u{2019}t connect",
                    "Cua Spaces couldn\u{2019}t reach the internet. Check your connection and try again.")
        case .throttled:
            let seconds = throttledWait(raw) ?? nil
            let wait = seconds.map { "Trying again in \($0) \($0 == 1 ? "second" : "seconds")." }
                ?? "Wait a few seconds, then try again."
            return ("Too many attempts right now", wait)
        case .guiSession:
            return ("Sign in to this Mac first",
                    "Setting up access needs someone signed in at this Mac\u{2019}s own screen, not only over "
                        + "SSH or Screen Sharing. Sign in at the Mac, then try again.")
        case .signedOut:
            return ("Sign in to Cua",
                    "Your Cua account isn\u{2019}t signed in on this Mac. Sign in, then try again.")
        case .service:
            return ("Couldn\u{2019}t start the Cua host service",
                    "macOS didn\u{2019}t start the service your other machines connect to. Try again; "
                        + "if it keeps failing, restart this Mac.")
        case .addFromAnotherComputer:
            return ("Add this Mac from another computer",
                    "Your account already has computers, so a new Mac is added from one of them or with a pairing code. "
                        + "Open Cua Spaces on one of your computers and add this Mac from there.")
        case .needsProof:
            return ("Confirm it\u{2019}s you",
                    "To add this Mac to your account, enter a code from a computer you already added. "
                        + "On that computer, open Cua Spaces, go to Computers and click Add a computer.")
        case .other:
            return ("Couldn\u{2019}t set up this Mac for access",
                    "Something went wrong. Try again, or open Details to see what happened.")
        }
    }
}
