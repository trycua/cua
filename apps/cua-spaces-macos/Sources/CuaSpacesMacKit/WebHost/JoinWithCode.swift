// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// "Join with a code": the pairing code a machine joins with, as the
/// `host.joinWithCode` bridge method and the `cua://add/<CODE>` link carry it
/// (`cua-local://add/<CODE>` in Cua Spaces Local: the app answers only to its
/// own scheme, `AppURLScheme`).
/// Pure parts only; the Process runs in `WebUIBridge+Pages.swift`.
enum JoinWithCode {
    /// Crockford base32, the pairing code's alphabet.
    static let alphabet = Set("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
    static let codeLength = 12
    /// How long `cua host setup --pair` may run.
    static let timeout: TimeInterval = 180

    /// The code as `XXXX-XXXX-XXXX`, from any case, with dashes and spaces
    /// ignored; `nil` unless it is 12 Crockford base32 characters.
    static func normalizeCode(_ raw: String) -> String? {
        let chars = raw.uppercased().filter { $0 != "-" && !$0.isWhitespace }
        guard chars.count == codeLength, chars.allSatisfy({ alphabet.contains($0) }) else { return nil }
        let s = Array(chars)
        return [s[0..<4], s[4..<8], s[8..<12]].map { String($0) }.joined(separator: "-")
    }

    /// A code or an add-machine link (`https://cua.ai/add/<code>?relay=...`)
    /// as the user pasted it: the trimmed text and the link's relay, if any.
    /// Other text passes through, for the CLI to judge.
    static func splitInput(code raw: String, relay: String?) -> (code: String, relay: String?) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        var relay = relay?.trimmingCharacters(in: .whitespacesAndNewlines)
        if relay?.isEmpty == true { relay = nil }
        if let url = URL(string: text), let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http" || scheme == "cua" {
            let parts = url.pathComponents.filter { $0 != "/" }
            var found: String?
            if let i = parts.firstIndex(of: "add"), i + 1 < parts.count { found = parts[i + 1] }
            let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
            if found == nil { found = items.first { $0.name == "code" }?.value }
            if relay == nil, let r = items.first(where: { $0.name == "relay" })?.value, !r.isEmpty { relay = r }
            if let found { return (found, relay) }
        }
        return (text, relay)
    }

    /// The arguments after `cua`; `nil` when there is no code.
    static func arguments(code raw: String, relay: String?) -> [String]? {
        let (text, relay) = splitInput(code: raw, relay: relay)
        guard !text.isEmpty else { return nil }
        let code = normalizeCode(text) ?? text
        var args = ["host", "setup", "--pair", code]
        if let relay { args += ["--relay", relay] }
        // It provides Spaces and keeps its screen private, as a computer
        // added by signing in does; sharing its desktop is a choice on its page.
        return args + ["--no-desktop", "--provide-spaces"]
    }

    /// The last non-empty line of the CLI's stderr, for the error message.
    static func failureMessage(stderr: String, status: Int32) -> String {
        let line = stderr.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { !$0.isEmpty }
        return line ?? "cua host setup exited with status \(status)"
    }

    /// What `cua://add/<CODE>`, `cua://add/<CODE>?relay=<url>` and
    /// `cua://add?code=<CODE>` ask for, in this app's scheme (`cua`, or
    /// `cua-local` in Cua Spaces Local): the page's route, or `nil` for any
    /// other link (another app's scheme included) or a code that is not well
    /// formed.
    static func route(for url: URL) -> String? {
        route(for: url, scheme: AppURLScheme.current)
    }

    /// [`route(for:)`] for the app whose scheme is `scheme`.
    static func route(for url: URL, scheme: String) -> String? {
        guard url.scheme?.lowercased() == scheme,
              let parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return nil }
        let segments = ([parts.host] + parts.path.split(separator: "/").map(String.init))
            .compactMap { $0 }.filter { !$0.isEmpty }
        guard segments.first?.lowercased() == "add" else { return nil }
        let items = parts.queryItems ?? []
        let raw = segments.count > 1 ? segments[1] : items.first { $0.name == "code" }?.value
        guard segments.count <= 2, let raw, let code = normalizeCode(raw) else { return nil }
        var route = "/machines?join=\(code)"
        if let relay = items.first(where: { $0.name == "relay" })?.value, !relay.isEmpty {
            route += "&relay=\(MainUIRoute.encodeComponent(relay))"
        }
        return route
    }

    /// Runs `cua <args>` off the main thread; throws the failure message on
    /// a non-zero exit, and after `timeout` seconds (the process is stopped).
    static func run(cua: String, args: [String], timeout: TimeInterval = JoinWithCode.timeout) async throws {
        let (status, err) = await Task.detached { () -> (Int32, String) in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: cua)
            process.arguments = args
            process.environment = ProcessInfo.processInfo.environment
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = FileHandle.nullDevice
            let pipe = Pipe()
            process.standardError = pipe
            do { try process.run() } catch { return (-1, "Could not start cua: \(error.localizedDescription)") }
            let timer = DispatchSource.makeTimerSource(queue: .global())
            nonisolated(unsafe) var timedOut = false
            timer.schedule(deadline: .now() + timeout)
            timer.setEventHandler { timedOut = true; process.terminate() }
            timer.resume()
            // Reads to EOF, so a chatty child never blocks on a full pipe.
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            timer.cancel()
            if timedOut { return (-1, "Joining timed out after \(Int(timeout)) seconds") }
            return (process.terminationStatus, String(decoding: data, as: UTF8.self))
        }.value
        if status != 0 { throw FailedRun(message: failureMessage(stderr: err, status: status)) }
    }

    struct FailedRun: Error { let message: String }
}
