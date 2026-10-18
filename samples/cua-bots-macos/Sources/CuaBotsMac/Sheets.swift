// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaBotsUI
import SwiftUI

// MARK: - Look

struct AvatarSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    @State var avatar = AvatarConfig()
    @State var name = ""
    @State var preview: BotMood = .idle

    var body: some View {
        VStack(spacing: 18) {
            KoalaAvatar(avatar, mood: preview).frame(width: 120, height: 120)
            Picker("Preview", selection: $preview) {
                Text("Idle").tag(BotMood.idle)
                Text("Thinking").tag(BotMood.thinking)
                Text("Working").tag(BotMood.working)
                Text("Asking").tag(BotMood.needsApproval)
                Text("Done").tag(BotMood.done)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            TextField("Name", text: $name).textFieldStyle(.roundedBorder).frame(maxWidth: 240)
            AvatarPicker(avatar: $avatar, name: name)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Save") {
                    if let bot = model.selectedBot {
                        model.store.updateAvatar(bot.id, avatar)
                        if name != bot.name { model.store.rename(bot.id, to: name) }
                    }
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 460)
        .onAppear {
            if let bot = model.selectedBot { avatar = bot.avatar; name = bot.name }
        }
    }
}

// MARK: - Rules

struct RulesSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    @State var adding = false
    @State var action = ""
    @State var behavior: RuleBehavior = .askFirst

    var body: some View {
        let bot = model.selectedBot
        let rules = bot.map { model.store.rules(for: $0.id) } ?? []
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text("Custom rules").font(.title3.weight(.semibold))
                Spacer()
                Button { adding = true } label: { Label("Add rule", systemImage: "plus") }
            }
            Text("\(bot?.name ?? "Your bot") follows these before it acts. Rules that keep you in charge of passwords and money can't be changed.")
                .font(.callout).foregroundStyle(.secondary)
            List {
                ForEach(rules) { r in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(r.action)
                            Text(r.isBuiltIn ? "Default" : "Yours").font(.caption).foregroundStyle(.tertiary)
                        }
                        Spacer()
                        if r.locked {
                            Label(r.behavior.short, systemImage: "lock").labelStyle(.titleAndIcon)
                                .font(.caption).foregroundStyle(.secondary)
                        } else {
                            Picker("", selection: Binding(get: { r.behavior }, set: { b in
                                guard let bot else { return }
                                var list = rules
                                if let i = list.firstIndex(where: { $0.id == r.id }) { list[i].behavior = b }
                                model.store.setRules(bot.id, list)
                            })) {
                                ForEach(RuleBehavior.allCases) { Text($0.short).tag($0) }
                            }
                            .labelsHidden()
                            .frame(width: 150)
                        }
                    }
                    .contextMenu {
                        if !r.locked && !r.isBuiltIn, let bot {
                            Button("Delete", role: .destructive) {
                                model.store.setRules(bot.id, rules.filter { $0.id != r.id })
                            }
                        }
                    }
                }
            }
            .listStyle(.inset(alternatesRowBackgrounds: true))
            .frame(minHeight: 260)
            Text("Written to its Volume home as agents/\(bot?.id ?? "")/rules.yaml, and into \(bot?.harness.instructionsFile ?? "its instructions") so the bot reads them.")
                .font(.caption).foregroundStyle(.tertiary)
            HStack { Spacer(); Button("Done") { dismiss() }.keyboardShortcut(.defaultAction) }
        }
        .padding(20)
        .frame(width: 560, height: 520)
        .sheet(isPresented: $adding) {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Button { adding = false } label: { Image(systemName: "xmark") }.buttonStyle(.borderless)
                    Spacer()
                    Text("Add rule").font(.headline)
                    Spacer()
                    Button {
                        if let bot, !action.trimmingCharacters(in: .whitespaces).isEmpty {
                            model.store.setRules(bot.id, rules + [CustomRule(action: action, behavior: behavior)])
                        }
                        action = ""
                        adding = false
                    } label: { Image(systemName: "checkmark") }
                    .buttonStyle(.borderedProminent)
                    .disabled(action.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                RuleForm(action: $action, behavior: $behavior, botName: bot?.name ?? "Your bot")
            }
            .padding(20)
            .frame(width: 440)
        }
    }
}

// MARK: - Memory

struct MemorySheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss

    var body: some View {
        let bot = model.selectedBot
        let files = bot.map { (try? model.store.volume.walk(VolumeLayout.home($0.id))) ?? [] } ?? []
        VStack(alignment: .leading, spacing: 12) {
            Text("\(bot?.name ?? "Bot")'s memory").font(.title3.weight(.semibold))
            Text("It lives in its Volume home, agents/\(bot?.id ?? "")/, so it outlives the bot's computer: pause, reset the Space, or move it to the cloud and it still remembers.")
                .font(.callout).foregroundStyle(.secondary)
            ScrollView {
                Text(bot.map { model.store.memory(for: $0.id) } ?? "")
                    .font(.body.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            }
            .background(RoundedRectangle(cornerRadius: 8).fill(.background))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.primary.opacity(0.1)))
            Text("Files in its home").font(.subheadline.weight(.medium)).foregroundStyle(.secondary)
            ForEach(files.filter { !$0.path.contains("/identity/") }.prefix(10)) { f in
                HStack {
                    Image(systemName: "doc").foregroundStyle(.secondary)
                    Text(f.path).font(.caption.monospaced())
                    Spacer()
                    Text(ByteCountFormatter.string(fromByteCount: Int64(f.size), countStyle: .file))
                        .font(.caption).foregroundStyle(.tertiary)
                }
            }
            HStack { Spacer(); Button("Done") { dismiss() }.keyboardShortcut(.defaultAction) }
        }
        .padding(20)
        .frame(width: 560, height: 500)
    }
}

// MARK: - Sign in

/// Step two of the private sign-in: use a saved sign-in from the Keyvault,
/// or take over and type it into the bot's computer yourself. Either way the
/// password never reaches the bot's model.
struct SignInSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    var approval: ApprovalRequest
    @State private var working = false
    @State private var failure: String?

    var site: String { if case .login(let s) = approval.source { return s }; return approval.action }

    var body: some View {
        let saved = model.keyvault.items(for: site)
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Image(systemName: "globe").foregroundStyle(.secondary)
                Text("https://\(site)").foregroundStyle(.secondary)
                Spacer()
                Button { dismiss() } label: { Image(systemName: "xmark") }.buttonStyle(.borderless)
            }
            Text("Sign in to \(site)").font(.title3.weight(.semibold))
            if let item = saved.first {
                Text("Use this saved sign-in? It's in your Cua Keyvault. The Cua daemon delivers it straight into \(model.selectedBot?.computerTitle ?? "the bot's computer") after Touch ID.")
                    .font(.callout).foregroundStyle(.secondary)
                HStack {
                    Image(systemName: "person.crop.circle")
                    Text(item.account ?? item.label)
                    Spacer()
                    Text("\(item.summary.cookies.count) cookies · \(item.summary.passwords) passwords")
                        .font(.caption).foregroundStyle(.secondary)
                }
                .padding(12)
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.primary.opacity(0.12)))
            } else {
                Text(model.keyvault.isReady
                     ? "There's no saved sign-in for \(site) in your Keyvault. Take over the bot's computer to sign in yourself; the bot waits."
                     : "\(model.keyvault.statusLine). You can take over the bot's computer and sign in yourself; the bot waits and never sees what you type.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            if let failure { Text(failure).font(.caption).foregroundStyle(.red) }
            Divider()
            HStack {
                Button("Take over to sign in") {
                    model.showComputer = true
                    dismiss()
                }
                Spacer()
                Button("Not now") {
                    Task { await model.store.decide(approval.id, approve: false) }
                    dismiss()
                }
                Button("Use saved sign-in") {
                    working = true
                    Task {
                        await model.store.decide(approval.id, approve: true)
                        working = false
                        if model.store.approval(approval.id)?.state == .approved { dismiss() }
                        else { failure = model.store.lastError; model.store.lastError = nil }
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(saved.isEmpty || working)
            }
        }
        .padding(20)
        .frame(width: 480)
        .task { await model.keyvault.refresh() }
    }
}

// MARK: - Output

struct OutputSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    var path: String

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Image(systemName: "doc.text")
                Text((path as NSString).lastPathComponent).font(.headline)
                Spacer()
                Text(path).font(.caption.monospaced()).foregroundStyle(.tertiary)
            }
            ScrollView {
                Text(MessageRow.markdown((try? model.store.volume.readText(path)) ?? "Not synced yet."))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            }
            .background(RoundedRectangle(cornerRadius: 8).fill(.background))
            HStack { Spacer(); Button("Done") { dismiss() }.keyboardShortcut(.defaultAction) }
        }
        .padding(20)
        .frame(width: 620, height: 520)
    }
}

extension MessageRow {
    static func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
    }
}

// MARK: - Pair iPhone

struct PairSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    var bot: Bot
    @State private var link: String?
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Pair iPhone").font(.title3.weight(.semibold))
            Text("Open this link in Cua Bots on your iPhone to chat with \(bot.name), watch its computer and answer its approvals. It carries a token for \(bot.computerTitle): share it only with your own devices.")
                .font(.callout).foregroundStyle(.secondary)
            if let link {
                Text(link).font(.caption.monospaced()).textSelection(.enabled).lineLimit(3)
                    .padding(10).background(RoundedRectangle(cornerRadius: 8).fill(.background))
            } else if let failure {
                Text(failure).font(.caption).foregroundStyle(.red)
            } else {
                ProgressView()
            }
            Text("This direct link works where the Mac is reachable (the iOS Simulator on this Mac, or your network). Reaching a bot from anywhere goes through the Cua relay.")
                .font(.caption).foregroundStyle(.tertiary)
            HStack {
                Spacer()
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(link ?? "", forType: .string)
                }
                .disabled(link == nil)
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 480)
        .task {
            do { link = try await model.engine?.pairingLink(bot) } catch { failure = error.localizedDescription }
        }
    }
}
