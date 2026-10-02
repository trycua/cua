// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
import CuaSpacesTeleport
import SwiftUI
import UniformTypeIdentifiers

/// A Space: the live desktop, its facts, then Stream (one row per window,
/// each with its own stream and picture in picture), Agents (one row per
/// coding-agent run) and Teleport.
/// What shows and what is enabled is the core's `appSpaceDetail`.
struct SpaceDetailView: View {
    let model: AppModel
    let space: AppSpace
    @State private var provider: SpaceStreamSourceProviding?
    @State private var session: LiveStreamSession?
    @State private var pips: StreamPiPSet?
    @State private var streams: StreamRowsModel?
    @State private var teleport: TeleportModel?
    @State private var dropStatus: TeleportDropZone.Status?
    @State private var agents: AgentRunsModel?
    @State private var sharing: ShareModel?
    /// Connect (or Try again) was pressed: the stream opens even with
    /// "Connect to the desktop automatically" off.
    @State private var connectRequested = false
    @Environment(\.openWindow) private var openWindow
    private let copy = appSpaceDetailCopy()

    private var deleting: Bool { space.status == .deleting }
    /// Off, or being turned off: nothing streams from it.
    private var off: Bool { space.power.map { $0.off || $0.turningOn == false } ?? false }
    /// What the live parts (stream, polling) restart on.
    private var live: String { "\(space.id) \(deleting) \(off)" }

    var body: some View {
        let detail = model.detail(space)
        ScrollViewReader { scroller in
            Form {
                Section {
                    PreviewCard(session: detail.canStream ? session : nil) { preview(detail) }
                } footer: {
                    if let error = detail.powerError {
                        // Turning it off or on failed: why, inline.
                        Text(error)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("space-power-error")
                    }
                }
                Section {
                    FactRows(facts: detail.facts)
                }
                ForEach(detail.sections, id: \.self) { title in
                    Section(title) {
                        switch title {
                        case "Stream": streamRows
                        case "Agents":
                            if let agents { AgentRows(agents: agents, copy: copy) } else { Text(copy.agentsLoading).foregroundStyle(.secondary) }
                        default: teleportRow
                        }
                    }
                    .id("section-\(title)")
                }
            }
            .formStyle(.grouped)
            .task(id: space.id) {
                // Captures: the Stream section at the top of the window once
                // its windows are listed (bounded wait).
                guard let view = DevHooks.value("CUA_SPACES_START_VIEW"),
                      ["stream", "window-pip"].contains(view) else { return }
                for _ in 0..<120 where streams?.windows == nil {
                    try? await Task.sleep(for: .milliseconds(250))
                }
                try? await Task.sleep(for: .milliseconds(300))
                scroller.scrollTo("section-Stream", anchor: .top)
            }
        }
        .navigationTitle(detail.title)
        .toolbar { toolbar(detail) }
        // A delete stops the stream, its pop-outs and the polling at once
        // (and a failed one starts them again).
        .task(id: "\(live) \(connectRequested)") { await connect(detail) }
        // The cover's preview: the shared store's image at once, then one
        // no older than the background interval.
        .task(id: live) {
            guard detail.canStream else { return }
            await model.thumbnails.refresh(space.id, maxAge: TimeInterval(SpaceThumbnails.policy.backgroundIntervalMs) / 1000)
        }
        // Leaving: the last frame is the newest preview there is.
        .onDisappear { keepLastFrame() }
        .onChange(of: deleting) { _, now in if now { disconnect() } }
        .onChange(of: off) { _, now in if now { disconnect() } }
        .task(id: live) {
            guard detail.showSections else { return }
            await model.pollUsage(space.id)
        }
        .task(id: live) {
            guard detail.showSections else { return }
            let model = AgentRunsModel(backend: self.model.backend, spaceId: space.id)
            agents = model
            await model.poll()
        }
        .task(id: space.id) {
            // Captures: this Space's own window (its live desktop, with the
            // other participants' cursors), as Open does.
            guard DevHooks.value("CUA_SPACES_START_VIEW") == "space-window",
                  detail.canStream else { return }
            try? await Task.sleep(for: .seconds(1))
            openWindow(id: "space", value: space.id)
        }
        .task(id: space.id) {
            // Captures: the teleport picker open on this Space.
            guard DevHooks.value("CUA_SPACES_START_VIEW") == "teleport-picker",
                  detail.canStream else { return }
            try? await Task.sleep(for: .seconds(2))
            await openTeleport(entry: nil)
        }
        .task(id: model.pendingTeleport) {
            guard let p = model.pendingTeleport, p.spaceId == space.id else { return }
            model.pendingTeleport = nil
            await openTeleport(entry: p.entry, files: p.files)
        }
        .sheet(item: Binding(get: { teleport.map { TeleportSheetItem(model: $0) } },
                             set: { if $0 == nil { teleport = nil } })) { item in
            TeleportPickerSheet(teleport: item.model) { teleport = nil }
        }
        .sheet(item: Binding(get: { sharing.map { ShareSheetItem(model: $0) } },
                             set: { if $0 == nil { sharing = nil } })) { item in
            ShareSheetView(model: item.model) { sharing = nil }
        }
        .alert(detail.confirm.title, isPresented: Binding(
            get: { model.confirmDeleteId == space.id },
            set: { if !$0 { model.confirmDeleteId = nil } })) {
            Button(detail.confirm.confirmLabel, role: .destructive) { model.delete(space) }
                .disabled(!detail.confirm.confirmEnabled)
            // A Space in your cloud: forget it, keep it running there.
            if let remove = detail.confirm.removeLabel {
                Button(remove) { model.delete(space, removeOnly: true) }
            }
            Button(detail.confirm.cancelLabel, role: .cancel) {}
        } message: {
            Text([detail.confirm.message, detail.confirm.disabledReason].compactMap { $0 }.joined(separator: "\n\n"))
        }
    }

    @ViewBuilder private func preview(_ detail: AppSpaceDetail) -> some View {
        if let notice = detail.creditNotice, !detail.canStream {
            ZStack {
                Rectangle().fill(.quaternary)
                // Out of Cua Cloud credit: one line and Add credit (the
                // website billing page). Local Spaces never see this.
                HStack(spacing: 12) {
                    Text(notice.text).foregroundStyle(.secondary)
                    Button(notice.button) { model.openBillingPage(notice.url) }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("add-credit")
                }
            }
        } else {
            StreamPhaseReader(session: detail.canStream ? session : nil) { phase in
                let cover = model.cover(detail, requested: connectRequested, stream: phase)
                ZStack {
                    if let session, detail.canStream {
                        SpaceScreenView(session: session, isInteractive: true, showsControls: false)
                    }
                    if cover.kind != .stream {
                        DesktopCoverView(cover: cover, image: model.thumbnails[space.id],
                                         progress: detail.progress, progressText: detail.progressText) {
                            press(cover)
                        }
                        .transition(.opacity)
                    }
                }
                .animation(.easeOut(duration: 0.25), value: cover.kind)
            }
        }
    }

    /// The cover's button: Connect opens the stream; Try again starts the
    /// failed one again.
    private func press(_ cover: AppDesktopCover) {
        connectRequested = true
        guard cover.kind == .status, let session else { return }
        Task {
            await session.stop()
            await session.start()
        }
    }

    /// Keeps the stream's last frame as the Space's preview.
    private func keepLastFrame() {
        guard let frame = session?.frame, let image = SpaceThumbnails.image(frame) else { return }
        model.thumbnails.set(space.id, image)
    }

    @ViewBuilder private var streamRows: some View {
        if let session, let pips, let streams {
            StreamRows(model: streams, session: session, pips: pips)
        } else {
            Text(copy.streamLoading).foregroundStyle(.secondary)
        }
    }

    /// The shared drop well (CuaSpacesTeleport), the same one the Swift
    /// samples use: files are sent, an app opens the teleport review.
    private var teleportRow: some View {
        TeleportDropZone(
            spaceID: space.id, teleport: model.backend.teleportHandle(), status: dropStatus,
            caption: copy.dropCaption, sendFileTitle: copy.sendFile, teleportAppTitle: copy.teleportApp,
            symbol: copy.teleportSymbol, activeSymbol: copy.teleportSymbolActive,
            onFiles: { urls in Task { await send(urls.map(\.path)) } },
            onApp: { entry, files in Task { await openTeleport(entry: entry, files: files) } },
            onSendFile: { chooseFiles() },
            onTeleportApp: { Task { await openTeleport(entry: nil) } })
            .listRowInsets(EdgeInsets(top: 8, leading: 8, bottom: 8, trailing: 8))
    }

    private func chooseFiles() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        guard panel.runModal() == .OK else { return }
        let paths = panel.urls.map(\.path)
        Task { await send(paths) }
    }

    private func send(_ paths: [String]) async {
        guard !paths.isEmpty else { return }
        model.recordFeature("file_send")
        dropStatus = .working(appDropSendingText(paths: paths))
        do {
            let files = try await model.backend.sendFiles(id: space.id, paths: paths)
            dropStatus = .done(appDropSentText(files: files))
        } catch {
            dropStatus = .failed(LiveSpacesBackend.words(error))
        }
    }

    @ToolbarContentBuilder private func toolbar(_ detail: AppSpaceDetail) -> some ToolbarContent {
        ToolbarItemGroup {
            ForEach(Array(detail.actions.enumerated()), id: \.offset) { _, action in
                actionButton(action)
            }
        }
    }

    @ViewBuilder private func actionButton(_ action: AppDetailAction) -> some View {
        let run = { perform(action.id) }
        if action.id == .cancel {
            // A create still running: a plain "Cancel" (disabled while it
            // cleans up; the row and preview say "Cancelling…").
            Button(action.label, action: run)
                .help(action.help)
                .disabled(!action.enabled)
                .accessibilityIdentifier("space-cancel-create")
        } else if action.busy {
            // Its action runs (turning the Space off or on): a spinner.
            ProgressView()
                .controlSize(.small)
                .help(action.help)
                .accessibilityLabel(action.help)
        } else if let symbol = action.symbol {
            Button(role: action.destructive ? .destructive : nil, action: run) {
                Label(action.label, systemImage: symbol)
            }
            .help(action.help)
            .disabled(!action.enabled)
        } else {
            Button(action.label, action: run)
                .buttonStyle(.borderedProminent)
                .help(action.help)
                .disabled(!action.enabled)
                .accessibilityIdentifier("space-open")
        }
    }

    private func perform(_ id: AppDetailActionId) {
        switch id {
        case .teleport:
            model.recordFeature("teleport_app_picker")
            Task { await openTeleport(entry: nil) }
        case .pip: pips?.toggle(.desktop, sharing: session)
        case .share:
            sharing = ShareModel(
                backend: model.backend, spaceId: space.id, spaceName: space.name,
                signedIn: model.identity != nil,
                shareable: space.id.hasPrefix("relay:") || (space.sdk?.features.contains("relay_attach") ?? false))
            sharing?.telemetry = model.telemetrySink
            model.recordFeature("share_open")
        case .power:
            if let button = appPowerButton(space: space) { model.setPower(space, on: button.turnOn) }
        case .delete: model.confirmDeleteId = space.id
        case .open:
            model.recordFeature("space_open_viewer")
            openWindow(id: "space", value: space.id)
        case .cancel: model.cancelCreate(space.id)
        }
    }

    private func connect(_ detail: AppSpaceDetail) async {
        // The core decides: auto-connect on, or Connect pressed.
        guard session == nil,
              model.cover(detail, requested: connectRequested, stream: .noSession).openStream else { return }
        do {
            let provider = try await model.backend.streamProvider(id: space.id)
            self.provider = provider
            let session = LiveStreamSession(provider: provider)
            self.session = session
            let pips = StreamPiPSet(provider: provider)
            self.pips = pips
            streams = StreamRowsModel(space: space, provider: provider, backend: model.backend)
            // Captures: pop out the first window's own stream.
            if DevHooks.value("CUA_SPACES_START_VIEW") == "window-pip" {
                await session.refreshWindows()
                if let first = session.windows.first { pips.popOut(.window(first)) }
            }
        } catch {
            model.show(error: "Could not open \(space.name): \(LiveSpacesBackend.words(error))")
        }
    }

    /// Stops the live desktop, closes its picture-in-picture panels and
    /// forgets the window list (the Space is being deleted).
    private func disconnect() {
        keepLastFrame()
        connectRequested = false
        agents = nil
        pips?.popInAll()
        if let session { Task { await session.stop() } }
        session = nil
        pips = nil
        streams = nil
        provider = nil
    }

    private func openTeleport(entry: TeleportCatalogEntry?, files: [String] = []) async {
        do {
            let context = try await model.backend.teleportContext(id: space.id)
            let t = TeleportModel(spaceName: space.name, teleport: context?.0, space: context?.1)
            // The review reads saved Keyvault items and a browser's sites
            // from the Keyvault, and starts from what was sent last time.
            t.keyvault = model.keyvault
            t.rememberedChoices = model.settings.teleportChoices
            t.onRemember = { [weak model] choices in
                model?.settings.teleportChoices = choices
                model?.saveSettings()
            }
            // From <Space>: the window's own stream, here, in a floating panel.
            t.onStreamWindow = { [weak t] id in
                if let w = streams?.window(id: id) ?? session?.windows.first(where: { $0.id == id }) {
                    pips?.popOut(.window(w))
                }
                if t != nil { teleport = nil }
            }
            let notch = model.notch
            t.onTransfer = { running in
                notch.setActivity(hotspot: notch.state.hotspot,
                                  transfer: running ? AppNotchTransfer(sent: nil, total: nil) : nil)
            }
            // The sheet shows at once; the catalog fills it (cached, ms).
            teleport = t
            if let entry { t.preselect(entry, files: files) } else { await t.load() }
        } catch {
            model.show(error: LiveSpacesBackend.words(error))
        }
    }
}

/// The live desktop's card: the stream runs flush to the card's edges with
/// only the card's own corner radius (the website's Spaces window). The card
/// sizes to the stream's aspect ratio once it has a frame, else to the
/// default desktop's 16:10 (1280x800), so it does not jump when the stream
/// starts; nothing is letterboxed.
///
/// `listRowInsets` has no effect in a macOS grouped Form, so the row is a
/// clear spacer and the content an overlay grown by the row's inset to the
/// whole card, which clips it to its corner radius. (Grown as the row
/// itself, an AppKit-backed view such as the stream is cut off at the
/// row's top inset; as an overlay it is not.)
struct PreviewCard<Content: View>: View {
    /// A macOS grouped Form's row inset on every side.
    static var rowInset: CGFloat { 10 }

    var session: LiveStreamSession?
    @ViewBuilder var content: Content

    var body: some View {
        if let session { Observed(session: session, content: content) } else { Self.card(nil, content) }
    }

    static func card(_ size: CGSize?, _ content: Content) -> some View {
        let aspect = size.flatMap { $0.width > 0 && $0.height > 0 ? $0.width / $0.height : nil } ?? 16.0 / 10.0
        return CardSpacer(aspect: aspect, inset: rowInset) { Color.clear }
            .overlay { content.padding(-rowInset) }
    }

    private struct Observed: View {
        @ObservedObject var session: LiveStreamSession
        let content: Content
        var body: some View { PreviewCard.card(session.surfaceSize, content) }
    }
}

/// A row whose card (the row plus `inset` on every side) is `aspect` wide
/// over high.
struct CardSpacer: Layout {
    var aspect: CGFloat
    var inset: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 480
        return CGSize(width: width, height: max((width + 2 * inset) / aspect - 2 * inset, 0))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for view in subviews { view.place(at: bounds.origin, proposal: ProposedViewSize(bounds.size)) }
    }
}

/// The detail's facts, one line each: the full value (and an image's
/// digest) as the tooltip, a copy button after the Image and Identifier
/// values, and the core's warning symbol with its tooltip (an emulated
/// local Space's Architecture).
struct FactRows: View {
    let facts: [AppFact]
    var write: (String) -> Void = FactCopyModel.pasteboard
    /// Snapshots: draw the copy buttons in their "Copied" state.
    var copied = false

    var body: some View {
        ForEach(facts, id: \.label) { fact in
            LabeledContent(fact.label) {
                HStack(spacing: 5) {
                    Text(fact.value)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .help(fact.help ?? fact.value)
                    if let warning = fact.warning {
                        Image(systemName: warning.symbol)
                            .foregroundStyle(.yellow)
                            .imageScale(.small)
                            .help(warning.help)
                            .accessibilityLabel(warning.help)
                    }
                    if let copy = fact.copy {
                        CopyFactButton(model: FactCopyModel(copy: copy, copied: copied, write: write))
                    }
                }
            }
        }
    }
}

/// A fact's copy button: `doc.on.doc`, then the core's checkmark and
/// "Copied" for its `confirmMs`.
struct CopyFactButton: View {
    @StateObject var model: FactCopyModel

    var body: some View {
        Button { model.copy() } label: {
            Image(systemName: model.symbol)
                .contentTransition(.symbolEffect(.replace))
                .frame(width: 14, height: 14)
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help(model.help)
        .accessibilityLabel(model.help)
        .accessibilityIdentifier("copy-fact")
    }
}

/// The Stream section: the core's rows (`appStreamSection`, the same ones
/// the Tauri app draws), the Desktop and one per window, each with its
/// picture-in-picture button.
struct StreamRows: View {
    @ObservedObject var model: StreamRowsModel
    @ObservedObject var session: LiveStreamSession
    @ObservedObject var pips: StreamPiPSet

    var body: some View {
        StreamRowList(section: model.section(openKeys: pips.openKeys), image: model.image(for:)) { row in
            // What the button does is the core's: close the row's open
            // panel (pip.exit), else open one.
            let source: StreamSource? = row.kind == .desktop ? .desktop : model.window(id: row.id).map { .window($0) }
            guard let source else { return }
            switch appStreamPipClick(open: model.openRows(openKeys: pips.openKeys), row: row.id) {
            case .close: pips.popIn(source)
            case .open: pips.popOut(source, sharing: row.kind == .desktop ? session : nil)
            }
        }
        .task { await model.poll() }
    }
}

/// The rows themselves: one line each, the icon (the Space's OS mark for
/// the Desktop, the app's own icon or nothing for a window), the label
/// truncated with its full text as the tooltip, and the icon buttons.
struct StreamRowList: View {
    let section: AppStreamSection
    let image: (AppStreamRowIcon) -> NSImage?
    let perform: (AppStreamRow) -> Void

    var body: some View {
        ForEach(section.rows, id: \.id) { row in
            HStack(spacing: 8) {
                icon(row.icon)
                    .frame(width: 16, height: 16)
                Text(row.label)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(row.help)
                    .frame(maxWidth: .infinity, alignment: .leading)
                ForEach(row.actions, id: \.symbol) { action in
                    Button { perform(row) } label: {
                        Image(systemName: action.symbol)
                    }
                    .buttonStyle(.borderless)
                    .help(action.help)
                    .accessibilityLabel(action.help)
                    .accessibilityIdentifier(row.kind == .desktop ? "pip-desktop" : "pip-\(row.id)")
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("stream-row-\(row.id)")
        }
        if let status = section.statusText {
            Text(status).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder private func icon(_ icon: AppStreamRowIcon) -> some View {
        switch icon {
        case let .os(id):
            OsIconImage(id: id, size: 14).foregroundStyle(.secondary)
        case .app:
            // No icon for the app: nothing, never a stand-in glyph (the
            // empty slot keeps the labels aligned).
            if let image = image(icon) {
                Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
            } else {
                Color.clear
            }
        }
    }
}

/// The Agents section: one line per run (the agent and what it was asked),
/// its status on the right, the reason as the tooltip.
struct AgentRows: View {
    let agents: AgentRunsModel
    let copy: AppDetailCopy

    var body: some View {
        switch agents.load {
        case .loading:
            Text(copy.agentsLoading).foregroundStyle(.secondary)
        case .failed(let reason):
            Text(copy.agentsFailed).foregroundStyle(.secondary).help(reason)
        case .ready(let runs) where runs.isEmpty:
            Text(copy.agentsEmpty).foregroundStyle(.secondary)
        case .ready(let runs):
            ForEach(runs, id: \.runId) { run in
                LabeledContent {
                    Text(AgentRunsModel.status(run)).foregroundStyle(.secondary)
                } label: {
                    Text(AgentRunsModel.line(run)).lineLimit(1).truncationMode(.tail)
                }
                .help(run.reason)
                .accessibilityIdentifier("agent-run-\(run.runId)")
            }
        }
    }
}

struct TeleportSheetItem: Identifiable {
    let model: TeleportModel
    var id: ObjectIdentifier { ObjectIdentifier(model) }
}

/// A Space's live desktop in its own window ("Open").
struct SpaceWindowView: View {
    let model: AppModel
    let spaceId: String
    @State private var session: LiveStreamSession?
    @State private var error: String?

    var body: some View {
        let space = model.spaces.first { $0.id == spaceId }
        Group {
            if let error {
                ZStack {
                    Rectangle().fill(.quaternary)
                    Text(error).foregroundStyle(.secondary)
                }
            } else if let space {
                // Open is a request to connect: the same cover as the
                // detail's until the first frame.
                StreamPhaseReader(session: session) { phase in
                    let cover = model.cover(model.detail(space), requested: true, stream: phase)
                    ZStack {
                        if let session { SpaceScreenView(session: session, isInteractive: true, showsControls: true) }
                        if cover.kind != .stream {
                            DesktopCoverView(cover: cover, image: model.thumbnails[spaceId]) {
                                // Try again.
                                guard let session else { return }
                                Task {
                                    await session.stop()
                                    await session.start()
                                }
                            }
                        }
                    }
                }
            }
        }
        .frame(minWidth: 640, minHeight: 400)
        .navigationTitle(space?.name ?? spaceId)
        .onChange(of: space?.status == .deleting) { _, deleting in
            // Deleting: the stream stops now.
            guard deleting, let session else { return }
            self.session = nil
            Task { await session.stop() }
        }
        .task(id: spaceId) {
            do {
                session = LiveStreamSession(provider: try await model.backend.streamProvider(id: spaceId))
            } catch {
                self.error = LiveSpacesBackend.words(error)
            }
        }
    }
}

/// The Share sheet's identity for `.sheet(item:)`.
private struct ShareSheetItem: Identifiable {
    let model: ShareModel
    var id: String { model.spaceId }
}
