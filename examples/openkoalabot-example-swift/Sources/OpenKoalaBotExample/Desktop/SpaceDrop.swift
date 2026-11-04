// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Foundation
import SwiftUI

/// The one drop area for the Bot's Space: a tall rounded well with a dashed
/// outline, a centred glyph, a one-line caption and "Send file…" below. While
/// a file is over it the outline turns solid accent and the well fills. Files
/// dropped or picked go to `onFiles` (the app's transfer into the Space).
struct SpaceDropZone: View {
    /// The line under the button: what the last drop did.
    enum Status: Equatable, Sendable {
        case working(String)
        case done(String)
        case failed(String)

        var text: String {
            switch self {
            case let .working(t), let .done(t), let .failed(t): return t
            }
        }
    }

    static let caption = "Drop a file"
    static let sendFileTitle = "Send file\u{2026}"
    static let symbol = "arrow.down.circle"
    static let activeSymbol = "arrow.down.circle.fill"
    /// Tall enough to aim at.
    static let minimumHeight: CGFloat = 180

    let spaceID: String
    let status: Status?
    let onFiles: ([URL]) -> Void
    let onSendFile: () -> Void

    init(spaceID: String, status: Status? = nil,
         onFiles: @escaping ([URL]) -> Void, onSendFile: @escaping () -> Void) {
        self.spaceID = spaceID
        self.status = status
        self.onFiles = onFiles
        self.onSendFile = onSendFile
    }

    /// The files in a drop: local, non-directory URLs. Anything else (web
    /// links, folders, app bundles) is not taken.
    static func files(in urls: [URL]) -> [URL] {
        urls.filter { url in
            var dir: ObjCBool = false
            return url.isFileURL
                && FileManager.default.fileExists(atPath: url.path, isDirectory: &dir) && !dir.boolValue
        }
    }

    func accept(_ urls: [URL]) -> Bool {
        let files = Self.files(in: urls)
        guard !files.isEmpty else { return false }
        onFiles(files)
        return true
    }

    @State private var over = false
    private var busy: Bool { if case .working = status { return true } else { return false } }

    var body: some View {
        VStack(spacing: 10) {
            Spacer(minLength: 0)
            Image(systemName: over ? Self.activeSymbol : Self.symbol)
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(over ? Color.accentColor : Color.secondary)
                .accessibilityHidden(true)
            Text(Self.caption)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(over ? Color.primary : Color.secondary)
                .lineLimit(1)
            Button(Self.sendFileTitle, action: onSendFile)
                .controlSize(.regular)
                .disabled(busy)
                .padding(.top, 4)
            if let status {
                Text(status.text)
                    .font(.system(size: 11.5))
                    .foregroundStyle(status.isFailure ? Color.red : Color.secondary)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16).padding(.vertical, 18)
        .frame(maxWidth: .infinity, minHeight: Self.minimumHeight)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .fill(over ? Color.accentColor.opacity(0.12) : Color.secondary.opacity(0.05)))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(over ? Color.accentColor : Color.secondary.opacity(0.5),
                          style: StrokeStyle(lineWidth: over ? 2 : 1.5, dash: over ? [] : [6, 5])))
        .animation(.easeOut(duration: 0.12), value: over)
        .opacity(busy ? 0.72 : 1)
        .contentShape(Rectangle())
        .dropDestination(for: URL.self) { urls, _ in accept(urls) } isTargeted: { over = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Self.caption)
        .accessibilityValue(over ? "Ready to drop" : "")
    }
}

private extension SpaceDropZone.Status {
    var isFailure: Bool { if case .failed = self { return true } else { return false } }
}

/// The drop zone's behaviour for the Bot's Space: files dropped or picked
/// with "Send file…" are uploaded for the Bot, and the zone says where they
/// landed. The Computer pane and the takeover share one coordinator, so there
/// is a single drop behaviour.
@MainActor
final class SpaceDropCoordinator: ObservableObject {
    /// What the drop zone's last drop did.
    @Published var zoneStatus: SpaceDropZone.Status?
    private let store: BotStore

    init(store: BotStore) {
        self.store = store
    }

    /// A drop on the zone: files upload for `botID`.
    @discardableResult
    func drop(_ urls: [URL], botID: String?) -> Bool {
        let files = SpaceDropZone.files(in: urls)
        guard !files.isEmpty, let botID else { return false }
        send(files, to: botID)
        return true
    }

    /// Files dropped on the zone or picked with "Send file…": uploaded into
    /// the Space for `botID`, and the zone says where they landed.
    func send(_ urls: [URL], to botID: String) {
        guard !urls.isEmpty else { return }
        let label = urls.count == 1 ? urls[0].lastPathComponent : "\(urls.count) files"
        zoneStatus = .working("Sending \(label)\u{2026}")
        Task {
            var sent = 0
            for url in urls {
                if await store.attach(url.path, to: botID) != nil { sent += 1 }
            }
            zoneStatus = sent == urls.count
                ? .done("Sent \(label) to /tmp/openkoalabots")
                : .failed("Could not send \(label)")
        }
    }

    /// "Send file…": pick files and send them like a drop.
    func pickFiles(for botID: String) {
        guard let bot = store.bot(botID) else { return }
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.prompt = "Attach"
        panel.message = "Files are uploaded into the Space for \(bot.name)."
        guard panel.runModal() == .OK else { return }
        send(panel.urls, to: botID)
    }

    /// The one drop target for `botID` in `space`. The Computer pane and the
    /// takeover both show this.
    func zone(space: String, botID: String) -> SpaceDropZone {
        SpaceDropZone(spaceID: space, status: zoneStatus,
                      onFiles: { [weak self] in self?.send($0, to: botID) },
                      onSendFile: { [weak self] in self?.pickFiles(for: botID) })
    }
}
#endif
