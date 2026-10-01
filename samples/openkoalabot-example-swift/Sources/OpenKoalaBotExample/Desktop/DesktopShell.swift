// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
import SwiftUI

/// Desktop palette. The greys are mid values chosen to sit between the
/// sidebar and the pane.
struct DesktopTheme {
    var pane: Color
    var sidebar: Color
    var divider: Color
    var selectedRow: Color
    var searchField: Color
    var botCard: Color
    var userBubble: Color
    var onUserBubble: Color
    var text: Color
    var secondary: Color
    var accent: Color
    var inset: Color
    /// Which of the two palettes this is. Added so views that are shared with
    /// the mobile surface (`RoutinesPanel`) can be told which ground they are
    /// sitting on. Not a colour, so no graded render can move because of it.
    var isDark: Bool = false

    static let dark = DesktopTheme(
        pane: Color(hex: 0x070707), sidebar: Color(hex: 0x111111),
        divider: Color(hex: 0x2E2E2E), selectedRow: Color(hex: 0x323232),
        searchField: Color(hex: 0x222222), botCard: Color(hex: 0x1F1F1F),
        userBubble: Color(hex: 0x353535), onUserBubble: Color(hex: 0xFAFAFA),
        text: Color(hex: 0xFAFAFA), secondary: Color(hex: 0x8A8A8A),
        accent: Color(hex: 0x3B82F6), inset: Color(hex: 0x282828), isDark: true)

    static let light = DesktopTheme(
        pane: Color(hex: 0xFCFCFC), sidebar: Color(hex: 0xF7F7F7),
        divider: Color(hex: 0xE4E4E4), selectedRow: Color(hex: 0xE0E0E0),
        searchField: Color(hex: 0xEAEAEA), botCard: Color(hex: 0xEDEDED),
        userBubble: Color(hex: 0x0A0A0A), onUserBubble: .white,
        text: Color(hex: 0x111111), secondary: Color(hex: 0x8A8A8A),
        accent: Color(hex: 0x2E7BE9), inset: Color(hex: 0xFDFDFD))
}

/// Desktop metrics, in points, for the 2560x1515 export canvas at 2x.
enum DT {
    static let sidebarW: CGFloat = 245
    static let rightPanelW: CGFloat = 279
    /// The transcript column is left-aligned inside the main pane with a small
    /// inset, not centred: the layout leaves a wide right gutter that the
    /// per-message hover actions live in.
    static let contentMax: CGFloat = 562
    static let contentInset: CGFloat = 11
    static let gutterActions: CGFloat = 40
    static let radius: CGFloat = 11
    static let rowH: CGFloat = 52
    static let titleBarH: CGFloat = 44
    static let composerH: CGFloat = 38
}

/// The desktop window: three columns. The right column is a slide-in, not an
/// overlay, and it stacks the Agent Computer preview above the Routines panel:
/// they are the same column, which is easy to get wrong.
struct DesktopShell: View {
    var theme: DesktopTheme = .dark
    /// The size this shell lays itself out at.
    ///
    /// The default is the graded canvas, and that default is what the export
    /// path uses: D1/D2/D3 are regression renders at exactly
    /// 1280x757.5, so `export` must keep proposing it. The *app* passes the
    /// window's real size instead, so the shell is laid out at whatever the
    /// user dragged the window to rather than rendered small and scaled up.
    /// Every metric inside is relative (`DT.sidebarW`, `DT.rightPanelW`, a
    /// flexible main pane), so a bigger canvas means a wider transcript and
    /// more visible roster rows: real layout, not magnification.
    ///
    /// See `FRICTION.md` §31 and §48.
    var canvas: CGSize = DS.desktopCanvas
    /// Whether the roster column and the transcript scroll.
    ///
    /// **Off on the export path, on in the app**, and that split is the whole
    /// point. `export` renders six fixture Bots and a nine-message thread into
    /// a 1280x757.5 canvas: nothing reaches the bottom of either column, so
    /// scrolling would be inert, but a `ScrollView` is not layout-neutral even
    /// when its content fits, and turning it on unconditionally moved all three
    /// graded desktop PNGs (measured, not assumed). The running app has
    /// thirty-odd live runs in the Space and an unbounded transcript, and
    /// without scrolling both columns overflowed their panes and painted over
    /// the chrome. So the graded still keeps the plain stack it was graded
    /// with, and the window gets the scrolling it needs. `FRICTION.md` §49.
    var scrolls: Bool = false
    var bot: Bot = Fixtures.bot("sales")
    /// Where the sidebar roster and the transcript come from.
    ///
    /// `FixtureDataSource` is the default, and `FixtureDataSource().thread(for:
    /// "sales")` **is** `Fixtures.salesThread`, so `DesktopShell(theme: .dark)`
    /// resolves to exactly the tree D1/D2/D3 rendered before this seam existed.
    /// The app passes the live `BotStore`.
    var source: BotDataSource = FixtureDataSource()
    /// An explicit transcript. `nil` means "ask the data source".
    var threadOverride: Thread? = nil
    var showRightPanel: Bool = true
    /// Where the right panel's Agent Computer preview gets its pixels, and
    /// (when `takeover` is on) the full-pane tier 3. `.fixture` is the
    /// export path and keeps D1/D2/D3 byte-identical.
    var screen: AgentScreenSource = .fixture
    /// Tier 3 on desktop: the stream takes the main pane. Not part of the
    /// export renders.
    var takeover: Bool = false
    /// Uploads started from `dropZone`, shown in the attachment tray.
    var intake: AttachmentIntake? = nil
    /// The one drop target for files, the same `SpaceDropZone` the
    /// Computer pane shows. Under the stream in the
    /// takeover, under the preview in the right panel. `nil` on the export
    /// path, which keeps D1/D2/D3 unchanged.
    var dropZone: SpaceDropZone? = nil
    /// Files the Bot produced, draggable back out to Finder via `download`.
    var artifacts: AgentArtifactExport? = nil
    var agentFiles: [String] = []
    /// Selecting a Bot in the sidebar. `nil` on the export path.
    var onSelect: ((Bot) -> Void)? = nil
    /// The sidebar `+`: hire a new Bot.
    var onHire: (() -> Void)? = nil
    /// The title-bar monitor button: escalate to the desktop takeover.
    var onToggleComputer: (() -> Void)? = nil
    /// Sending a composed message; non-`nil` makes the composer typeable.
    var onSend: ((String) -> Void)? = nil
    /// The last send that did not reach the Bot.
    var refusalNotice: String? = nil
    /// Mount point for the sibling Routines surface; see `routinesPanel`.
    var onCreateRoutine: (() -> Void)? = nil
    /// The live Routines surface. Non-nil replaces the inline empty state below
    /// the Agent Computer preview with the real `RoutinesPanel`.
    ///
    /// Injected rather than constructed here for one reason: the export path
    /// passes nothing and therefore still renders the inline empty state,
    /// character for character, which is what keeps D1/D2/D3 byte-identical.
    /// The empty state's two strings live in both branches.
    var routineStore: RoutineStore? = nil

    init(theme: DesktopTheme = .dark, canvas: CGSize = DS.desktopCanvas,
         scrolls: Bool = false, bot: Bot = Fixtures.bot("sales"),
         thread: Thread? = nil, source: BotDataSource = FixtureDataSource(),
         showRightPanel: Bool = true, screen: AgentScreenSource = .fixture,
         takeover: Bool = false, intake: AttachmentIntake? = nil,
         dropZone: SpaceDropZone? = nil,
         artifacts: AgentArtifactExport? = nil, agentFiles: [String] = [],
         onSelect: ((Bot) -> Void)? = nil, onHire: (() -> Void)? = nil,
         onToggleComputer: (() -> Void)? = nil, onSend: ((String) -> Void)? = nil,
         refusalNotice: String? = nil, onCreateRoutine: (() -> Void)? = nil,
         routineStore: RoutineStore? = nil) {
        self.theme = theme
        self.canvas = canvas
        self.scrolls = scrolls
        self.bot = bot
        self.threadOverride = thread
        self.source = source
        self.showRightPanel = showRightPanel
        self.screen = screen
        self.takeover = takeover
        self.intake = intake
        self.dropZone = dropZone
        self.artifacts = artifacts
        self.agentFiles = agentFiles
        self.onSelect = onSelect
        self.onHire = onHire
        self.onToggleComputer = onToggleComputer
        self.onSend = onSend
        self.refusalNotice = refusalNotice
        self.onCreateRoutine = onCreateRoutine
        self.routineStore = routineStore
    }

    /// The transcript column's width.
    ///
    /// `DT.contentMax` (562pt) is the column width of the 2560x1515 desktop
    /// export renders, so at the graded canvas this
    /// must resolve to exactly that, and it does: 1280 less the 245pt sidebar,
    /// the 279pt right panel, two dividers and the 11pt inset leaves 743pt, and
    /// the `min` picks 562.
    ///
    /// It is a `min` rather than a constant because the app's window goes down
    /// to 720pt. There, a hard 562pt column plus the sidebar is wider than the
    /// window: the main pane could not shrink to fit, the whole three-column
    /// `HStack` overflowed, and being centred it was clipped on *both* sides:
    /// the sidebar lost its avatars and the search field lost its first
    /// character. Which is the other half of what "the top bar is overlapping"
    /// looked like once the vertical overlap was gone. `FRICTION.md` §50.
    private var contentWidth: CGFloat {
        let chrome = DT.sidebarW + (showRightPanel ? DT.rightPanelW + 1 : 0) + 1 + DT.contentInset
        return max(200, min(DT.contentMax, canvas.width - chrome))
    }

    private var thread: Thread { threadOverride ?? source.thread(for: bot.id) }
    private var presence: BotPresence { source.presence(for: bot.id) }

    /// Whether the human holds the keyboard and mouse of the streamed Space.
    @State private var hasControl = false
    @State private var typed: String = ""

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            Rectangle().fill(theme.divider).frame(width: 1)
            mainPane
            if showRightPanel {
                Rectangle().fill(theme.divider).frame(width: 1)
                rightPanel
            }
        }
        .background(theme.pane)
        .frame(width: canvas.width, height: canvas.height)
        .clipped()
    }

    // MARK: Sidebar

    private var sidebar: some View {
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "sidebar.left")
                    .font(.system(size: 14)).foregroundStyle(theme.secondary)
                Spacer()
                Image(systemName: "plus")
                    .font(.system(size: 15, weight: .medium)).foregroundStyle(theme.text)
                    .modifier(OptionalTap(action: onHire))
            }
            .padding(.horizontal, 14).padding(.top, 14).padding(.bottom, 12)

            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 12)).foregroundStyle(theme.secondary)
                Text("Search").font(DS.font(12)).foregroundStyle(theme.secondary)
                Spacer()
            }
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(theme.searchField))
            .padding(.horizontal, 12)

            OptionalScroll(scrolls: scrolls) {
                VStack(spacing: 2) {
                    ForEach(source.bots) { b in
                        rosterRow(b, selected: b.id == bot.id)
                            .modifier(OptionalTap(action: onSelect.map { pick in { pick(b) } }))
                    }
                }
                .padding(.horizontal, 8)
                .padding(.top, 12)
            }

            Spacer(minLength: 0)

            VStack(spacing: 2) {
                dockRow(icon: "powerplug", label: "Plugins")
                HStack(spacing: 10) {
                    Circle().fill(theme.secondary.opacity(0.45)).frame(width: 22, height: 22)
                    Text("Alex").font(DS.font(13, .medium)).foregroundStyle(theme.text)
                    Spacer()
                }
                .padding(.horizontal, 8).frame(height: 34)
            }
            .padding(.horizontal, 8).padding(.bottom, 10)
        }
        .frame(width: DT.sidebarW)
        .background(theme.sidebar)
    }

    private func dockRow(icon: String, label: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon).font(.system(size: 14)).foregroundStyle(theme.secondary)
                .frame(width: 22)
            Text(label).font(DS.font(13, .medium)).foregroundStyle(theme.text)
            Spacer()
        }
        .padding(.horizontal, 8).frame(height: 32)
    }

    private func rosterRow(_ b: Bot, selected: Bool) -> some View {
        HStack(spacing: 10) {
            BotAvatar(bot: b, size: 26)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text(b.name).font(DS.font(12, .semibold)).foregroundStyle(theme.text)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Text(b.timestamp).font(DS.font(10)).foregroundStyle(theme.secondary)
                }
                Text(b.preview).font(DS.font(11)).foregroundStyle(theme.secondary).lineLimit(1)
            }
        }
        .padding(.horizontal, 8)
        .frame(height: DT.rowH)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .fill(selected ? theme.selectedRow : .clear))
    }

    // MARK: Main pane

    private var mainPane: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                BotAvatar(bot: bot, size: 20)
                Text(bot.name).font(DS.font(14, .semibold)).foregroundStyle(theme.text)
                Spacer()
                Image(systemName: "display").font(.system(size: 15)).foregroundStyle(theme.text)
                    .modifier(OptionalTap(action: onToggleComputer))
            }
            .padding(.horizontal, 16)
            .frame(height: DT.titleBarH)

            if takeover {
                desktopTakeover
            } else {
                OptionalScroll(scrolls: scrolls) {
                    HStack(alignment: .top, spacing: 0) {
                        VStack(spacing: 8) {
                            ForEach(Array(thread.messages.enumerated()), id: \.element.id) { i, m in
                                DesktopMessageRow(message: m, theme: theme,
                                                  showActions: i == 1)
                            }
                        }
                        .frame(width: contentWidth)
                        Spacer(minLength: 0)
                    }
                    .padding(.leading, DT.contentInset)
                }

                // Only on the export path. When the transcript scrolls it is
                // already the flexible element and this would fight it for the
                // pane's height.
                if !scrolls { Spacer(minLength: 0) }

                if let intake {
                    AttachmentTray(intake: intake, onSurface: theme.botCard,
                                   textColor: theme.text, secondary: theme.secondary)
                        .frame(width: contentWidth)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.leading, DT.contentInset)
                        .padding(.bottom, 8)
                }

                composer
            }
        }
        .frame(maxWidth: .infinity)
    }

    /// Desktop tier 3. The desktop takeover and the take/return-control
    /// buttons are not part of the export renders.
    private var desktopTakeover: some View {
        VStack(spacing: 0) {
            AgentScreen(source: screen, isInteractive: hasControl, showsControls: false)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let dropZone {
                dropZone.padding(.horizontal, 14).padding(.top, 10)
            }
            HStack(spacing: 10) {
                Circle().fill(hasControl ? Color(hex: 0x18BE4B) : Color(hex: 0x8B5CF6))
                    .frame(width: 8, height: 8)
                Text(hasControl ? "You have control of \(bot.name)'s screen"
                                : "\(bot.name) has control")
                    .font(DS.font(11)).foregroundStyle(theme.secondary)
                Spacer()
                Button(hasControl ? "Return control to \(bot.name)" : "Take control") {
                    hasControl.toggle()
                }
                .font(DS.font(11, .medium))
            }
            .padding(.horizontal, 14)
            .frame(height: 36)
        }
    }

    /// The bottom composer. The placeholder is `Message <Bot>`, the desktop
    /// string, distinct from mobile's `Ask <Bot>`.
    private func sendTyped() {
        let t = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        typed = ""
        onSend?(t)
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 6) {
            // A refusal is shown under the composer, not swallowed.
            if let refusalNotice {
                HStack(spacing: 6) {
                    Image(systemName: "xmark.octagon.fill")
                        .font(.system(size: 11)).foregroundStyle(Color(hex: 0xF4234B))
                    Text(refusalNotice).font(DS.font(11)).foregroundStyle(theme.text)
                }
                .frame(width: contentWidth, alignment: .leading)
            } else if let hint = onSend != nil ? presence.refusalHint : nil {
                HStack(spacing: 6) {
                    Image(systemName: "exclamationmark.circle.fill")
                        .font(.system(size: 11)).foregroundStyle(Color(hex: 0xE0AE09))
                    Text(hint).font(DS.font(11)).foregroundStyle(theme.secondary)
                }
                .frame(width: contentWidth, alignment: .leading)
            }

            // This shell is reachable in the app (the takeover route), so the
            // same rule applies here as in `CuaShell`: the `plus.circle.fill`
            // and `mic.circle.fill` glyphs that used to bracket this field had
            // no actions and are gone. The send button is wired.
            HStack(spacing: 8) {
                if onSend != nil {
                    TextField("Message \(bot.name)", text: $typed)
                        .textFieldStyle(.plain)
                        .font(DS.font(12))
                        .foregroundStyle(theme.text)
                        .onSubmit(sendTyped)
                } else {
                    Text("Message \(bot.name)").font(DS.font(12)).foregroundStyle(theme.secondary)
                }
                Spacer()
                if onSend != nil {
                    let canSend = !typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ZStack {
                        Circle().fill(canSend ? theme.text : theme.secondary.opacity(0.35))
                        Image(systemName: "arrow.up")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(canSend ? theme.pane : theme.secondary)
                    }
                    .frame(width: 22, height: 22)
                    .contentShape(Circle())
                    .onTapGesture { if canSend { sendTyped() } }
                    .help("Send")
                    .accessibilityLabel("Send")
                }
            }
            .padding(.horizontal, 8)
            .frame(height: DT.composerH)
            .background(RoundedRectangle(cornerRadius: 19, style: .continuous).fill(theme.searchField))
        }
        .frame(width: contentWidth)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.leading, DT.contentInset)
        .padding(.bottom, 13)
    }

    // MARK: Right panel: preview stacked over Routines

    private var rightPanel: some View {
        VStack(spacing: 0) {
            VStack(spacing: 8) {
                // Tier 2 on desktop. A preview, so no input is forwarded.
                AgentScreen(source: screen, isInteractive: false,
                            showsControls: false, fixtureScale: 0.3)
                    .aspectRatio(16.0 / 10.0, contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .streamWindowDropTarget(screen)
                Text("\(bot.name)'s screen")
                    .font(DS.font(11)).foregroundStyle(theme.secondary)
                if let dropZone { dropZone.padding(.top, 4) }
            }
            .padding(12)

            // The drag source for window drag-and-drop, and the drag source for
            // pulling a produced file back out. Both appear only when a live
            // Space is behind the panel, so D1/D2/D3 are unchanged.
            if let session = screen.session {
                // Capped. A real Space has dozens of windows, and an uncapped
                // list is greedy in a `VStack`: it takes the whole column,
                // squeezes the tier-2 preview above it to nothing and pushes
                // Routines off the bottom. Live-only, so D1/D2/D3 never see it.
                StreamWindowList(session: session, theme: theme)
                    // A hard height, not a maximum, and clipped: see the note
                    // in `StreamWindowList`. Live-only, so D1/D2/D3 never
                    // reach this branch.
                    .frame(height: 190)
                    .clipped()
                    .padding(.horizontal, 12)
            }
            if let artifacts, !agentFiles.isEmpty {
                agentFileList(artifacts)
                    .padding(.horizontal, 12)
                    .padding(.top, 10)
            }

            Spacer(minLength: 0)

            if let routineStore {
                // Bounded and clipped for the same reason the window list is:
                // a Bot with a dozen routines must not grow the panel through
                // the bottom of the column.
                ScrollView {
                    RoutinesPanel(botID: bot.id, botName: bot.name,
                                  store: routineStore,
                                  dark: theme.isDark, scale: 1)
                }
                .frame(maxHeight: 300)
                .clipped()
                .padding(.bottom, 20)
            } else {
            VStack(spacing: 12) {
                Text("Routines are recurring tasks this\nagent runs on a schedule.")
                    .font(DS.font(11))
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .foregroundStyle(theme.secondary)
                // Mount point for the Routines surface. The empty state and its
                // two strings ship here; the surface
                // the button opens is a separate workstream, so the handler is
                // injected rather than implemented inline.
                Text("Create Routine")
                    .font(DS.font(12, .medium))
                    .foregroundStyle(theme.text)
                    .padding(.horizontal, 16).frame(height: 30)
                    .background(Capsule().stroke(theme.divider, lineWidth: 1))
                    .modifier(OptionalTap(action: onCreateRoutine))
            }
            .padding(.horizontal, 18)
            .padding(.bottom, 90)
            }

            Spacer(minLength: 0)
        }
        .frame(width: DT.rightPanelW)
        .background(theme.pane)
    }

    /// Files the Bot produced inside the Space, draggable out to Finder.
    ///
    /// Each row fetches itself with `download` when it appears, because
    /// `NSItemProvider` has to be handed a real local URL the instant the drag
    /// starts and there is no way to promise one across an async MCP call. A
    /// row that has not landed yet says so rather than handing Finder a path
    /// with nothing behind it.
    private func agentFileList(_ artifacts: AgentArtifactExport) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Files from \(bot.name)")
                .font(DS.font(10, .semibold)).foregroundStyle(theme.secondary)
            ForEach(agentFiles, id: \.self) { path in
                let ready = artifacts.isReady(path)
                HStack(spacing: 6) {
                    Image(systemName: ready ? "doc.fill" : "arrow.down.circle")
                        .font(.system(size: 9)).foregroundStyle(theme.secondary)
                    Text((path as NSString).lastPathComponent)
                        .font(DS.font(10)).foregroundStyle(theme.text).lineLimit(1)
                    Spacer(minLength: 0)
                    if !ready {
                        Text("fetching…").font(DS.font(9)).foregroundStyle(theme.secondary)
                    }
                }
                .padding(.horizontal, 6).padding(.vertical, 4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(theme.botCard))
                .contentShape(Rectangle())
                .onDrag { artifacts.itemProvider(path) }
                .task { await artifacts.prepare(path) }
            }
        }
    }
}

/// Desktop transcript row. Bot output is a full-column card; the user's message
/// is a shrink-to-fit bubble flush to the column's right edge. No avatars.
struct DesktopMessageRow: View {
    var message: Message
    var theme: DesktopTheme
    /// Reaction / reply / overflow, floated in the gutter opposite the bubble.
    var showActions: Bool = false

    var body: some View {
        switch message.body {
        case .systemEvent(let t):
            Text(t).font(DS.font(11)).foregroundStyle(theme.secondary)
                .frame(maxWidth: .infinity, alignment: .center)

        case .prose(let t):
            if showActions && message.sender == .bot {
                HStack(alignment: .center, spacing: 0) {
                    prose(t)
                    HStack(spacing: 10) {
                        ForEach(["face.smiling", "arrowshape.turn.up.left", "ellipsis"], id: \.self) { g in
                            Image(systemName: g).font(.system(size: 11))
                                .foregroundStyle(theme.secondary)
                        }
                    }
                    .padding(.leading, 12)
                }
            } else if message.sender == .user {
                HStack {
                    Spacer(minLength: 60)
                    Text(t).font(DS.font(12)).foregroundStyle(theme.onUserBubble)
                        .padding(.horizontal, 14).padding(.vertical, 10)
                        .background(RoundedRectangle(cornerRadius: DT.radius, style: .continuous)
                            .fill(theme.userBubble))
                }
            } else {
                prose(t)
            }

        case .card(let c):
            card {
                Text(c.title).font(DS.font(12, .bold)).foregroundStyle(theme.text)
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(c.bodyLines.enumerated()), id: \.offset) { _, l in
                        Text(l).font(DS.font(12)).foregroundStyle(theme.text)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(10)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(theme.inset))
                HStack(spacing: 8) {
                    btn(c.primary, filled: true); btn(c.secondary, filled: false); Spacer()
                }
            }

        case .approval(let a):
            card {
                HStack(spacing: 8) {
                    Image(systemName: "lock.fill").font(.system(size: 12)).foregroundStyle(theme.text)
                    Text(a.title).font(DS.font(12, .bold)).foregroundStyle(theme.text)
                }
                Text(a.detail).font(DS.font(12)).foregroundStyle(theme.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                // Order and wording come from `Approval`, not from literals
                // here, so the defined order cannot drift away from what is
                // drawn.
                HStack(spacing: 8) {
                    ForEach(Array((a.offersAlwaysAllow
                                   ? Approval.desktopOrder
                                   : Approval.localCommandOrder).enumerated()), id: \.offset) { i, answer in
                        btn(answer.rawValue, filled: i == 0)
                    }
                    Spacer()
                }
            }

        case .linkFile(let lf):
            card {
                HStack(spacing: 10) {
                    FileGlyph(kind: lf.kind).frame(width: 22, height: 27)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(lf.title).font(DS.font(12, .semibold)).foregroundStyle(theme.text)
                        Text(lf.subtitle).font(DS.font(11)).foregroundStyle(theme.secondary)
                    }
                    Spacer()
                }
            }

        case .activity(let group):
            ActivityGroupRow(group: group, theme: theme)
                .padding(.horizontal, 14)

        case .computerStatus(let cs):
            HStack(spacing: 8) {
                Circle().fill(cs.active ? Color(hex: 0x8B5CF6) : theme.secondary)
                    .frame(width: 7, height: 7)
                Text(cs.text).font(DS.font(11, .medium)).foregroundStyle(theme.secondary)
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(theme.secondary)
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 4)

        case .choices:
            // **Unreachable from fixtures, and deliberately empty.**
            //
            // The choice card belongs to the live shell; this view renders the
            // fixed export layout, and D1/D2/D3 are checked byte for byte. No
            // fixture thread contains a `.choices` body, so this branch never
            // renders, but Swift requires it, and drawing a card here would add
            // an element the export layout does not have. The live shell
            // (`TranscriptRow`) is where it renders.
            EmptyView()
        }
    }

    private func prose(_ t: String) -> some View {
        Text(t).font(DS.font(12)).foregroundStyle(theme.text)
            .lineSpacing(3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.vertical, 12)
            .background(RoundedRectangle(cornerRadius: DT.radius, style: .continuous)
                .fill(theme.botCard))
    }

    @ViewBuilder private func card<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 10) { content() }
            .padding(.horizontal, 14).padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: DT.radius, style: .continuous)
                .fill(theme.botCard))
    }

    private func btn(_ t: String, filled: Bool) -> some View {
        Text(t).font(DS.font(12, .medium))
            .foregroundStyle(filled ? theme.onUserBubble : theme.text)
            .padding(.horizontal, 14).frame(height: 28)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(filled ? theme.userBubble : theme.inset))
    }
}
