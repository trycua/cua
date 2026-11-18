// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import SwiftUI

// MARK: - Avatar

/// The koala avatar in a size, with an optional pencil badge.
public struct AvatarBadge: View {
    public var avatar: AvatarConfig
    public var mood: BotMood
    public var size: CGFloat
    public var editable: Bool

    public init(_ avatar: AvatarConfig, mood: BotMood = .idle, size: CGFloat, editable: Bool = false) {
        self.avatar = avatar
        self.mood = mood
        self.size = size
        self.editable = editable
    }

    public var body: some View {
        KoalaAvatar(avatar, mood: mood)
            .frame(width: size, height: size)
            .overlay(alignment: .bottomTrailing) {
                if editable {
                    Image(systemName: "pencil")
                        .font(.system(size: size * 0.13, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .padding(size * 0.06)
                        .background(Circle().fill(.background).shadow(color: .black.opacity(0.12), radius: 2, y: 1))
                        .offset(x: -size * 0.04, y: -size * 0.1)
                }
            }
    }
}

/// Color, eyes and ears: the customization a bot gets.
public struct AvatarPicker: View {
    @Binding public var avatar: AvatarConfig
    public var name: String

    public init(avatar: Binding<AvatarConfig>, name: String) {
        _avatar = avatar
        self.name = name
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            section("Color") {
                HStack(spacing: 10) {
                    ForEach(BotColor.allCases) { c in
                        Button { avatar.color = c } label: {
                            Circle().fill(c.face)
                                .overlay(Circle().strokeBorder(.primary.opacity(0.12)))
                                .frame(width: 26, height: 26)
                                .padding(3)
                                .overlay(Circle().strokeBorder(avatar.color == c ? Color.accentColor : .clear, lineWidth: 2))
                        }
                        .buttonStyle(.plain)
                        .help(c.name)
                        .accessibilityLabel(c.name)
                    }
                }
            }
            section("Eyes") {
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(58), spacing: 8), count: 6), spacing: 8) {
                    ForEach(EyeStyle.allCases) { e in
                        option(selected: avatar.eyes == e, label: e.name) {
                            avatar.eyes = e
                        } content: {
                            KoalaAvatar(AvatarConfig(color: avatar.color, eyes: e, ears: avatar.ears), animated: false)
                        }
                    }
                }
            }
            section("Ears") {
                HStack(spacing: 8) {
                    ForEach(EarShape.allCases) { ear in
                        option(selected: avatar.ears == ear, label: ear.name) {
                            avatar.ears = ear
                        } content: {
                            KoalaAvatar(AvatarConfig(color: avatar.color, eyes: avatar.eyes, ears: ear), animated: false)
                        }
                    }
                }
            }
        }
    }

    private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.subheadline.weight(.medium)).foregroundStyle(.secondary)
            content()
        }
    }

    private func option<C: View>(selected: Bool, label: String, action: @escaping () -> Void,
                                 @ViewBuilder content: () -> C) -> some View {
        Button(action: action) {
            VStack(spacing: 4) {
                content().frame(width: 46, height: 46)
                Text(label).font(.caption2).foregroundStyle(.secondary)
            }
            .frame(width: 58, height: 66)
            .background(RoundedRectangle(cornerRadius: 8).fill(selected ? Color.accentColor.opacity(0.12) : .clear))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(selected ? Color.accentColor : .primary.opacity(0.08)))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Messages

/// One message row: the bot's text flush left, the user's in a bubble on
/// the right, notices small and centered.
public struct MessageRow: View {
    public var message: ChatMessage
    public var accent: Color

    public init(_ message: ChatMessage, accent: Color) {
        self.message = message
        self.accent = accent
    }

    public var body: some View {
        switch message.role {
        case .user:
            HStack {
                Spacer(minLength: 60)
                Text(message.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(RoundedRectangle(cornerRadius: 16).fill(accent.opacity(0.22)))
            }
        case .bot:
            HStack {
                Text(Self.markdown(message.text))
                    .textSelection(.enabled)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(RoundedRectangle(cornerRadius: 16).fill(.primary.opacity(0.05)))
                Spacer(minLength: 60)
            }
        case .system:
            Text(message.text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity)
        }
    }

    static func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
    }
}

// MARK: - Cards

/// A request for approval, in the conversation and the Approvals list.
public struct ApprovalCard: View {
    public var approval: ApprovalRequest
    public var botName: String
    public var onDecide: (Bool) -> Void
    public var onOpenComputer: (() -> Void)?

    public init(_ approval: ApprovalRequest, botName: String, onDecide: @escaping (Bool) -> Void,
                onOpenComputer: (() -> Void)? = nil) {
        self.approval = approval
        self.botName = botName
        self.onDecide = onDecide
        self.onOpenComputer = onOpenComputer
    }

    var isHandOff: Bool { if case .handOff = approval.source { return true }; return false }

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(isHandOff ? "\(botName) needs you to do this" : "\(botName) wants to",
                  systemImage: isHandOff ? "hand.raised" : "questionmark.circle")
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
            Text(approval.action).font(.body.weight(.medium))
            if !approval.detail.isEmpty {
                Text(approval.detail).font(.callout).foregroundStyle(.secondary)
            }
            switch approval.state {
            case .pending:
                HStack {
                    if isHandOff, let onOpenComputer {
                        Button("Open computer", action: onOpenComputer)
                    }
                    Spacer()
                    Button(isHandOff ? "Not now" : "Deny") { onDecide(false) }
                    Button(isHandOff ? "I did it" : "Approve") { onDecide(true) }
                        .buttonStyle(.borderedProminent)
                }
                .controlSize(.regular)
            case .approved:
                Label("Approved", systemImage: "checkmark").font(.caption).foregroundStyle(.secondary)
            case .denied:
                Label("Denied", systemImage: "xmark").font(.caption).foregroundStyle(.secondary)
            case .handedOff:
                Label("You did it", systemImage: "checkmark").font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(14)
        .frame(maxWidth: 420, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12).fill(.background))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.primary.opacity(0.1)))
    }
}

/// "Sign in to <site>": step one of the private sign-in.
public struct SignInCard: View {
    public var site: String
    public var approval: ApprovalRequest?
    public var onOpen: () -> Void

    public init(site: String, approval: ApprovalRequest?, onOpen: @escaping () -> Void) {
        self.site = site
        self.approval = approval
        self.onOpen = onOpen
    }

    public var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 12) {
                Image(systemName: approval?.state == .approved ? "checkmark.shield" : "lock")
                    .foregroundStyle(.orange)
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(.background))
                VStack(alignment: .leading, spacing: 2) {
                    Text(approval?.state == .approved ? "Signed in to" : "Sign in to").font(.callout)
                    Text(site).font(.callout.weight(.medium))
                }
                Spacer()
                if approval?.state == .pending { Image(systemName: "chevron.right").foregroundStyle(.secondary) }
            }
            .padding(12)
            .frame(maxWidth: 360)
            .background(RoundedRectangle(cornerRadius: 22).fill(.primary.opacity(0.06)))
        }
        .buttonStyle(.plain)
        .disabled(approval?.state != .pending)
    }
}

/// A finished piece of work with its files.
public struct ResultCard: View {
    public var title: String
    public var outputs: [String]
    public var onOpen: (String) -> Void

    public init(title: String, outputs: [String], onOpen: @escaping (String) -> Void) {
        self.title = title
        self.outputs = outputs
        self.onOpen = onOpen
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: "checkmark.circle").font(.callout.weight(.medium))
            ForEach(outputs, id: \.self) { path in
                Button { onOpen(path) } label: {
                    HStack {
                        Image(systemName: "doc.text").foregroundStyle(.secondary)
                        Text((path as NSString).lastPathComponent)
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                    }
                }
                .buttonStyle(.plain)
            }
        }
        .padding(12)
        .frame(maxWidth: 360, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12).strokeBorder(.primary.opacity(0.1)))
    }
}

// MARK: - Rules

/// "When <name> wants to ... / <name> should ...": the rule sheet's body.
public struct RuleForm: View {
    @Binding public var action: String
    @Binding public var behavior: RuleBehavior
    public var botName: String

    public init(action: Binding<String>, behavior: Binding<RuleBehavior>, botName: String) {
        _action = action
        _behavior = behavior
        self.botName = botName
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 6) {
                Text("When \(botName) wants to").font(.subheadline).foregroundStyle(.secondary)
                TextField("Share anything about the surprise party", text: $action, axis: .vertical)
                    .lineLimit(3...5)
                    .textFieldStyle(.roundedBorder)
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("\(botName) should").font(.subheadline).foregroundStyle(.secondary)
                VStack(spacing: 0) {
                    ForEach(RuleBehavior.allCases) { b in
                        Button { behavior = b } label: {
                            HStack(alignment: .top) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(b.title)
                                    Text(Self.explain(b, botName)).font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                Image(systemName: behavior == b ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(behavior == b ? Color.accentColor : .secondary)
                                    .font(.title3)
                            }
                            .padding(.vertical, 8)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if b != RuleBehavior.allCases.last { Divider() }
                    }
                }
                .padding(.horizontal, 12)
                .background(RoundedRectangle(cornerRadius: 10).strokeBorder(.primary.opacity(0.1)))
            }
        }
    }

    static func explain(_ b: RuleBehavior, _ name: String) -> String {
        switch b {
        case .withoutAsking: "\(name) can do this without asking."
        case .ifPreApproved: "\(name) goes ahead only when you asked for it yourself."
        case .askFirst: "\(name) asks for your approval first."
        case .handOff: "\(name) asks you to do it, and never does it on its own."
        }
    }
}

// MARK: - Tasks

public struct TaskRow: View {
    public var task: BotTask
    public var unread: Bool

    public init(_ task: BotTask, unread: Bool = false) {
        self.task = task
        self.unread = unread
    }

    public var body: some View {
        HStack(spacing: 10) {
            icon.frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(task.title).lineLimit(1)
                let sub = task.subtitle()
                if !sub.isEmpty { Text(sub).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            Spacer()
            if unread { Circle().fill(Color.accentColor).frame(width: 6, height: 6) }
        }
    }

    @ViewBuilder var icon: some View {
        switch task.state {
        case .inProgress: ProgressView().controlSize(.small)
        case .completed: Image(systemName: "checkmark.circle").foregroundStyle(.secondary)
        case .scheduled: Image(systemName: task.symbol == "circle.dashed" ? "clock" : task.symbol).foregroundStyle(.secondary)
        case .paused: Image(systemName: "pause.circle").foregroundStyle(.secondary)
        case .failed: Image(systemName: "exclamationmark.circle").foregroundStyle(.orange)
        }
    }
}

/// "Active 2 minutes ago", "Working", "Paused".
public func presenceLine(_ bot: Bot, busy: Bool, lastActive: Date?) -> String {
    if bot.isPaused { return "Paused" }
    if busy || bot.mood == .thinking || bot.mood == .working { return bot.status }
    if bot.mood == .needsApproval { return "Waiting for you" }
    guard let lastActive else { return bot.status }
    let minutes = Int(Date().timeIntervalSince(lastActive) / 60)
    if minutes < 1 { return "Active just now" }
    if minutes < 60 { return "Active \(minutes) minute\(minutes == 1 ? "" : "s") ago" }
    return "Active \(lastActive.formatted(.relative(presentation: .named)))"
}
