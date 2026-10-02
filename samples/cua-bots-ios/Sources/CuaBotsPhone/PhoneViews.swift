// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaBotsRemote
import CuaBotsUI
import SwiftUI

/// The phone app: your bots, a conversation with each, its profile, its
/// computer (which opens in your control), approvals and notifications.
/// Everything goes through `RemoteBot`, the bot's own Space over the relay.
public struct PhoneRoot: View {
    @ObservedObject var model: PhoneModel

    public init(model: PhoneModel) { self.model = model }

    public var body: some View {
        NavigationStack(path: $model.path) {
            BotsHome(model: model)
                .navigationDestination(for: PhoneModel.Screen.self) { screen in
                    switch screen {
                    case .chat(let id): if let bot = model.bot(id) { PhoneChat(bot: bot, model: model) }
                    case .computer(let id): if let bot = model.bot(id) { PhoneComputer(bot: bot) }
                    case .approvals: PhoneApprovals(model: model)
                    }
                }
        }
        .sheet(item: $model.profileFor) { ref in
            if let bot = model.bot(ref.id) { PhoneProfile(bot: bot, model: model) }
        }
        .overlay(alignment: .top) { PhoneBanner(model: model) }
    }
}

/// The phone's state: the bots it reaches and where it is.
@MainActor
public final class PhoneModel: ObservableObject {
    public enum Screen: Hashable { case chat(String), computer(String), approvals }
    public struct Ref: Identifiable { public var id: String
        public init(id: String) { self.id = id } }

    @Published public var bots: [RemoteBot] = []
    @Published public var path: [Screen] = []
    @Published public var profileFor: Ref?
    @Published public var banner: BotNotification?
    @Published public var connectionLabel = "Not connected"
    private var seen: Set<String> = []
    private var cancellables: [AnyObject] = []
    /// Called for each new notification (the app posts it to the system).
    public var onNotification: ((BotNotification, Bot) -> Void)?

    public init() {}

    public func bot(_ id: String) -> RemoteBot? { bots.first { $0.botID == id } }

    public func attach(_ bots: [RemoteBot], label: String) {
        self.bots = bots
        connectionLabel = label
        for b in bots {
            b.startRefreshing()
            let c = b.objectWillChange.sink { [weak self, weak b] _ in
                Task { @MainActor in
                    guard let self, let b else { return }
                    self.objectWillChange.send()
                    self.surface(b)
                }
            }
            cancellables.append(c as AnyObject)
        }
    }

    /// New notifications become a banner (and a system notification).
    func surface(_ b: RemoteBot) {
        guard let snap = b.snapshot else { return }
        let fresh = snap.notifications.filter { !$0.read && !seen.contains($0.id) }
        if seen.isEmpty && !snap.notifications.isEmpty {
            // First look: don't replay history as banners.
            seen.formUnion(snap.notifications.map(\.id))
            return
        }
        for n in fresh.reversed() {
            seen.insert(n.id)
            withAnimation(.spring(response: 0.2, dampingFraction: 0.9)) { banner = n }
            onNotification?(n, snap.bot)
        }
    }

    public var pendingApprovals: [(RemoteBot, ApprovalRequest)] {
        bots.flatMap { b in (b.snapshot?.approvals ?? []).filter { $0.state == .pending }.map { (b, $0) } }
    }
}

import Combine

// MARK: - Home

struct BotsHome: View {
    @ObservedObject var model: PhoneModel

    var body: some View {
        List {
            if !model.pendingApprovals.isEmpty {
                Section {
                    NavigationLink(value: PhoneModel.Screen.approvals) {
                        Label("\(model.pendingApprovals.count) waiting for you", systemImage: "checkmark.shield")
                    }
                }
            }
            Section {
                ForEach(model.bots, id: \.botID) { b in
                    BotRow(bot: b)
                        .contentShape(Rectangle())
                        .onTapGesture { model.path.append(.chat(b.botID)) }
                }
            } header: {
                Text("Your bots")
            } footer: {
                Text("Connected through \(model.connectionLabel). Create bots in Cua Bots on your Mac.")
            }
        }
        .navigationTitle("Cua Bots")
    }
}

struct BotRow: View {
    @ObservedObject var bot: RemoteBot

    var body: some View {
        HStack(spacing: 12) {
            if let s = bot.snapshot {
                KoalaAvatar(s.bot.avatar, mood: s.bot.mood).frame(width: 44, height: 44)
                VStack(alignment: .leading, spacing: 2) {
                    Text(s.bot.name).font(.headline)
                    Text(s.messages.last(where: { m in
                        if case .text = m.kind { return m.role != .system }
                        return false
                    })?.text ?? s.bot.status)
                        .font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                if s.bot.isPaused {
                    Image(systemName: "pause.circle").foregroundStyle(.secondary)
                } else if s.approvals.contains(where: { $0.state == .pending }) {
                    Circle().fill(Color.accentColor).frame(width: 8, height: 8)
                }
            } else {
                ProgressView()
                Text(bot.botID)
                Spacer()
            }
        }
        .padding(.vertical, 4)
    }
}

// MARK: - Chat

struct PhoneChat: View {
    @ObservedObject var bot: RemoteBot
    @ObservedObject var model: PhoneModel
    @State private var draft = ""

    var body: some View {
        let snap = bot.snapshot
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        ForEach(bot.messages) { m in row(m).id(m.id) }
                        if snap?.busy == true {
                            HStack(spacing: 6) {
                                ProgressView()
                                Text(snap?.bot.status ?? "").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    .padding(14)
                }
                .onChange(of: bot.messages.count) { _, _ in proxy.scrollTo(bot.messages.last?.id, anchor: .bottom) }
                .onAppear { proxy.scrollTo(bot.messages.last?.id, anchor: .bottom) }
            }
            if snap?.bot.isPaused == true {
                Button { Task { await bot.send(.resume) } } label: {
                    Text("Paused · Tap to resume").font(.callout).frame(maxWidth: .infinity).padding(8)
                }
                .buttonStyle(.plain)
                .background(.quaternary.opacity(0.5))
            }
            HStack(spacing: 8) {
                TextField("Message", text: $draft, axis: .vertical)
                    .lineLimit(1...5)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(Capsule().strokeBorder(.primary.opacity(0.15)))
                Button {
                    let t = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                    draft = ""
                    if !t.isEmpty { Task { await bot.send(.message(t)) } }
                } label: { Image(systemName: "arrow.up.circle.fill").font(.title) }
                .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
        }
        .toolbar {
            ToolbarItem(placement: .principal) {
                if let s = snap {
                    Button { model.profileFor = .init(id: bot.botID) } label: {
                        VStack(spacing: 0) {
                            KoalaAvatar(s.bot.avatar, mood: s.bot.mood).frame(width: 30, height: 30)
                            Text(s.bot.name).font(.caption.weight(.semibold))
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            ToolbarItem(placement: .primaryAction) {
                Button { model.path.append(.computer(bot.botID)) } label: { Image(systemName: "desktopcomputer") }
            }
        }
        .navigationTitle(snap?.bot.name ?? bot.botID)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .onAppear { Task { await bot.send(.markRead) } }
    }

    @ViewBuilder func row(_ m: ChatMessage) -> some View {
        let accent = bot.snapshot?.bot.avatar.color.accent ?? .accentColor
        let name = bot.snapshot?.bot.name ?? "Bot"
        switch m.kind {
        case .text: MessageRow(m, accent: accent)
        case .notice: MessageRow(ChatMessage(botID: m.botID, role: .system, text: m.text), accent: accent)
        case .approval(let id):
            if let a = bot.snapshot?.approvals.first(where: { $0.id == id }) {
                ApprovalCard(a, botName: name, onDecide: { ok in Task { await bot.send(.decide(approvalID: a.id, approve: ok)) } },
                             onOpenComputer: { model.path.append(.computer(bot.botID)) })
            }
        case .login(let site, let id):
            SignInCard(site: site, approval: bot.snapshot?.approvals.first { $0.id == id }) {
                // On the phone, signing in means taking over the bot's computer.
                model.path.append(.computer(bot.botID))
            }
        case .result(let title, let outputs):
            ResultCard(title: title, outputs: outputs) { _ in }
        }
    }
}

// MARK: - Profile

struct PhoneProfile: View {
    @ObservedObject var bot: RemoteBot
    @ObservedObject var model: PhoneModel
    @Environment(\.dismiss) var dismiss

    var body: some View {
        content
        #if os(macOS)
            .frame(width: 402, height: 800)  // the preview host shows the sheet at phone size
        #endif
    }

    var content: some View {
        NavigationStack {
            if let s = bot.snapshot {
                List {
                    Section {
                        VStack(spacing: 6) {
                            AvatarBadge(s.bot.avatar, mood: s.bot.mood, size: 96)
                            Text(s.bot.name).font(.title2.weight(.semibold))
                            Text(s.bot.isPaused ? "Paused" : s.bot.status).font(.subheadline).foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity)
                        .listRowBackground(Color.clear)
                    }
                    Section("Computers") {
                        Button { dismiss(); model.path.append(.computer(bot.botID)) } label: {
                            LabeledContent(s.bot.computerTitle, value: s.bot.isPaused ? "Paused" : "Connected")
                        }
                        LabeledContent("Your Mac", value: s.bot.hostAccess.isAllowed ? "Connected" : "Off")
                    }
                    let active = s.tasks.filter { $0.state == .inProgress }
                    let scheduled = s.tasks.filter { $0.schedule != nil }
                    let done = s.tasks.filter { $0.state == .completed }
                    if !active.isEmpty { Section("In progress") { ForEach(active) { TaskRow($0) } } }
                    if !scheduled.isEmpty { Section("Scheduled") { ForEach(scheduled) { TaskRow($0) } } }
                    if !done.isEmpty { Section("Completed") { ForEach(done.prefix(5)) { TaskRow($0) } } }
                    Section("Customize") {
                        NavigationLink("Memory") {
                            ScrollView { Text(s.memory).font(.callout.monospaced()).padding() }
                                .navigationTitle("Memory")
                        }
                        NavigationLink("Outputs") {
                            List(s.outputs, id: \.self) { Text(($0 as NSString).lastPathComponent) }
                                .navigationTitle("Outputs")
                        }
                    }
                }
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button { dismiss() } label: { Image(systemName: "xmark") }
                    }
                    ToolbarItem(placement: .primaryAction) {
                        Menu {
                            Button(s.bot.isPaused ? "Resume" : "Pause") {
                                Task { await bot.send(s.bot.isPaused ? .resume : .pause) }
                            }
                        } label: { Image(systemName: "ellipsis") }
                    }
                }
            }
        }
    }
}

// MARK: - Computer

/// The bot's screen, live. On the phone it opens in your control: taps are
/// clicks and the keyboard types into it.
struct PhoneComputer: View {
    @ObservedObject var bot: RemoteBot
    @State private var typing = ""

    var body: some View {
        let accent = bot.snapshot?.bot.avatar.color.accent ?? .gray
        VStack(spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 14).fill(accent.opacity(0.85))
                GeometryReader { geo in
                    if let frame = bot.frame {
                        let fit = Self.fit(bot.frameSize, geo.size)
                        Image(decorative: frame, scale: 1)
                            .resizable()
                            .frame(width: fit.width, height: fit.height)
                            .position(x: geo.size.width / 2, y: geo.size.height / 2)
                            .onTapGesture(coordinateSpace: .local) { p in
                                let x = (p.x - (geo.size.width - fit.width) / 2) / fit.width * bot.frameSize.width
                                let y = (p.y - (geo.size.height - fit.height) / 2) / fit.height * bot.frameSize.height
                                Task { await bot.tap(at: CGPoint(x: x, y: y)) }
                            }
                    } else {
                        ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
                .padding(10)
            }
            .aspectRatio(bot.frameSize.width > 0 ? bot.frameSize.width / bot.frameSize.height : 16 / 10,
                         contentMode: .fit)
            HStack {
                Text("You have control").font(.callout)
                Spacer()
                TextField("Type into it", text: $typing).textFieldStyle(.roundedBorder).frame(maxWidth: 170)
                    .onSubmit { let t = typing; typing = ""; Task { await bot.type(t) } }
            }
            Spacer()
        }
        .padding(14)
        .navigationTitle(bot.snapshot?.bot.computerTitle ?? "Computer")
        .task { await bot.startStream(maxDimension: 1024) }
        .onDisappear { Task { await bot.stopStream() } }
    }

    static func fit(_ content: CGSize, _ box: CGSize) -> CGSize {
        guard content.width > 0, content.height > 0 else { return box }
        let s = min(box.width / content.width, box.height / content.height)
        return CGSize(width: content.width * s, height: content.height * s)
    }
}

// MARK: - Approvals

struct PhoneApprovals: View {
    @ObservedObject var model: PhoneModel

    var body: some View {
        ScrollView {
            VStack(spacing: 12) {
                ForEach(model.pendingApprovals, id: \.1.id) { pair in
                    let (b, a) = pair
                    ApprovalCard(a, botName: b.snapshot?.bot.name ?? b.botID,
                                 onDecide: { ok in Task { await b.send(.decide(approvalID: a.id, approve: ok)) } })
                }
                if model.pendingApprovals.isEmpty {
                    Text("Nothing is waiting for you.").foregroundStyle(.secondary).padding(.top, 40)
                }
            }
            .padding(14)
        }
        .navigationTitle("Approvals")
    }
}

// MARK: - Banner

struct PhoneBanner: View {
    @ObservedObject var model: PhoneModel

    var body: some View {
        if let n = model.banner, let bot = model.bot(n.botID)?.snapshot?.bot {
            Button {
                model.path = [.chat(bot.id)]
                model.banner = nil
            } label: {
                HStack(spacing: 10) {
                    KoalaAvatar(bot.avatar, mood: n.kind == .approval ? .needsApproval : .done).frame(width: 34, height: 34)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(n.title).font(.subheadline.weight(.semibold))
                        Text(n.body).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                    Spacer()
                }
                .padding(12)
                .background(RoundedRectangle(cornerRadius: 18).fill(.regularMaterial))
                .shadow(color: .black.opacity(0.15), radius: 10, y: 4)
                .padding(.horizontal, 10)
            }
            .buttonStyle(.plain)
            .transition(.move(edge: .top))
            .task(id: n.id) {
                try? await Task.sleep(for: .seconds(5))
                if model.banner?.id == n.id { withAnimation { model.banner = nil } }
            }
        }
    }
}
