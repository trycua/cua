// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaBotsUI
import SwiftUI

struct RootView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 280)
        } detail: {
            detail
                .inspector(isPresented: inspectorBinding) {
                    if let bot = model.selectedBot {
                        ProfilePanel(bot: bot)
                            .inspectorColumnWidth(min: 290, ideal: 320, max: 380)
                    }
                }
        }
        .overlay(alignment: .top) { ToastView().padding(.top, 8) }
        .sheet(isPresented: $model.editingAvatar) { AvatarSheet() }
        .sheet(isPresented: $model.showingRules) { RulesSheet() }
        .sheet(isPresented: $model.showingMemory) { MemorySheet() }
        .sheet(item: $model.signInFor) { approval in SignInSheet(approval: approval) }
        .sheet(item: $model.pairing) { bot in PairSheet(bot: bot) }
        .sheet(item: Binding(get: { model.openOutput.map(OutputRef.init) },
                             set: { model.openOutput = $0?.path })) { OutputSheet(path: $0.path) }
        .confirmationDialog(resetTitle, isPresented: Binding(get: { model.confirmReset != nil },
                                                              set: { if !$0 { model.confirmReset = nil } }),
                            titleVisibility: .visible) {
            Button("Reset", role: .destructive) { if let b = model.confirmReset { model.reset(b) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This deletes its computer, conversation, memory and scheduled tasks. Files it made in other places stay.")
        }
        .alert("Something went wrong", isPresented: Binding(get: { model.store.lastError != nil },
                                                             set: { if !$0 { model.store.lastError = nil } })) {
            Button("OK") {}
        } message: {
            Text(model.store.lastError ?? "")
        }
    }

    var resetTitle: String { "Reset \(model.confirmReset?.name ?? "this bot")?" }

    var inspectorBinding: Binding<Bool> {
        Binding(get: { model.showProfile && model.selectedBot != nil }, set: { model.showProfile = $0 })
    }

    @ViewBuilder var detail: some View {
        switch model.route {
        case .bot(let id):
            if let bot = model.store.bot(id) {
                BotDetail(bot: bot)
            } else {
                NewBotView()
            }
        case .newBot: NewBotView()
        case .scheduled: ScheduledView()
        case .approvals: ApprovalsView()
        case .outputs: OutputsView()
        }
    }
}

struct OutputRef: Identifiable { var path: String; var id: String { path } }

// MARK: - Sidebar

struct Sidebar: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        List(selection: Binding(get: { model.route }, set: { if let r = $0 { model.route = r } })) {
            Section {
                Label("New bot", systemImage: "plus.bubble").tag(Route.newBot)
                ForEach(model.store.bots) { bot in
                    HStack(spacing: 8) {
                        KoalaAvatar(bot.avatar, mood: bot.mood).frame(width: 22, height: 22)
                        Text(bot.name)
                        Spacer()
                        if bot.isPaused {
                            Image(systemName: "pause.circle").foregroundStyle(.secondary).font(.caption)
                        } else if !model.store.pendingApprovals(for: bot.id).isEmpty
                                    || model.store.notifications.contains(where: { $0.botID == bot.id && !$0.read }) {
                            Circle().fill(Color.accentColor).frame(width: 6, height: 6)
                        }
                    }
                    .tag(Route.bot(bot.id))
                }
            }
            Section("Work") {
                Label("Scheduled", systemImage: "clock").tag(Route.scheduled)
                HStack {
                    Label("Approvals", systemImage: "checkmark.shield")
                    Spacer()
                    let n = model.store.pendingApprovals().count
                    if n > 0 { Text("\(n)").font(.caption).foregroundStyle(.secondary) }
                }
                .tag(Route.approvals)
                Label("Outputs", systemImage: "tray.full").tag(Route.outputs)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 6) {
                CuaMark().frame(width: 14, height: 14)
                Text("Cua Bots").font(.caption.weight(.medium))
                Spacer()
                Text(model.engine == nil ? "Offline" : "Spaces on this Mac").font(.caption2).foregroundStyle(.secondary)
            }
            .padding(10)
        }
    }
}

/// The Cua koala mark, drawn from the same geometry as the bots.
struct CuaMark: View {
    var body: some View {
        KoalaAvatar(AvatarConfig(color: .cloud, eyes: .star, ears: .scalloped), animated: false)
    }
}

// MARK: - Bot detail: chat, with the computer beside it

struct BotDetail: View {
    @EnvironmentObject var model: AppModel
    var bot: Bot

    var body: some View {
        HStack(spacing: 0) {
            ChatView(bot: bot)
                .frame(minWidth: 320, maxWidth: model.showComputer ? 360 : .infinity)
            if model.showComputer {
                Divider()
                ComputerPane(bot: bot)
                    .frame(minWidth: 420, maxWidth: .infinity)
                    .clipped()
                    .transition(.move(edge: .trailing))
            }
        }
        .navigationTitle(model.showComputer ? bot.computerTitle : bot.name)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    withAnimation(.spring(response: 0.2, dampingFraction: 0.95)) {
                        model.showComputer.toggle()
                        // The computer takes the room the profile had.
                        if model.showComputer { model.showProfile = false }
                    }
                } label: {
                    Label(bot.computerTitle, systemImage: "desktopcomputer")
                }
                .help("Open \(bot.computerTitle)")
                .disabled(bot.spaceID == nil)
                Button { model.togglePause(bot) } label: {
                    Label(bot.isPaused ? "Resume" : "Pause", systemImage: bot.isPaused ? "play.circle" : "pause.circle")
                }
                .help(bot.isPaused ? "Resume \(bot.name)" : "Pause \(bot.name)")
                Button { model.showProfile.toggle() } label: {
                    Label("Profile", systemImage: "sidebar.right")
                }
                .help("Show \(bot.name)'s profile")
            }
        }
        .onAppear { model.store.markAllRead(bot: bot.id) }
    }
}

// MARK: - Toast

struct ToastView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        if let t = model.toast, let bot = model.store.bot(t.botID) {
            Button {
                model.route = .bot(bot.id)
                model.store.markRead(t.id)
                model.toast = nil
            } label: {
                HStack(spacing: 10) {
                    KoalaAvatar(bot.avatar, mood: t.kind == .approval ? .needsApproval : .done)
                        .frame(width: 30, height: 30)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(t.title).font(.callout.weight(.semibold))
                        if !t.body.isEmpty { Text(t.body).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                    }
                }
                .padding(.horizontal, 14).padding(.vertical, 9)
                .background(Capsule().fill(.regularMaterial))
                .overlay(Capsule().strokeBorder(.primary.opacity(0.08)))
                .shadow(color: .black.opacity(0.12), radius: 10, y: 3)
            }
            .buttonStyle(.plain)
            .transition(.move(edge: .top).combined(with: .opacity))
        }
    }
}
