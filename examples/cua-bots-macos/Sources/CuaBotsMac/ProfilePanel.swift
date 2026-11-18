// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaBotsUI
import SwiftUI

/// The bot's profile beside the chat: who it is, its computers, what it's
/// doing, what it made, and the controls (pause, reset, rules, memory).
struct ProfilePanel: View {
    @EnvironmentObject var model: AppModel
    var bot: Bot
    @State private var showAllActivity = false
    @State private var thumbnail: NSImage?

    var tasks: [BotTask] { model.store.tasks(for: bot.id) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                identity
                computers
                notifications
                activity
                outputs
                customize
            }
            .padding(16)
        }
        .task(id: bot.spaceID) {
            // A fresh look at its screen every half minute while visible.
            while !Task.isCancelled {
                await loadThumbnail()
                try? await Task.sleep(for: .seconds(thumbnail == nil ? 5 : 30))
            }
        }
    }

    // MARK: Identity

    var identity: some View {
        VStack(spacing: 6) {
            HStack {
                Spacer()
                Menu {
                    Button(bot.isPaused ? "Resume" : "Pause") { model.togglePause(bot) }
                    Button("Rename and edit look…") { model.editingAvatar = true }
                    Button("Pair iPhone…") { model.pairing = bot }.disabled(bot.spaceID == nil)
                    Divider()
                    Button("Reset…", role: .destructive) { model.confirmReset = bot }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            Button { model.editingAvatar = true } label: {
                AvatarBadge(bot.avatar, mood: bot.mood, size: 84, editable: true)
            }
            .buttonStyle(.plain)
            .help("Edit \(bot.name)'s look")
            Text(bot.name).font(.title3.weight(.semibold))
            Text(bot.handle).font(.caption).foregroundStyle(.tertiary)
            if bot.isPaused {
                Button { model.togglePause(bot) } label: {
                    Text("Paused · Click to resume").font(.caption)
                }
                .buttonStyle(.link)
            } else {
                Text(presenceLine(bot, busy: model.store.isBusy(bot.id), lastActive: model.lastActive(bot.id)))
                    .font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 14) {
                circleButton("bubble.left", "Message") { model.route = .bot(bot.id) }
                circleButton("desktopcomputer", "Open computer") { model.showComputer = true }
                circleButton(bot.isPaused ? "play" : "pause", bot.isPaused ? "Resume" : "Pause") { model.togglePause(bot) }
                circleButton("pip.enter", "Picture in picture") { model.popOut(bot) }
            }
            .padding(.top, 4)
        }
        .frame(maxWidth: .infinity)
    }

    func circleButton(_ symbol: String, _ help: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .frame(width: 32, height: 32)
                .background(Circle().strokeBorder(.primary.opacity(0.15)))
        }
        .buttonStyle(.plain)
        .help(help)
    }

    // MARK: Computers

    var computers: some View {
        section("Computers") {
            row {
                Image(systemName: "desktopcomputer").overlay(alignment: .bottomTrailing) {
                    Circle().fill(bot.spaceID == nil ? Color.secondary : bot.isPaused ? .orange : .green)
                        .frame(width: 6, height: 6).offset(x: 2, y: 2)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(bot.computerTitle)
                    Text(bot.spaceID == nil ? (model.store.setupPhase[bot.id] ?? "Not created")
                         : bot.isPaused ? "Suspended" : "Connected")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button { model.showComputer = true } label: {
                    Group {
                        if let thumbnail {
                            Image(nsImage: thumbnail).resizable().aspectRatio(contentMode: .fill)
                        } else {
                            Rectangle().fill(bot.avatar.color.accent.opacity(0.4))
                        }
                    }
                    .frame(width: 52, height: 32)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                }
                .buttonStyle(.plain)
                .disabled(bot.spaceID == nil)
            }
            Divider()
            row {
                Image(systemName: "laptopcomputer")
                VStack(alignment: .leading, spacing: 1) {
                    Text(model.host.machineName)
                    Text(bot.hostAccess.isAllowed ? "Your computer · Connected" : "Your computer")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if bot.hostAccess.isAllowed {
                    Button("Revoke") { model.setHostAccess(bot, allow: false) }.controlSize(.small)
                } else {
                    Button("Allow") { model.setHostAccess(bot, allow: true) }
                        .buttonStyle(.borderedProminent).controlSize(.small)
                        .help(model.host.isConfigured ? "Let \(bot.name) use this Mac"
                              : "Set up this Mac with `cua host setup` first")
                }
            }
        }
    }

    // MARK: Notifications

    @ViewBuilder var notifications: some View {
        if model.canUseSystemNotifications && model.systemNotifications != .authorized {
            box {
                row {
                    Image(systemName: "bell")
                    Text("Enable notifications")
                    Spacer()
                    Button("Allow") { Task { await model.enableNotifications() } }
                        .buttonStyle(.borderedProminent).controlSize(.small)
                }
            }
        }
    }

    // MARK: Activity

    var activity: some View {
        let visible = showAllActivity ? tasks : Array(tasks.prefix(4))
        return section("Activity") {
            if tasks.isEmpty {
                Text("Nothing yet. Ask \(bot.name) for something, or schedule a routine.")
                    .font(.caption).foregroundStyle(.secondary).padding(.vertical, 6)
            }
            ForEach(visible) { t in
                TaskRow(t, unread: t.state == .completed && t.lastRun.map { Date().timeIntervalSince($0) < 600 } == true)
                    .padding(.vertical, 5)
                    .contextMenu {
                        if t.schedule != nil {
                            Button("Run now") { Task { _ = await model.store.fire(taskID: t.id, botID: bot.id) } }
                            Button("Delete", role: .destructive) { model.store.deleteTask(t) }
                        }
                    }
                if t.id != visible.last?.id { Divider() }
            }
            if tasks.count > 4 {
                Divider()
                Button { showAllActivity.toggle() } label: {
                    Label(showAllActivity ? "Show less" : "Show more",
                          systemImage: showAllActivity ? "chevron.up" : "chevron.down")
                        .font(.callout)
                }
                .buttonStyle(.plain).padding(.vertical, 5)
            }
        }
    }

    // MARK: Outputs

    var outputs: some View {
        let files = model.store.outputs(for: bot.id)
        return section("Outputs") {
            if files.isEmpty {
                Text("Finished files land in its Volume home at agents/\(bot.id)/outputs.")
                    .font(.caption).foregroundStyle(.secondary).padding(.vertical, 6)
            }
            ForEach(files.prefix(6)) { f in
                Button { model.openOutput = f.path } label: {
                    HStack {
                        Image(systemName: Self.icon(for: f.name)).foregroundStyle(.secondary).frame(width: 18)
                        Text(f.name).lineLimit(1)
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                    }
                    .padding(.vertical, 5)
                }
                .buttonStyle(.plain)
                if f.id != files.prefix(6).last?.id { Divider() }
            }
        }
    }

    static func icon(for name: String) -> String {
        switch (name as NSString).pathExtension.lowercased() {
        case "png", "jpg", "jpeg", "gif": "photo"
        case "csv", "xlsx": "tablecells"
        default: "doc.text"
        }
    }

    // MARK: Customize

    var customize: some View {
        section("Customize") {
            navRow("brain", "Memory", "agents/\(bot.id)/memory") { model.showingMemory = true }
            Divider()
            navRow("checklist", "Custom rules", "\(model.store.rules(for: bot.id).count) rules") { model.showingRules = true }
            Divider()
            navRow("key", "Saved sign-ins", model.keyvault.statusLine) {
                Task { await model.keyvault.refresh() }
            }
            Divider()
            row {
                Image(systemName: "cpu")
                Text("Runs on")
                Spacer()
                Text("\(bot.harness.name) · \(bot.placement.name)").foregroundStyle(.secondary)
            }
        }
    }

    func navRow(_ symbol: String, _ title: String, _ detail: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            row {
                Image(systemName: symbol).frame(width: 18)
                Text(title)
                Spacer()
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
            }
        }
        .buttonStyle(.plain)
    }

    // MARK: Layout helpers

    func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.subheadline.weight(.medium)).foregroundStyle(.secondary)
            box(content)
        }
    }

    func box<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 0) { content() }
            .padding(.horizontal, 12).padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 10).fill(.background))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.primary.opacity(0.08)))
    }

    func row<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        HStack(spacing: 10) { content() }.padding(.vertical, 6).contentShape(Rectangle())
    }

    func loadThumbnail() async {
        guard bot.spaceID != nil, let engine = model.engine,
              let space = try? await engine.space(bot),
              let shot = try? await space.screenshot(options: nil) else { return }
        thumbnail = NSImage(data: shot.image)
    }
}
