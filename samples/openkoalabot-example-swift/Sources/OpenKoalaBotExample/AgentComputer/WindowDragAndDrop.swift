// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
#if canImport(AppKit)
import AppKit
import SwiftUI
import UniformTypeIdentifiers

// MARK: - The payload

/// Dragging a window from the Space's window list into a stream pane.
///
/// The payload is a *value*, not a reference to a row in some list, because the
/// drop can land in a pane that does not share the list's state: the PiP panel
/// is a separate `NSWindow` with its own view tree. So the whole
/// `StreamWindow` travels on the pasteboard.
///
/// The epoch travels with it. A target handle is only valid with its current
/// generation, and reusing a stale pair is rejected as `stale_target`
/// (`Streaming/FRICTION.md` §1 documents how misleading that code is). Dropping
/// a window that was enumerated minutes ago must therefore be able to notice
/// that the live list has moved on, which is what `resolve` below does.
enum StreamWindowDrag {
    /// A private, in-process type. Not `public.file-url` and not plain text:
    /// a stream pane must not accept a stray text drag from another app, and a
    /// file dropped on the same pane means something completely different
    /// (an upload).
    static let typeIdentifier = "com.cua.openkoalabots.stream-window"
    static var pasteboardType: NSPasteboard.PasteboardType {
        NSPasteboard.PasteboardType(typeIdentifier)
    }

    /// Unit separator. Window titles contain `|`, `-`, tabs and newlines
    /// (`lume - watch.command - tail -f out.log - 120×30` is a real title from
    /// the live Space), so the separator has to be a character a title cannot
    /// hold.
    private static let separator = "\u{1F}"

    static func encode(_ window: StreamWindow) -> String {
        [window.id, String(window.epoch), window.app, window.title].joined(separator: separator)
    }

    static func decode(_ text: String) -> StreamWindow? {
        let parts = text.components(separatedBy: separator)
        guard parts.count == 4, !parts[0].isEmpty, let epoch = UInt64(parts[1]) else { return nil }
        return StreamWindow(id: parts[0], app: parts[2], title: parts[3], epoch: epoch)
    }

    static func data(for window: StreamWindow) -> Data {
        Data(encode(window).utf8)
    }

    static func window(from data: Data) -> StreamWindow? {
        String(data: data, encoding: .utf8).flatMap(decode)
    }

    /// What a drop should actually select.
    ///
    /// Prefers the entry in the session's *current* window list over the one
    /// that was dragged: same handle, but a freshly enumerated epoch and real
    /// geometry. Falls back to the dragged value when the list has not been
    /// refreshed since (a pane can be dropped on before its `refreshWindows()`
    /// has returned), because refusing the drop in that case would make the
    /// gesture feel unreliable for no benefit: a stale epoch surfaces as a
    /// clear `stale_target` failure on the session instead.
    static func resolve(_ dragged: StreamWindow, against live: [StreamWindow]) -> StreamSource {
        if let current = live.first(where: { $0.id == dragged.id }) {
            return .window(current)
        }
        return .window(dragged)
    }

    static func itemProvider(for window: StreamWindow) -> NSItemProvider {
        let provider = NSItemProvider()
        let payload = data(for: window)
        provider.registerDataRepresentation(forTypeIdentifier: typeIdentifier,
                                            visibility: .ownProcess) { completion in
            completion(payload, nil)
            return nil
        }
        return provider
    }
}

// MARK: - The list you drag from

/// The Space's windows, as draggable rows.
///
/// Drawn as a plain list because its whole job is to be a drag source.
struct StreamWindowList: View {
    @ObservedObject var session: LiveStreamSession
    var theme: DesktopTheme = .dark

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text("Windows").font(DS.font(10, .semibold)).foregroundStyle(theme.secondary)
                Spacer()
                Button {
                    Task { await session.refreshWindows() }
                } label: {
                    Image(systemName: "arrow.clockwise").font(.system(size: 9))
                }
                .buttonStyle(.plain)
                .foregroundStyle(theme.secondary)
            }
            if session.windows.isEmpty {
                Text("No windows listed yet.")
                    .font(DS.font(10)).foregroundStyle(theme.secondary)
            }
            // The rows **scroll**, and that is not cosmetic.
            //
            // This was a bare `ForEach` in a `VStack`, with the call site
            // capping it at `.frame(maxHeight: 190)`. A frame modifier only
            // changes the size *proposed* to a `VStack`; the stack still lays
            // its children out at their ideal heights and draws straight
            // through the bottom of the frame. On the demo Space, which has
            // around eighty Terminal windows open, the list drew over the
            // Routines panel below it and the tier-2 preview above it, both
            // illegible. Caught by running the app, not by building it.
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    rows
                }
            }
        }
    }

    @ViewBuilder private var rows: some View {
            ForEach(session.windows) { window in
                HStack(spacing: 6) {
                    Image(systemName: "macwindow").font(.system(size: 9))
                        .foregroundStyle(theme.secondary)
                    Text(window.displayName)
                        .font(DS.font(10)).foregroundStyle(theme.text)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 6).padding(.vertical, 4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(isSelected(window) ? theme.selectedRow : .clear))
                .contentShape(Rectangle())
                .onDrag { StreamWindowDrag.itemProvider(for: window) }
            }
    }

    private func isSelected(_ window: StreamWindow) -> Bool {
        if case let .window(current) = session.source { return current.id == window.id }
        return false
    }
}

// MARK: - The pane you drop onto

/// Accepts a dragged window and retargets the pane's session at it.
///
/// A no-op on a `.fixture` source: the export screens have nothing to
/// retarget, and quietly refusing the drop is better than pretending.
struct StreamWindowDropTarget: ViewModifier {
    var source: AgentScreenSource
    @State private var isTargeted = false

    @ViewBuilder
    func body(content: Content) -> some View {
        if source.isLive {
            live(content)
        } else {
            // Not merely an optimisation. An *inert* `.onDrop` (one that can
            // never fire, on a source with nothing to retarget) still changed
            // the exported PNGs of the three screens it was attached to, by
            // laying out a stray copy of the content above it. The tier views
            // are rendered offscreen by `ImageRenderer` for the rubric export,
            // and adding an interaction modifier to a view inside an
            // `ImageRenderer` is evidently not free. See `FRICTION.md`.
            content
        }
    }

    private func live(_ content: Content) -> some View {
        content
            .onDrop(of: [StreamWindowDrag.typeIdentifier], isTargeted: $isTargeted) { providers in
                guard let session = source.session,
                      let provider = providers.first else { return false }
                provider.loadDataRepresentation(forTypeIdentifier: StreamWindowDrag.typeIdentifier) { data, _ in
                    guard let data, let dragged = StreamWindowDrag.window(from: data) else { return }
                    Task { @MainActor in
                        await session.select(StreamWindowDrag.resolve(dragged, against: session.windows))
                    }
                }
                return true
            }
            .overlay {
                if isTargeted {
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(Color(hex: 0x8B5CF6), style: StrokeStyle(lineWidth: 3))
                        .background(Color(hex: 0x8B5CF6).opacity(0.14))
                        .allowsHitTesting(false)
                }
            }
    }
}

extension View {
    /// Make this stream pane a drop destination for windows.
    func streamWindowDropTarget(_ source: AgentScreenSource) -> some View {
        modifier(StreamWindowDropTarget(source: source))
    }
}

// MARK: - The PiP panel you drop onto

/// Makes the picture-in-picture panel accept a dragged window too.
///
/// `Streaming/StreamPiPController` owns the panel and exposes no seam for
/// adding behaviour to its content, so this attaches an AppKit dragging
/// destination from outside: a transparent view over the panel's content view.
///
/// The interesting part is `hitTest`. A plain overlay view would swallow every
/// click, and the PiP is interactive, and that would silently break takeover in
/// the pop-out. So the view is hit-testable **only while a drag carrying our
/// own type is in flight**, which it learns from the drag pasteboard. The mouse
/// passes straight through at every other moment.
///
/// Because the PiP shares the *same* `LiveStreamSession` as the pane it popped
/// out of (by design: one decode, two observers), a window dropped on the PiP
/// retargets that session, so the PiP and the pane behind it change together.
/// There is no API to give the PiP a session of its own; see `FRICTION.md`.
@MainActor
enum StreamPiPDropTarget {
    /// Install on the currently open PiP panel. Returns false when no panel is
    /// open, which is not an error: it just means there is nothing to install
    /// onto yet.
    @discardableResult
    static func install(session: LiveStreamSession) -> Bool {
        guard let panel = floatingStreamPanel(), let content = panel.contentView else { return false }
        if let existing = content.subviews.compactMap({ $0 as? WindowDropCatcher }).first {
            existing.session = session
            return true
        }
        let catcher = WindowDropCatcher(frame: content.bounds)
        catcher.autoresizingMask = [.width, .height]
        catcher.session = session
        content.addSubview(catcher)
        return true
    }

    /// The PiP is the app's only floating utility panel, which is how it is
    /// identified. Matching on the title would be wrong: the panel is titled
    /// after the stream source and therefore changes on every switch, including
    /// the ones this drop target causes.
    /// The installed drop target, if any.
    static func installedCatcher() -> WindowDropCatcher? {
        floatingStreamPanel()?.contentView?.subviews.compactMap { $0 as? WindowDropCatcher }.first
    }

    static func floatingStreamPanel() -> NSPanel? {
        NSApp.windows.compactMap { $0 as? NSPanel }
            .first { $0.isFloatingPanel && $0.isVisible }
    }
}

/// The transparent dragging destination described above.
final class WindowDropCatcher: NSView {
    weak var session: LiveStreamSession?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([StreamWindowDrag.pasteboardType])
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        registerForDraggedTypes([StreamWindowDrag.pasteboardType])
    }

    /// Invisible to the mouse; visible to our own drags only.
    override func hitTest(_ point: NSPoint) -> NSView? {
        let dragTypes = NSPasteboard(name: .drag).types ?? []
        return dragTypes.contains(StreamWindowDrag.pasteboardType) ? self : nil
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        layer?.borderWidth = 3
        layer?.borderColor = NSColor.systemPurple.cgColor
        wantsLayer = true
        return .copy
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        layer?.borderWidth = 0
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        layer?.borderWidth = 0
        guard let data = sender.draggingPasteboard.data(forType: StreamWindowDrag.pasteboardType)
        else { return false }
        return accept(data)
    }

    /// The whole drop behaviour, separated from `NSDraggingInfo` so it can be
    /// driven without synthesising an AppKit drag session.
    @discardableResult
    func accept(_ data: Data) -> Bool {
        guard let dragged = StreamWindowDrag.window(from: data), let session else { return false }
        Task { @MainActor in
            await session.select(StreamWindowDrag.resolve(dragged, against: session.windows))
        }
        return true
    }
}
#endif
