// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What a bot does when an action matches a rule: the four choices a rule
/// offers.
public enum RuleBehavior: String, Codable, CaseIterable, Identifiable, Sendable {
    case withoutAsking = "allow"
    case ifPreApproved = "pre-approved"
    case askFirst = "ask"
    case handOff = "hand-off"

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .withoutAsking: "Take action without asking"
        case .ifPreApproved: "Take action when you say so"
        case .askFirst: "Ask before taking action"
        case .handOff: "Hand off to you"
        }
    }

    public var short: String {
        switch self {
        case .withoutAsking: "Allowed"
        case .ifPreApproved: "When you say so"
        case .askFirst: "Asks first"
        case .handOff: "Hands off"
        }
    }

    public var symbol: String {
        switch self {
        case .withoutAsking: "checkmark.circle"
        case .ifPreApproved: "checkmark.seal"
        case .askFirst: "questionmark.circle"
        case .handOff: "hand.raised"
        }
    }
}

/// The kinds of action the rule engine recognizes in a bot's request.
public enum ActionClass: String, Codable, CaseIterable, Sendable {
    case purchase, delete, communicate, credentials, signIn, install, readOnly, other

    /// Words that put an action in the class. Checked in `ActionClass.allCases`
    /// order, so the most sensitive class wins.
    var keywords: [String] {
        switch self {
        case .credentials: ["password", "transfer money", "wire", "bank transfer", "2fa", "passcode"]
        case .purchase: ["buy", "purchase", "order", "checkout", "pay ", "payment", "book "]
        case .delete: ["delete", "remove", "erase", "wipe", "drop "]
        case .communicate: ["send", "email", "message", "post ", "reply", "invite", "text ", "slack"]
        case .signIn: ["sign in", "log in", "login", "log me in", "signin"]
        case .install: ["install", "brew ", "apt ", "download and run"]
        case .readOnly: ["read", "look up", "search", "summarize", "check", "review"]
        case .other: []
        }
    }

    public static func classify(_ action: String) -> ActionClass {
        let a = " " + action.lowercased() + " "
        for c in [ActionClass.credentials, .purchase, .delete, .communicate, .signIn, .install, .readOnly]
        where c.keywords.contains(where: { a.contains($0) }) {
            return c
        }
        return .other
    }
}

/// One custom rule: "When your bot wants to <action>, <behavior>."
public struct CustomRule: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    /// What the rule covers, in the user's words.
    public var action: String
    public var behavior: RuleBehavior
    /// The class it applies to (built-in rules), or nil to match on the
    /// user's words.
    public var actionClass: ActionClass?
    /// A built-in safety requirement: shown, never editable.
    public var locked: Bool
    public var isBuiltIn: Bool

    public init(id: String = UUID().uuidString, action: String, behavior: RuleBehavior,
                actionClass: ActionClass? = nil, locked: Bool = false, isBuiltIn: Bool = false) {
        self.id = id
        self.action = action
        self.behavior = behavior
        self.actionClass = actionClass
        self.locked = locked
        self.isBuiltIn = isBuiltIn
    }

    /// The defaults every bot starts with: act on reading, ask before
    /// buying, deleting or contacting anyone, and always hand password and
    /// money moves back to the user.
    public static let defaults: [CustomRule] = [
        CustomRule(id: "builtin.credentials", action: "Change a password or move money",
                   behavior: .handOff, actionClass: .credentials, locked: true, isBuiltIn: true),
        CustomRule(id: "builtin.purchase", action: "Make a purchase",
                   behavior: .askFirst, actionClass: .purchase, isBuiltIn: true),
        CustomRule(id: "builtin.delete", action: "Permanently delete data",
                   behavior: .askFirst, actionClass: .delete, isBuiltIn: true),
        CustomRule(id: "builtin.communicate", action: "Send a message or email to someone",
                   behavior: .askFirst, actionClass: .communicate, isBuiltIn: true),
        CustomRule(id: "builtin.signin", action: "Sign in to a site with a saved password",
                   behavior: .askFirst, actionClass: .signIn, isBuiltIn: true),
        CustomRule(id: "builtin.install", action: "Install software on its computer",
                   behavior: .withoutAsking, actionClass: .install, isBuiltIn: true),
        CustomRule(id: "builtin.read", action: "Read and research in the apps you connect",
                   behavior: .withoutAsking, actionClass: .readOnly, isBuiltIn: true),
    ]
}

/// The decision for one action.
public struct RuleDecision: Equatable, Sendable {
    public var behavior: RuleBehavior
    public var rule: CustomRule?
    public var actionClass: ActionClass
}

public enum RuleEngine {
    /// Decide what a bot may do. A user rule whose words appear in the action
    /// wins over the class defaults, except that a locked rule always wins.
    /// "If pre-approved" becomes "allow" when the user asked for exactly this
    /// in their own message.
    public static func decide(_ action: String, rules: [CustomRule], preApproved: Bool = false) -> RuleDecision {
        let cls = ActionClass.classify(action)
        let lower = action.lowercased()
        if let locked = rules.first(where: { $0.locked && $0.actionClass == cls }) {
            return RuleDecision(behavior: locked.behavior, rule: locked, actionClass: cls)
        }
        let custom = rules.filter { !$0.isBuiltIn && !$0.locked }
            .first { rule in
                let words = rule.action.lowercased()
                    .split(whereSeparator: { !$0.isLetter && !$0.isNumber })
                    .filter { $0.count > 3 && !stopWords.contains(String($0)) }
                return !words.isEmpty && words.allSatisfy { lower.contains($0) }
            }
        let rule = custom ?? rules.first { $0.actionClass == cls }
        var behavior = rule?.behavior ?? .withoutAsking
        if behavior == .ifPreApproved { behavior = preApproved ? .withoutAsking : .askFirst }
        return RuleDecision(behavior: behavior, rule: rule, actionClass: cls)
    }

    static let stopWords: Set<String> = ["when", "with", "that", "this", "your", "from", "into", "about"]

    /// The rules as the bot reads them, written to `agents/<name>/rules.yaml`.
    public static func yaml(_ rules: [CustomRule]) -> String {
        var out = "# Custom rules for this bot. Written by the Cua Bots app.\n"
        out += "# behavior: allow | pre-approved | ask | hand-off\nrules:\n"
        for r in rules {
            out += "  - action: \(quote(r.action))\n    behavior: \(r.behavior.rawValue)\n"
            if let c = r.actionClass { out += "    class: \(c.rawValue)\n" }
            if r.locked { out += "    locked: true\n" }
        }
        return out
    }

    /// The rules as standing instructions in the harness's instructions file.
    public static func instructions(_ rules: [CustomRule]) -> String {
        var out = "## Rules for acting on the user's behalf\n\n"
        out += "Before an action, find its rule. If it says **ask**, stop and write "
        out += "`[[ask: <the action> | <why and exact details>]]`, then wait for the user's "
        out += "\"Approved\" or \"Denied\". If it says **hand-off**, write `[[handoff: <the action>]]` "
        out += "and wait: the user does it themselves. **pre-approved** means allowed only when the user "
        out += "asked for exactly this in their own message; otherwise ask. Approving one action is not "
        out += "ongoing permission.\n\n"
        for r in rules {
            out += "- \(r.action): **\(r.behavior.rawValue)**\(r.locked ? " (cannot be changed)" : "")\n"
        }
        return out
    }

    private static func quote(_ s: String) -> String {
        "\"" + s.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }
}
