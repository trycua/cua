// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import CuaSpaces
import CuaSpacesStreaming
import SwiftUI
import UniformTypeIdentifiers

/// The app window: a sidebar of Bots, routines and group chats on the left, the
/// selected conversation in the middle, and the Agent Computer on the right
/// when it is open.
///
/// A conventional desktop chat layout: an empty conversation is a centred
/// prompt with a large composer; a conversation with messages is a centred
/// column with the user's turns in bubbles on the right, the Bot's replies as
/// plain text on the left, and the composer docked at the bottom. Every
/// surface is opaque (`KoalaPalette`).
struct KoalaShell: View {
    @ObservedObject var model: AppModel
    @ObservedObject var store: BotStore
    @ObservedObject var routines: RoutineStore
    @ObservedObject var groups: GroupChatStore
    @ObservedObject var pip: StreamPiPController

    @Environment(\.colorScheme) private var scheme
    private var p: KoalaPalette { .resolve(scheme) }

    @State private var search = ""
    @State private var draft = ""
    /// Owned by `DesktopSurface`, so the Computer pane and the takeover share
    /// one drop coordinator.
    @ObservedObject private var drops: SpaceDropCoordinator

    init(model: AppModel, drops: SpaceDropCoordinator) {
        self.model = model
        self.store = model.store
        self.drops = drops
        self.routines = model.routines
        self.groups = model.groups
        self.pip = model.pip
    }

    static let sidebarWidth: CGFloat = 260
    static let columnMax: CGFloat = 760
    static let computerPaneWidth: CGFloat = 340
    static let headerHeight: CGFloat = 52
    /// Room for the window's traffic lights when the header runs under them.
    static let trafficLightInset: CGFloat = 78
    /// The title bar's height: the window content runs under it, and the
    /// sidebar's first strip is exactly this tall so its control lines up with
    /// the traffic lights.
    static let titleStrip: CGFloat = 32

    // MARK: Derived state

    /// The Bot the conversation pane shows. Unlike `AppModel.focusedBot` this
    /// does not fall back to the first Bot: with nothing selected the pane is
    /// the new-conversation prompt.
    private var bot: Bot? { model.route.botID.flatMap { store.bot($0) } }
    private var messages: [Message] { bot.map { store.thread(for: $0.id).messages } ?? [] }
    private var computerOpen: Bool {
        if case .preview = model.route { return true }
        return false
    }

    var body: some View {
        HStack(spacing: 0) {
            if model.sidebarVisible {
                sidebar.frame(width: Self.sidebarWidth)
                Rectangle().fill(p.border).frame(width: 1)
            }
            VStack(spacing: 0) {
                header
                Rectangle().fill(p.border).frame(height: 1)
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .background(p.bg)
            if computerOpen, let bot {
                Rectangle().fill(p.border).frame(width: 1)
                computerPane(bot).frame(width: Self.computerPaneWidth)
            }
        }
        .background(p.bg)
        .foregroundStyle(p.text)
        .ignoresSafeArea(.container, edges: .top)
    }

    // MARK: - Sidebar

    enum SidebarItem: Hashable {
        case bot(String)
        case routines(String)
        case group(String)
    }

    private var selection: Binding<SidebarItem?> {
        Binding {
            switch model.route {
            case .thread(let id), .preview(let id), .takeover(let id): return .bot(id)
            case .routines(let id): return .routines(id)
            case .groupThread(let id): return .group(id)
            case .roster: return nil
            }
        } set: { item in
            switch item {
            case .bot(let id): model.open(.thread(id))
            case .routines(let id): model.open(.routines(id))
            case .group(let id): model.open(.groupThread(id))
            case nil: break
            }
        }
    }

    private var filteredBots: [Bot] {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        let visible = store.bots.filter { !$0.isHiddenFromSidebar }
        return q.isEmpty ? visible : visible.filter {
            $0.name.lowercased().contains(q) || $0.preview.lowercased().contains(q)
        }
    }

    /// Bots that have at least one routine, in roster order.
    private var routineOwners: [Bot] {
        store.bots.filter { !routines.routines(for: $0.id).isEmpty }
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            // The traffic lights sit in the first strip, with the collapse
            // control at the far end; the brand is the row under them.
            HStack {
                Spacer()
                iconButton("sidebar.left", help: ShellLabels.hideSidebar) {
                    withAnimation(.easeOut(duration: 0.15)) { model.sidebarVisible = false }
                }
            }
            .padding(.horizontal, 10)
            .frame(height: Self.titleStrip)
            HStack(spacing: 8) {
                KoalaMark(size: 24)
                Text(AppIdentity.name).font(.system(size: 15, weight: .semibold))
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .frame(height: 40)

            VStack(spacing: 2) {
                actionRow("square.and.pencil", "New Bot", shortcut: "\u{2318}N") { newBot() }
                actionRow("macwindow.badge.plus", "New Space", shortcut: nil) {
                    model.creatingSpace = true
                }
            }
            .padding(.horizontal, 8)

            searchField.padding(.horizontal, 10).padding(.vertical, 10)

            List(selection: selection) {
                Section {
                    if filteredBots.isEmpty {
                        Text(search.isEmpty ? EmptyStateCopy.noBots : "No matches")
                            .font(.callout).foregroundStyle(p.secondary)
                            .selectionDisabled()
                    }
                    ForEach(filteredBots) { b in
                        botRow(b).tag(SidebarItem.bot(b.id))
                    }
                } header: { sectionHeader("Bots") }
                Section {
                    if routineOwners.isEmpty {
                        Text("No routines yet").font(.callout).foregroundStyle(p.secondary)
                            .selectionDisabled()
                    }
                    ForEach(routineOwners) { b in
                        let rs = routines.routines(for: b.id)
                        sidebarRow(symbol: "clock", title: rs.first?.title ?? "Routine",
                                   subtitle: rs.count == 1 ? b.name : "\(b.name), \(rs.count) routines")
                            .tag(SidebarItem.routines(b.id))
                    }
                } header: { sectionHeader("Routines") }
                Section {
                    ForEach(groups.chats) { chat in
                        sidebarRow(symbol: "person.2", title: chat.title,
                                   subtitle: chat.membershipLabel)
                            .tag(SidebarItem.group(chat.id))
                    }
                    Button {
                        model.creatingGroup = true
                    } label: {
                        Label("New group chat", systemImage: "plus")
                            .font(.callout).foregroundStyle(p.secondary)
                    }
                    .buttonStyle(.plain)
                    .disabled(store.bots.count < GroupChat.minBots)
                    .help(store.bots.count < GroupChat.minBots
                          ? "A group needs at least \(GroupChat.minBots) Bots" : "Put Bots in one thread")
                    .selectionDisabled()
                } header: { sectionHeader("Group chats") }
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
            .tint(p.secondary)

            Rectangle().fill(p.border).frame(height: 1)
            accountRow
        }
        .background(p.sidebar)
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title).font(.system(size: 11, weight: .semibold))
            .foregroundStyle(p.secondary)
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").font(.system(size: 12))
                .foregroundStyle(p.secondary)
            TextField(SidebarStrings.search, text: $search)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
            if !search.isEmpty {
                Button { search = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(p.secondary)
                }
                .buttonStyle(.plain)
                .help(ShellLabels.clearSearch)
                .accessibilityLabel(ShellLabels.clearSearch)
            }
        }
        .padding(.horizontal, 9)
        .frame(height: 30)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(p.surface2))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(p.border))
    }

    private func actionRow(_ symbol: String, _ title: String, shortcut: String?,
                           action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: symbol).font(.system(size: 13)).frame(width: 20)
                Text(title).font(.system(size: 13, weight: .medium))
                Spacer(minLength: 0)
                if let shortcut {
                    Text(shortcut).font(.system(size: 11)).foregroundStyle(p.secondary)
                }
            }
            .padding(.horizontal, 8)
            .frame(height: 30)
            .contentShape(Rectangle())
        }
        .buttonStyle(SidebarButtonStyle(p: p))
    }

    private func botRow(_ b: Bot) -> some View {
        let subtitle = store.creating.contains(b.id)
            ? EmptyStateCopy.creating
            : b.preview.trimmingCharacters(in: .whitespacesAndNewlines)
        return HStack(spacing: 10) {
            BotAvatar(bot: b, size: 26)
            VStack(alignment: .leading, spacing: 1) {
                Text(b.name).font(.system(size: 13, weight: .medium)).lineLimit(1)
                    .foregroundStyle(p.text)
                if !subtitle.isEmpty {
                    Text(subtitle).font(.system(size: 12)).foregroundStyle(p.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if store.presence(for: b.id).state == .running {
                Circle().fill(p.accent).frame(width: 6, height: 6)
                    .help("Working")
            }
        }
        .padding(.vertical, 3)
        .help(subtitle.isEmpty ? EmptyStateCopy.noMessagesYet : subtitle)
    }

    private func sidebarRow(symbol: String, title: String, subtitle: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol).font(.system(size: 12))
                .foregroundStyle(p.secondary).frame(width: 26)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.system(size: 13, weight: .medium)).lineLimit(1)
                    .foregroundStyle(p.text)
                Text(subtitle).font(.system(size: 12)).foregroundStyle(p.secondary).lineLimit(1)
            }
        }
        .padding(.vertical, 2)
    }

    private var accountRow: some View {
        HStack(spacing: 10) {
            ZStack {
                Circle().fill(p.surface2)
                Text(Account.initials).font(.system(size: 11, weight: .semibold))
            }
            .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(Account.displayName).font(.system(size: 13, weight: .medium)).lineLimit(1)
                Text(spaceLine).font(.system(size: 11)).foregroundStyle(p.secondary).lineLimit(1)
            }
            Spacer(minLength: 0)
            Menu {
                Picker("Appearance", selection: $model.desktopTheme) {
                    ForEach(AppAppearance.allCases) { a in Text(a.label).tag(a.rawValue) }
                }
                Divider()
                Button("New Space\u{2026}") { model.creatingSpace = true }
                if !model.createdSpaces.isEmpty {
                    Section("Created here") {
                        ForEach(model.createdSpaces, id: \.self) { Text($0) }
                    }
                }
            } label: {
                Image(systemName: "gearshape").font(.system(size: 13))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help(ShellLabels.settings)
            .accessibilityLabel(ShellLabels.settings)
        }
        .padding(.horizontal, 14)
        .frame(height: 56)
    }

    private var spaceLine: String {
        switch store.connection {
        case .attached(let s): return model.isLiveBackend ? s : "Offline demo"
        case .connecting: return "Connecting\u{2026}"
        case .failed: return "Not connected"
        case .detached: return "Detached"
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            if !model.sidebarVisible {
                Spacer().frame(width: Self.trafficLightInset - 14)
                iconButton("sidebar.left", help: ShellLabels.showSidebar) {
                    withAnimation(.easeOut(duration: 0.15)) { model.sidebarVisible = true }
                }
            }
            titleBlock
            statusPill
            Spacer(minLength: 8)
            if let bot {
                iconButton(computerOpen ? "display.trianglebadge.exclamationmark" : "display",
                           help: computerOpen ? "Hide \(bot.name)\u{2019}s Computer"
                                              : "\(bot.name)\u{2019}s Computer",
                           highlighted: computerOpen) {
                    model.open(computerOpen ? .thread(bot.id) : .preview(bot.id))
                }
            }
            moreMenu
        }
        .padding(.horizontal, 14)
        .frame(height: Self.headerHeight)
        .background(p.bg)
    }

    @ViewBuilder private var titleBlock: some View {
        switch model.route {
        case .groupThread(let id):
            Label(groups.chat(id)?.title ?? "Group chat", systemImage: "person.2")
                .font(.system(size: 13, weight: .semibold))
        case .routines(let id):
            Label("Routines, \(store.bot(id)?.name ?? "")", systemImage: "clock")
                .font(.system(size: 13, weight: .semibold))
        default:
            if let bot {
                HStack(spacing: 8) {
                    BotAvatar(bot: bot, size: 20)
                    Text(bot.name).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                }
            } else {
                Text("New Bot").font(.system(size: 13, weight: .semibold))
            }
        }
    }

    private var statusPill: some View {
        let (color, text): (Color, String) = {
            switch store.connection {
            case .attached(let s):
                return model.isLiveBackend ? (Color(hex: 0x22C55E), s)
                                           : (Color(hex: 0xE0AE09), "Offline demo")
            case .connecting: return (Color(hex: 0xE0AE09), "Connecting")
            case .failed: return (Color(hex: 0xEF4444), "Not connected")
            case .detached: return (p.secondary, "Detached")
            }
        }()
        return HStack(spacing: 6) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(text).font(.system(size: 11, weight: .medium)).foregroundStyle(p.secondary)
                .lineLimit(1).truncationMode(.middle)
                .frame(maxWidth: 220, alignment: .leading)
                .fixedSize(horizontal: true, vertical: false)
        }
        .padding(.horizontal, 9).frame(height: 22)
        .background(Capsule().fill(p.surface2))
        .overlay(Capsule().strokeBorder(p.border))
        .help(model.backendNote)
    }

    private var moreMenu: some View {
        Menu {
            if let bot {
                Button("Routines") { model.open(.routines(bot.id)) }
                Button("Take over the screen") { model.open(.takeover(bot.id)) }
                Button(pip.isOpen ? "Close the pop-out" : "Pop out the Agent Computer") {
                    model.togglePiP()
                }
                .disabled(model.session == nil)
                Button("Stop \(bot.name)") { model.stop(bot.id) }
                    .disabled(store.runID(for: bot.id) == nil)
                Divider()
            }
            Button("New Bot") { newBot() }
            Button("New Space\u{2026}") { model.creatingSpace = true }
            Button("New group chat\u{2026}") { model.creatingGroup = true }
                .disabled(store.bots.count < GroupChat.minBots)
            Divider()
            Picker("Appearance", selection: $model.desktopTheme) {
                ForEach(AppAppearance.allCases) { a in Text(a.label).tag(a.rawValue) }
            }
        } label: {
            Image(systemName: "ellipsis").font(.system(size: 14, weight: .medium))
                .frame(width: 28, height: 28)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(ShellLabels.more)
        .accessibilityLabel(ShellLabels.more)
    }

    private func iconButton(_ symbol: String, help: String, highlighted: Bool = false,
                            action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).font(.system(size: 14))
                .frame(width: 28, height: 28)
                .background(RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .fill(highlighted ? p.surface2 : .clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(p.text)
        .help(help)
        .accessibilityLabel(help)
    }

    // MARK: - Content

    @ViewBuilder private var content: some View {
        switch model.route {
        case .groupThread(let id):
            GroupChatPane(chatID: id, store: groups, bots: store.bots, p: p)
        case .routines(let id):
            ScrollView {
                RoutinesPanel(botID: id, botName: store.bot(id)?.name ?? id,
                              store: routines, dark: p.isDark, scale: 1.15)
                    .frame(maxWidth: Self.columnMax)
                    .padding(.top, 24)
                    .frame(maxWidth: .infinity)
            }
        default:
            conversation
        }
    }

    @ViewBuilder private var conversation: some View {
        let empty = messages.isEmpty && !(bot.map { store.isAwaitingReply($0.id) } ?? false)
        Group {
            if empty { emptyState } else if let bot { thread(bot) }
        }
    }

    static let emptyPrompt = "What should Koala work on?"

    private var emptyState: some View {
        GeometryReader { g in
            VStack(spacing: 22) {
                Spacer(minLength: 0)
                KoalaMark(size: 64)
                Text(bot.map { "What should \($0.name) work on?" } ?? Self.emptyPrompt)
                    .font(.system(size: 28, weight: .semibold))
                    .multilineTextAlignment(.center)
                composer(minLines: 2)
                    .frame(width: min(Self.columnMax - 60, g.size.width - 48))
                suggestions
                    .frame(width: min(Self.columnMax - 60, g.size.width - 48))
                Spacer(minLength: 0)
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity)
        }
    }

    /// A few starting points. Each one fills the composer; nothing is sent
    /// until the user presses send.
    private var suggestions: some View {
        HStack(spacing: 8) {
            ForEach(["Triage my inbox", "Research a topic", "Fill in a web form"], id: \.self) { s in
                Button { draft = s } label: {
                    Text(s).font(.system(size: 12, weight: .medium))
                        .padding(.horizontal, 12).frame(height: 28)
                        .background(Capsule().fill(p.bg))
                        .overlay(Capsule().strokeBorder(p.border))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .foregroundStyle(p.secondary)
            }
        }
    }

    private func thread(_ bot: Bot) -> some View {
        GeometryReader { g in
            let column = min(Self.columnMax, g.size.width - 48)
            VStack(spacing: 0) {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 18) {
                            ForEach(messages) { m in
                                TranscriptRow(message: m, theme: p.theme, columnWidth: column,
                                              onChoose: { text in
                                                  Task { await store.answer(text, to: bot.id) }
                                              },
                                              onDismissChoices: { store.dismissChoices(in: bot.id) })
                            }
                            if store.isAwaitingReply(bot.id) {
                                ShellTypingIndicator(theme: p.theme)
                                    .transition(.opacity)
                            }
                            Color.clear.frame(height: 1).id("bottom")
                        }
                        .frame(width: column)
                        .padding(.vertical, 24)
                        .frame(maxWidth: .infinity)
                    }
                    .onChange(of: messages.count) { _, _ in
                        withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("bottom") }
                    }
                    .onAppear { proxy.scrollTo("bottom") }
                }
                composer(minLines: 1)
                    .frame(width: column)
                    .padding(.bottom, 18)
            }
        }
    }

    // MARK: - Composer

    private func composer(minLines: Int) -> some View {
        let canSend = !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !chrome.creating
        return VStack(alignment: .leading, spacing: 8) {
            if let notice = model.lastRefusal {
                Label(notice, systemImage: "exclamationmark.circle.fill")
                    .font(.system(size: 12)).foregroundStyle(p.secondary)
                    .lineLimit(2)
            }
            VStack(alignment: .leading, spacing: 10) {
                TextField(bot.map { "Message \($0.name)" } ?? "Ask Koala anything",
                          text: $draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.system(size: 14))
                    .lineLimit(minLines...10)
                    .onSubmit(send)
                HStack(spacing: 6) {
                    Button { attachFiles() } label: {
                        Image(systemName: "paperclip").font(.system(size: 14))
                            .frame(width: 30, height: 30)
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(p.secondary)
                    .disabled(bot == nil || store.connection.spaceID == nil)
                    .help(bot == nil ? "Start a conversation to attach files" : ShellLabels.attach)
                    .accessibilityLabel(ShellLabels.attach)
                    agentMenu
                    Spacer(minLength: 0)
                    if chrome.creating { ProgressView().controlSize(.small) }
                    Button(action: send) {
                        Image(systemName: "arrow.up").font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(canSend ? p.onPrimary : p.secondary)
                            .frame(width: 32, height: 32)
                            .background(Circle().fill(canSend ? p.primaryFill : p.surface2))
                    }
                    .buttonStyle(.plain)
                    .disabled(!canSend)
                    .keyboardShortcut(.return, modifiers: [.command])
                    .help(ShellLabels.send)
                    .accessibilityLabel(ShellLabels.send)
                }
            }
            .padding(.leading, 16).padding(.trailing, 10).padding(.top, 14).padding(.bottom, 10)
            .background(RoundedRectangle(cornerRadius: 24, style: .continuous).fill(p.surface))
            .overlay(RoundedRectangle(cornerRadius: 24, style: .continuous).strokeBorder(p.border))
            .shadow(color: .black.opacity(p.isDark ? 0 : 0.04), radius: 8, y: 2)
        }
    }

    private var chrome: ShellState { model.shell }

    /// The agent and Space the message goes to. One agent harness today
    /// (`claude-code`), so the agent row is informational; the Space rows act.
    private var agentMenu: some View {
        Menu {
            Section("Agent") {
                Button { } label: { Label("Claude Code", systemImage: "checkmark") }
            }
            Section("Space") {
                Text(spaceLine)
                Button("New Space\u{2026}") { model.creatingSpace = true }
                if let bot {
                    Button("Open \(bot.name)\u{2019}s Computer") { model.open(.preview(bot.id)) }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "sparkles").font(.system(size: 12))
                Text("Claude Code").font(.system(size: 12, weight: .medium))
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold))
            }
            .foregroundStyle(p.secondary)
            .padding(.horizontal, 10).frame(height: 28)
            .background(Capsule().strokeBorder(p.border))
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(ShellLabels.agentMenu)
        .accessibilityLabel(ShellLabels.agentMenu)
    }

    // MARK: - Agent Computer pane

    private func computerPane(_ bot: Bot) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("\(bot.name)\u{2019}s Computer").font(.system(size: 13, weight: .semibold))
                Spacer()
                iconButton("xmark", help: ShellLabels.closeComputer) { model.open(.thread(bot.id)) }
            }
            .padding(.horizontal, 14)
            .frame(height: Self.headerHeight)
            Rectangle().fill(p.border).frame(height: 1)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if model.session != nil {
                        AgentScreen(source: model.screenSource, isInteractive: false,
                                    showsControls: false, fixtureScale: 0.3)
                            .aspectRatio(16.0 / 10.0, contentMode: .fit)
                            .presenceCursors(model.session, over: model.screenSource)
                            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .strokeBorder(p.border))
                            .overlay(alignment: .topTrailing) { pipButton }
                        HStack(spacing: 8) {
                            Text(DetailsStrings.screenCaption(bot.name))
                                .font(.system(size: 11)).foregroundStyle(p.secondary)
                            Spacer(minLength: 0)
                            if let session = model.session { PresenceAvatars(session: session) }
                        }
                    } else {
                        noLiveScreen
                    }
                    Button("Take over") { model.open(.takeover(bot.id)) }
                        .accessibilityLabel(ShellLabels.takeOver)
                        .controlSize(.regular)
                    if let session = model.session, let pips = model.windowPiPs,
                       let icons = model.windowIcons {
                        SpaceWindowList(session: session, pips: pips, icons: icons, p: p) {
                            model.toggleWindowPiP($0)
                        }
                    }
                    if let space = store.spaceID {
                        drops.zone(space: space, botID: bot.id)
                    }
                    Rectangle().fill(p.border).frame(height: 1).padding(.vertical, 4)
                    RoutinesPanel(botID: bot.id, botName: bot.name, store: routines,
                                  dark: p.isDark)
                        .padding(.horizontal, -14)
                }
                .padding(14)
            }
        }
        .background(p.bg)
    }

    /// The one PiP control on the stream: pops the desktop out into a
    /// floating panel (the same session, no second stream).
    private var pipButton: some View {
        Button { model.togglePiP() } label: {
            Image(systemName: pip.isOpen ? "pip.exit" : "pip.enter")
                .font(.system(size: 12, weight: .medium))
                .frame(width: 26, height: 22)
                .background(.black.opacity(0.55), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .foregroundStyle(.white)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(6)
        .help(pip.isOpen ? ShellLabels.popIn : ShellLabels.popOut)
        .accessibilityLabel(pip.isOpen ? ShellLabels.popIn : ShellLabels.popOut)
    }

    /// With no stream there is nothing to show, and the pane says so rather
    /// than showing a black box or a picture that looks live.
    private var noLiveScreen: some View {
        VStack(spacing: 10) {
            KoalaSleep(size: 84)
            Text(ShellLabels.noLiveScreenTitle).font(.system(size: 13, weight: .semibold))
            Text(ShellLabels.noLiveScreenBody)
                .font(.system(size: 12)).foregroundStyle(p.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 22).padding(.horizontal, 16)
        .frame(maxWidth: .infinity)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(p.surface2))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(p.border))
        .accessibilityElement(children: .combine)
    }

    // MARK: - Actions

    private func newBot() {
        guard !chrome.creating else { return }
        draft = ""
        model.open(.roster)
    }

    /// Send the draft. With no Bot selected this creates one first, then
    /// sends: typing into the new-conversation prompt is how a Bot is made.
    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !chrome.creating else { return }
        draft = ""
        if let bot {
            model.send(text, to: bot.id)
            return
        }
        Task {
            chrome.creating = true
            defer { chrome.creating = false }
            let name = String(text.split(separator: " ").prefix(4).joined(separator: " ")
                .prefix(32))
            if let id = await model.createBot(named: name.isEmpty ? BotStore.newBotName : name) {
                model.send(text, to: id)
            } else {
                draft = text
            }
        }
    }

    /// The composer's paperclip: the same pick-and-send as "Send file…".
    private func attachFiles() {
        guard let bot else { return }
        drops.pickFiles(for: bot.id)
    }
}

/// Every icon-only control's name, for tooltips, VoiceOver and automation.
enum ShellLabels {
    static let hideSidebar = "Hide sidebar"
    static let showSidebar = "Show sidebar"
    static let clearSearch = "Clear search"
    static let settings = "Settings"
    static let more = "More"
    static let attach = "Attach files"
    static let send = "Send"
    static let agentMenu = "Agent and Space"
    static let closeComputer = "Close the Agent Computer"
    static let takeOver = "Take over the screen"
    static let popOut = "Pop out the Agent Computer"
    static let popIn = "Pop in"
    static let popOutWindow = "Pop out this window"
    static let popOutUnavailable = "No live stream to pop out: no Space is attached"
    static let noLiveScreenTitle = "No live screen"
    static let noLiveScreenBody =
        "The app is not attached to a Space, so there is no stream to show. "
        + "Attach one to watch the Bot work."

    static let all = [hideSidebar, showSidebar, clearSearch, settings, more, attach, send,
                      agentMenu, closeComputer, takeOver, popOut, popOutWindow]
}

/// A sidebar button row: flat, with the hover colour behind it.
private struct SidebarButtonStyle: ButtonStyle {
    var p: KoalaPalette
    @State private var hovering = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(p.text)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(configuration.isPressed ? p.rowSelected : hovering ? p.rowHover : .clear))
            .onHover { hovering = $0 }
    }
}

// MARK: - Group chat, desktop

/// A group conversation at desktop density. The same store and messenger as
/// `GroupChatScreen`; this is only a layout.
struct GroupChatPane: View {
    let chatID: String
    @ObservedObject var store: GroupChatStore
    var bots: [Bot]
    var p: KoalaPalette
    @State private var draft = ""
    @State private var showingMembers = false

    private var chat: GroupChat? { store.chat(chatID) }
    private func bot(_ id: String) -> Bot? { bots.first { $0.id == id } }

    var body: some View {
        GeometryReader { g in
            let column = min(KoalaShell.columnMax, g.size.width - 48)
            VStack(spacing: 0) {
                if let chat {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 16) {
                            HStack {
                                Text(chat.membershipLabel).font(.system(size: 12))
                                    .foregroundStyle(p.secondary)
                                Spacer()
                                Button("Members\u{2026}") { showingMembers = true }
                                    .buttonStyle(.link)
                            }
                            ForEach(chat.messages) { m in row(m) }
                            ForEach(store.workingBots(in: chat.id), id: \.self) { id in
                                if let b = bot(id) {
                                    HStack(spacing: 8) {
                                        BotAvatar(bot: b, size: 20)
                                        ShellTypingIndicator(theme: p.theme)
                                    }
                                }
                            }
                        }
                        .frame(width: column).padding(.vertical, 24).frame(maxWidth: .infinity)
                    }
                } else {
                    Spacer()
                    Text("This group no longer exists.").foregroundStyle(p.secondary)
                    Spacer()
                }
                if let error = store.lastError {
                    Label(error, systemImage: "exclamationmark.circle.fill")
                        .font(.system(size: 12)).foregroundStyle(p.secondary)
                        .frame(width: column, alignment: .leading)
                }
                HStack(spacing: 8) {
                    TextField("Message the group", text: $draft, axis: .vertical)
                        .textFieldStyle(.plain).font(.system(size: 14)).lineLimit(1...8)
                        .onSubmit(send)
                    Button(action: send) {
                        Image(systemName: "arrow.up").font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(p.onPrimary)
                            .frame(width: 32, height: 32)
                            .background(Circle().fill(p.primaryFill))
                    }
                    .buttonStyle(.plain)
                    .help(ShellLabels.send)
                    .accessibilityLabel(ShellLabels.send)
                    .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding(.leading, 16).padding(.trailing, 10).padding(.vertical, 10)
                .background(RoundedRectangle(cornerRadius: 24, style: .continuous).fill(p.surface))
                .overlay(RoundedRectangle(cornerRadius: 24, style: .continuous).strokeBorder(p.border))
                .frame(width: column)
                .padding(.bottom, 18)
            }
        }
        .sheet(isPresented: $showingMembers) {
            if let chat {
                GroupMembersSheet(chat: chat, store: store, bots: bots) { showingMembers = false }
            }
        }
    }

    @ViewBuilder private func row(_ m: GroupMessage) -> some View {
        switch m.speaker {
        case .human:
            HStack {
                Spacer(minLength: 80)
                Text(m.text).font(.system(size: 14)).textSelection(.enabled)
                    .padding(.horizontal, 14).padding(.vertical, 9)
                    .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(p.surface2))
            }
        case .bot(let id):
            HStack(alignment: .top, spacing: 10) {
                if let b = bot(id) { BotAvatar(bot: b, size: 22) }
                VStack(alignment: .leading, spacing: 3) {
                    Text(bot(id)?.name ?? id).font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(p.secondary)
                    Text(m.text).font(.system(size: 14)).textSelection(.enabled)
                        .opacity(m.undelivered ? 0.6 : 1)
                }
            }
        case .system:
            Text(m.text).font(.system(size: 11)).foregroundStyle(p.secondary)
                .frame(maxWidth: .infinity)
        }
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        Task {
            await store.send(text, in: chatID)
            await store.collectReplies(in: chatID)
        }
    }
}
#endif
