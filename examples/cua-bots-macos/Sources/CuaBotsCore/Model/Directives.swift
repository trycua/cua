// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The small protocol a bot uses inside its replies to drive the app: status
/// under its name, steps in the activity view, approvals, notifications and
/// "log me into this site". The bot learns it from its instructions file
/// (`BotInstructions`); the app strips the markers and renders the rest.
///
/// ```text
/// [[status: Checking inbox]]
/// [[step: Trace dependencies]]        [[step-done: Trace dependencies]]
/// [[ask: Send the invoice to Lina | $1,200, due Friday]]
/// [[handoff: Change the bank password]]
/// [[login: github.com]]
/// [[notify: Your research is ready | Three sources, one summary]]
/// [[output: agents/ada/outputs/research.md]]
/// [[done: Research teething tips]]
/// [[schedule: daily 09:00 | Morning briefing | Summarize overnight email]]
/// ```
public enum Directive: Equatable, Sendable {
    case status(String)
    case step(String)
    case stepDone(String)
    case ask(action: String, detail: String)
    case handOff(String)
    case login(site: String)
    case notify(title: String, body: String)
    case output(path: String)
    case done(title: String)
    case schedule(TaskSchedule, title: String, prompt: String)
}

public struct ParsedReply: Equatable, Sendable {
    /// The text a person reads, with every marker removed.
    public var text: String
    public var directives: [Directive]
}

public enum DirectiveParser {
    public static func parse(_ raw: String) -> ParsedReply {
        var directives: [Directive] = []
        var lines: [String] = []
        for line in raw.components(separatedBy: "\n") {
            let (text, found) = strip(line)
            directives.append(contentsOf: found)
            // A line that only carried markers disappears; a blank line the
            // bot wrote stays a paragraph break.
            if !found.isEmpty && text.trimmingCharacters(in: .whitespaces).isEmpty { continue }
            lines.append(text)
        }
        return ParsedReply(text: tidy(lines.joined(separator: "\n")), directives: directives)
    }

    /// Remove the recognized markers from one line.
    static func strip(_ line: String) -> (String, [Directive]) {
        var directives: [Directive] = []
        var text = ""
        var rest = Substring(line)
        while let open = rest.range(of: "[[") {
            text += rest[rest.startIndex..<open.lowerBound]
            guard let close = rest.range(of: "]]", range: open.upperBound..<rest.endIndex) else {
                text += rest[open.lowerBound...]
                rest = ""
                break
            }
            let body = String(rest[open.upperBound..<close.lowerBound])
            if let d = directive(body) {
                directives.append(d)
            } else {
                text += rest[open.lowerBound..<close.upperBound]
            }
            rest = rest[close.upperBound...]
        }
        text += rest
        return (text, directives)
    }

    static func directive(_ body: String) -> Directive? {
        guard let colon = body.firstIndex(of: ":") else { return nil }
        let key = body[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
        let value = body[body.index(after: colon)...].trimmingCharacters(in: .whitespacesAndNewlines)
        if key == "schedule" {
            let p = value.split(separator: "|", maxSplits: 2).map { $0.trimmingCharacters(in: .whitespaces) }
            guard p.count >= 2, let schedule = parseSchedule(p[0]) else { return nil }
            return .schedule(schedule, title: p[1], prompt: p.count > 2 ? p[2] : p[1])
        }
        let parts = value.split(separator: "|", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        let first = parts.first ?? ""
        let second = parts.count > 1 ? parts[1] : ""
        guard !first.isEmpty else { return nil }
        switch key {
        case "status": return .status(first)
        case "step": return .step(first)
        case "step-done", "stepdone", "done-step": return .stepDone(first)
        case "ask", "approve": return .ask(action: first, detail: second)
        case "handoff", "hand-off": return .handOff(first)
        case "login", "log-in", "signin": return .login(site: first)
        case "notify": return .notify(title: first, body: second)
        case "output": return .output(path: first)
        case "done": return .done(title: first)
        default: return nil
        }
    }

    /// `daily 09:00`, `weekly mon 08:30`, `every 30m`, `every 2h`.
    public static func parseSchedule(_ spec: String) -> TaskSchedule? {
        let w = spec.lowercased().split(separator: " ").map(String.init)
        func clock(_ s: String) -> (Int, Int)? {
            let hm = s.split(separator: ":").compactMap { Int($0) }
            guard hm.count == 2, (0..<24).contains(hm[0]), (0..<60).contains(hm[1]) else { return nil }
            return (hm[0], hm[1])
        }
        switch w.first {
        case "daily":
            guard w.count >= 2, let (h, m) = clock(w[1]) else { return nil }
            return .daily(hour: h, minute: m)
        case "weekly":
            let days = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
            guard w.count >= 3, let d = days.firstIndex(where: { w[1].hasPrefix($0) }),
                  let (h, m) = clock(w[2]) else { return nil }
            return .weekly(weekday: d + 1, hour: h, minute: m)
        case "every":
            guard w.count >= 2 else { return nil }
            let n = w[1].filter(\.isNumber)
            guard let v = Int(n), v > 0 else { return nil }
            return .every(minutes: w[1].hasSuffix("h") ? v * 60 : v)
        default:
            return nil
        }
    }

    /// Collapse the blank lines a removed marker leaves behind.
    static func tidy(_ s: String) -> String {
        let lines = s.components(separatedBy: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        var out: [String] = []
        for line in lines {
            if line.isEmpty, out.last?.isEmpty ?? true { continue }
            out.append(line)
        }
        while out.last?.isEmpty == true { out.removeLast() }
        return out.joined(separator: "\n")
    }
}

/// The standing instructions a bot's harness reads from its home, written by
/// the app into the Volume (`agents/<name>/`) and copied into the Space.
public enum BotInstructions {
    public static func render(bot: Bot, rules: [CustomRule], memoryPath: String = "memory/MEMORY.md") -> String {
        """
        # You are \(bot.name)

        You are \(bot.owner)'s bot: a persistent assistant that works on its own \
        computer (this machine) and keeps working between conversations. \
        Your handle is \(bot.handle). Be brief and concrete.

        ## Memory

        Your memory lives in `\(memoryPath)` next to this file. Read it at the start of \
        every conversation. When you learn something about \(bot.owner), their goals, \
        preferences or standards, add one line to it. Never write passwords, tokens or \
        keys into memory: sign-ins go through the user's Keyvault.

        Put finished work in `outputs/`. Files the user hands you arrive in `inbox/`.

        ## Talking to the app

        Put these markers on their own lines; the app shows them as status, steps, \
        cards and notifications, and hides the markers:

        - `[[status: <3 words>]]` what you are doing now ("Checking inbox")
        - `[[step: <step>]]` a step you are starting, `[[step-done: <step>]]` when it is done
        - `[[ask: <action> | <details>]]` before any action your rules say to ask about
        - `[[handoff: <action>]]` when the user must do it themselves
        - `[[login: <site>]]` when you need to be signed in to a site; wait for "Signed in"
        - `[[output: outputs/<file>]]` for each file you finish
        - `[[notify: <title> | <one line>]]` when something finished or needs the user
        - `[[done: <task title>]]` when a task is complete
        - `[[schedule: daily 09:00 | <title> | <what to do each time>]]` when asked to do \
        something on a schedule (also `weekly mon 08:30`, `every 30m`); the app runs it for you

        \(RuleEngine.instructions(rules))
        """
    }
}
