// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Cua
import CuaSpacesFFI
import SwiftUI

/// The one drop area for a Space: a tall rounded well with a dashed outline, a centred glyph and a
/// one-line caption, and "Send file…" and "Teleport an app…" below. While a
/// file or window is over it the outline turns solid accent and the well
/// fills. It takes files, app bundles (Finder or Dock) and dragged windows.
///
/// Files go to `onFiles` (the app's transfer), an app bundle to `onApp` (the
/// picker at that app), and a window released over the zone commits through
/// the ``WindowDragWatcher``'s `onCommit`, which the zone registers itself
/// with. The same shape as the Cua Spaces app's zone and `<cua-drop-zone>` in
/// `@trycua/cua/teleport`.
public struct TeleportDropZone: View {
    /// The line under the buttons: what the last drop did.
    public enum Status: Equatable, Sendable {
        case working(String)
        case done(String)
        case failed(String)

        public var text: String {
            switch self {
            case let .working(t), let .done(t), let .failed(t): return t
            }
        }
    }

    public static let caption = "Drop a file or window"
    public static let sendFileTitle = "Send file\u{2026}"
    public static let teleportAppTitle = "Teleport an app\u{2026}"
    public static let symbol = "arrow.down.circle"
    public static let activeSymbol = "arrow.down.circle.fill"

    let spaceID: String
    let teleport: Teleport?
    let watcher: WindowDragWatcher?
    let status: Status?
    let onFiles: ([URL]) -> Void
    let onApp: (TeleportCatalogEntry, [String]) -> Void
    let onSendFile: () -> Void
    let onTeleportApp: () -> Void
    /// The words (an app passes its own shared copy; the defaults match it).
    let captionText: String
    let sendFileText: String
    let teleportAppText: String
    /// The teleport icon, and while a drag is over the well.
    let symbolName: String
    let activeSymbolName: String

    public init(spaceID: String, teleport: Teleport?, watcher: WindowDragWatcher? = nil,
                status: Status? = nil,
                caption: String = TeleportDropZone.caption,
                sendFileTitle: String = TeleportDropZone.sendFileTitle,
                teleportAppTitle: String = TeleportDropZone.teleportAppTitle,
                symbol: String = TeleportDropZone.symbol,
                activeSymbol: String = TeleportDropZone.activeSymbol,
                onFiles: @escaping ([URL]) -> Void,
                onApp: @escaping (TeleportCatalogEntry, [String]) -> Void,
                onSendFile: @escaping () -> Void,
                onTeleportApp: @escaping () -> Void) {
        self.spaceID = spaceID
        self.teleport = teleport
        self.watcher = watcher
        self.status = status
        self.onFiles = onFiles
        self.onApp = onApp
        self.onSendFile = onSendFile
        self.onTeleportApp = onTeleportApp
        self.captionText = caption
        self.sendFileText = sendFileTitle
        self.teleportAppText = teleportAppTitle
        self.symbolName = symbol
        self.activeSymbolName = activeSymbol
    }

    public var body: some View {
        if let watcher {
            Watching(zone: self, watcher: watcher)
        } else {
            Well(zone: self, windowOver: false)
        }
    }

    /// Where a drop goes.
    public static func route(_ urls: [URL], teleport: Teleport?) -> TeleportDropOutcome {
        guard let teleport else {
            let files = urls.filter(\.isFileURL)
            return files.isEmpty ? .none : .files(files)
        }
        let items = urls.map { $0.isFileURL ? $0.path : $0.absoluteString }
        let drop = teleport.parseDrop(items: items)
        if let app = drop.apps.first, let entry = try? teleport.catalogEntryForPath(path: app, options: nil) {
            return .app(entry, files: drop.files)
        }
        let files = urls.filter { $0.isFileURL && !drop.apps.contains($0.path) }
        return files.isEmpty ? .none : .files(files)
    }

    func accept(_ urls: [URL]) -> Bool {
        switch Self.route(urls, teleport: teleport) {
        case let .app(entry, files):
            onApp(entry, files)
            return true
        case let .files(files):
            onFiles(files)
            return true
        case .none:
            return false
        }
    }

    private struct Watching: View {
        let zone: TeleportDropZone
        @ObservedObject var watcher: WindowDragWatcher

        var body: some View {
            Well(zone: zone, windowOver: watcher.state.active && watcher.state.overId == zone.spaceID)
                .background(ZoneFrame { view in
                    watcher.registerZone(zone.spaceID) { [weak view] in view.flatMap(Self.screenRect) }
                })
                .onDisappear { watcher.unregisterZone(zone.spaceID) }
        }

        /// The view's frame in global top-left points (the monitor's space).
        static func screenRect(_ view: NSView) -> CGRect? {
            guard let window = view.window else { return nil }
            let r = window.convertToScreen(view.convert(view.bounds, to: nil))
            let top = NSScreen.screens.first?.frame.maxY ?? r.maxY
            return CGRect(x: r.minX, y: top - r.maxY, width: r.width, height: r.height)
        }
    }

    /// The well's minimum height: tall enough to aim at.
    public static let minimumHeight: CGFloat = 180

    struct Well: View {
        let zone: TeleportDropZone
        let windowOver: Bool
        @State private var fileOver = false
        /// Forces the drag-over look (snapshots, previews).
        @Environment(\.teleportDropZoneHighlighted) private var forced

        private var over: Bool { fileOver || windowOver || forced }
        private var busy: Bool { if case .working = zone.status { return true } else { return false } }

        var body: some View {
            VStack(spacing: 10) {
                Spacer(minLength: 0)
                Image(systemName: over ? zone.activeSymbolName : zone.symbolName)
                    .font(.system(size: 34, weight: .light))
                    .foregroundStyle(over ? Color.accentColor : Color.secondary)
                    .accessibilityHidden(true)
                Text(zone.captionText)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(over ? Color.primary : Color.secondary)
                    .lineLimit(1)
                HStack(spacing: 8) {
                    Button(zone.sendFileText, action: zone.onSendFile)
                    Button(zone.teleportAppText, action: zone.onTeleportApp)
                }
                .controlSize(.regular)
                .disabled(busy)
                .padding(.top, 4)
                if let status = zone.status {
                    Text(status.text)
                        .font(.system(size: 11.5))
                        .foregroundStyle(statusColor(status))
                        .multilineTextAlignment(.center)
                        .lineLimit(2)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16).padding(.vertical, 18)
            .frame(maxWidth: .infinity, minHeight: TeleportDropZone.minimumHeight)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(over ? Color.accentColor.opacity(0.12) : Color.secondary.opacity(0.05)))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(over ? Color.accentColor : Color.secondary.opacity(0.5),
                              style: StrokeStyle(lineWidth: over ? 2 : 1.5, dash: over ? [] : [6, 5])))
            .animation(.easeOut(duration: 0.12), value: over)
            .opacity(busy ? 0.72 : 1)
            .contentShape(Rectangle())
            .dropDestination(for: URL.self) { urls, _ in zone.accept(urls) } isTargeted: { fileOver = $0 }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(zone.captionText)
            .accessibilityValue(over ? "Ready to drop" : "")
        }

        private func statusColor(_ s: Status) -> Color {
            if case .failed = s { return .red }
            return .secondary
        }
    }
}

private struct TeleportDropZoneHighlightedKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// Shows a ``TeleportDropZone`` in its drag-over state (snapshots,
    /// previews and design reviews).
    public var teleportDropZoneHighlighted: Bool {
        get { self[TeleportDropZoneHighlightedKey.self] }
        set { self[TeleportDropZoneHighlightedKey.self] = newValue }
    }
}

/// Hands the hosting `NSView` to `onView` once it is in a window.
private struct ZoneFrame: NSViewRepresentable {
    let onView: (NSView) -> Void

    func makeNSView(context: Context) -> NSView {
        let v = NSView()
        DispatchQueue.main.async { onView(v) }
        return v
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}
#endif
