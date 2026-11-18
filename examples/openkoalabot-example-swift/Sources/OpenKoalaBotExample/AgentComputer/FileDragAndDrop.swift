// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import CuaSpaces
import SwiftUI

// MARK: - The attachment caps

/// The attachment caps: **6 attachments**, **25 MB** per file, **200 MB** per
/// message.
///
/// The numbers, and the arithmetic that applies them, are now
/// `CuaSpaces.TransferLimits` — `FRICTION.md` §14 says the caps are a product
/// contract with nothing behind them in the API, so the SDK carries them as
/// data and checks them before any I/O. What stays here is the *wording*: "Up
/// to 6 attachments per message" is copy, and copy belongs to the app. The app
/// also keeps its own binary-MB formatter, so the strings the screenshots pin
/// do not move.
enum AttachmentLimits {
    static let maxCount = 6
    static let maxBytesPerFile = 25 * 1024 * 1024
    static let maxBytesTotal = 200 * 1024 * 1024

    /// This app's caps in the SDK's shape. `TransferLimits.koalaBotsAttachments`
    /// states the same contract in decimal MB; this spelling keeps the exact
    /// byte counts the app has always enforced.
    static var transferLimits: TransferLimits {
        TransferLimits(maxFileCount: maxCount,
                       maxBytesPerFile: maxBytesPerFile,
                       maxBytesPerBatch: maxBytesTotal)
    }

    /// Megabytes, one decimal, no locale surprises in test assertions.
    static func format(_ bytes: Int) -> String {
        let mb = Double(bytes) / 1_048_576
        if mb < 0.1 && bytes > 0 { return "<0.1 MB" }
        return String(format: "%.1f MB", mb)
    }
}

/// The banner's phrasing over the SDK's verdict.
///
/// **The rule moved.** `TransferLimits.admit` owns partial admission and the
/// order the caps are tested in — per-file size before the running total, so
/// one oversized file is named as oversized rather than as "the batch is too
/// big", which would point the user at the wrong file to remove. Any app
/// dropping files into a Space would otherwise reimplement exactly that, and
/// get the order wrong in exactly that way. What is left here is the sentence
/// the user reads, which is product copy and not transfer policy.
enum AttachmentAdmission {
    typealias Candidate = TransferLimits.Candidate

    struct Rejection: Equatable {
        var candidate: Candidate
        var reason: String
    }

    struct Result: Equatable {
        var accepted: [Candidate] = []
        var rejected: [Rejection] = []
        /// One line for the banner. `nil` when everything was accepted.
        var message: String? {
            guard let first = rejected.first else { return nil }
            if rejected.count == 1 { return first.reason }
            return "\(first.reason) \(rejected.count - 1) more were not attached."
        }
    }

    /// Admit what fits, and say precisely why the rest does not.
    static func admit(_ candidates: [Candidate], existing: [Candidate] = []) -> Result {
        let admission = AttachmentLimits.transferLimits.admit(candidates, existing: existing)
        return Result(
            accepted: admission.accepted,
            rejected: admission.rejected.map {
                Rejection(candidate: $0.candidate,
                          reason: phrasing(for: $0.violation, $0.candidate))
            })
    }

    /// `TransferLimits.Violation.description` is diagnostic prose for a log.
    /// This is what the user reads, and it is deliberately not the same string.
    private static func phrasing(for violation: TransferLimits.Violation,
                                 _ candidate: Candidate) -> String {
        switch violation {
        case .tooManyFiles:
            return "Up to \(AttachmentLimits.maxCount) attachments per message."
        case let .fileTooLarge(name, bytes, limit):
            return "“\(name)” is \(AttachmentLimits.format(bytes)). "
                 + "Files are capped at \(AttachmentLimits.format(limit))."
        case let .batchTooLarge(_, limit):
            return "“\(candidate.name)” would take this message over "
                 + "\(AttachmentLimits.format(limit))."
        }
    }
}

// MARK: - Intake

/// One attachment on its way into the Space.
struct PendingAttachment: Identifiable, Equatable {
    enum State: Equatable {
        case queued
        case uploading
        /// Landed in the Space at this path.
        case uploaded(String)
        case failed(String)
    }

    let id = UUID()
    var url: URL
    var name: String
    var byteCount: Int
    var state: State = .queued

    var candidate: AttachmentAdmission.Candidate {
        .init(name: name, byteCount: byteCount)
    }
}

/// The drop target's model: caps, queue, and the actual `upload` call.
///
/// The uploader is a closure rather than a `SpacesClient`, for the same reason
/// `Streaming/` takes a closure-shaped provider: this view layer has to compile
/// and be testable with no Spaces client in the picture, and the export path
/// runs with no uploader at all.
@MainActor
final class AttachmentIntake: ObservableObject {
    @Published private(set) var items: [PendingAttachment] = []
    /// The failure state, verbatim as shown. Cleared by the next successful drop.
    @Published private(set) var rejection: String?

    /// `(local file, path inside the Space) -> ()`. `nil` means "no Space":
    /// files are accepted and queued but never leave the host.
    var upload: (@Sendable (URL, String) async throws -> Void)?
    /// Where attachments land in the Space.
    var remoteDirectory = "/tmp/openkoalabots-attachments"

    init(items: [PendingAttachment] = []) {
        self.items = items
    }

    var totalBytes: Int { items.reduce(0) { $0 + $1.byteCount } }

    /// Take a drop. Returns the accepted attachments, and leaves the rejection
    /// message on `rejection` for the banner.
    @discardableResult
    func accept(_ urls: [URL]) -> [PendingAttachment] {
        let candidates = urls.map { url -> (URL, AttachmentAdmission.Candidate) in
            let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
            return (url, .init(name: url.lastPathComponent, byteCount: size))
        }
        let result = AttachmentAdmission.admit(candidates.map(\.1),
                                               existing: items.map(\.candidate))
        rejection = result.message

        var admitted: [PendingAttachment] = []
        for (url, candidate) in candidates where result.accepted.contains(candidate) {
            let pending = PendingAttachment(url: url, name: candidate.name,
                                            byteCount: candidate.byteCount)
            items.append(pending)
            admitted.append(pending)
        }
        for pending in admitted { Task { await send(pending.id) } }
        return admitted
    }

    func remove(_ id: UUID) {
        items.removeAll { $0.id == id }
    }

    func clear() {
        items.removeAll()
        rejection = nil
    }

    /// Push one queued attachment into the Space.
    func send(_ id: UUID) async {
        guard let index = items.firstIndex(where: { $0.id == id }) else { return }
        guard let upload else {
            items[index].state = .failed("no Space is attached")
            return
        }
        let item = items[index]
        // A collision-proof remote name. `upload` overwrites its destination
        // silently, and two drops of "notes.txt" from two folders are two
        // different files.
        let remote = "\(remoteDirectory)/\(item.id.uuidString.prefix(8))-\(item.name)"
        items[index].state = .uploading
        do {
            try await upload(item.url, remote)
            if let i = items.firstIndex(where: { $0.id == id }) {
                items[i].state = .uploaded(remote)
            }
        } catch {
            if let i = items.firstIndex(where: { $0.id == id }) {
                items[i].state = .failed("\(error)")
            }
        }
    }
}

// MARK: - Dragging a produced file back out

/// A file the Bot made, dragged out of the transcript into Finder.
///
/// The awkward shape: `NSItemProvider` must be handed a URL **synchronously**
/// when the drag begins, but `download` is an async MCP round trip. There is no
/// "promise me this file" affordance that survives the gap, so the file is
/// fetched to a host temp directory when its row appears and the drag carries
/// the already-local copy. A row dragged before its fetch finishes is refused
/// rather than handing Finder a path with nothing behind it.
@MainActor
final class AgentArtifactExport: ObservableObject {
    /// `remote path -> local path`.
    @Published private(set) var local: [String: String] = [:]
    @Published private(set) var failures: [String: String] = [:]

    /// `(path in Space) -> host path`.
    var download: (@Sendable (String) async throws -> String)?

    func prepare(_ remotePath: String) async {
        guard local[remotePath] == nil, let download else { return }
        do {
            local[remotePath] = try await download(remotePath)
        } catch {
            failures[remotePath] = "\(error)"
        }
    }

    func isReady(_ remotePath: String) -> Bool {
        guard let path = local[remotePath] else { return false }
        return FileManager.default.fileExists(atPath: path)
    }

    /// The provider for `.onDrag`. Empty when the fetch has not landed.
    func itemProvider(_ remotePath: String) -> NSItemProvider {
        guard let path = local[remotePath],
              FileManager.default.fileExists(atPath: path),
              let provider = NSItemProvider(contentsOf: URL(fileURLWithPath: path)) else {
            return NSItemProvider()
        }
        return provider
    }
}

// MARK: - Attachment tray + failure state

/// The queued attachments, with per-file upload state, and the rejection
/// banner.
struct AttachmentTray: View {
    @ObservedObject var intake: AttachmentIntake
    var scale: CGFloat = 1
    var onSurface: Color = DS.surface
    var textColor: Color = DS.onSurface
    var secondary: Color = DS.secondary

    var body: some View {
        VStack(alignment: .leading, spacing: 10 * scale) {
            if let rejection = intake.rejection {
                HStack(spacing: 8 * scale) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 12 * scale))
                    Text(rejection).font(DS.font(12 * scale, .medium))
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .foregroundStyle(Color(hex: 0xD93B3B))
                .padding(.horizontal, 12 * scale).padding(.vertical, 8 * scale)
                .background(RoundedRectangle(cornerRadius: 10 * scale, style: .continuous)
                    .fill(Color(hex: 0xD93B3B).opacity(0.12)))
            }
            if !intake.items.isEmpty {
                HStack(spacing: 8 * scale) {
                    ForEach(intake.items) { item in
                        chip(item)
                    }
                    Spacer(minLength: 0)
                }
            }
        }
    }

    private func chip(_ item: PendingAttachment) -> some View {
        HStack(spacing: 7 * scale) {
            glyph(item.state)
            VStack(alignment: .leading, spacing: 0) {
                Text(item.name).font(DS.font(12 * scale, .medium)).foregroundStyle(textColor)
                    .lineLimit(1)
                Text(detail(item)).font(DS.font(10 * scale)).foregroundStyle(secondary)
                    .lineLimit(1)
            }
            Button { intake.remove(item.id) } label: {
                Image(systemName: "xmark").font(.system(size: 9 * scale, weight: .semibold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(secondary)
        }
        .padding(.horizontal, 12 * scale).padding(.vertical, 7 * scale)
        .background(Capsule().fill(onSurface))
    }

    @ViewBuilder private func glyph(_ state: PendingAttachment.State) -> some View {
        switch state {
        case .queued:
            Image(systemName: "doc.fill").font(.system(size: 13 * scale)).foregroundStyle(secondary)
        case .uploading:
            ProgressView().controlSize(.small).scaleEffect(0.7)
        case .uploaded:
            Image(systemName: "checkmark.circle.fill").font(.system(size: 13 * scale))
                .foregroundStyle(Color(hex: 0x18BE4B))
        case .failed:
            Image(systemName: "exclamationmark.circle.fill").font(.system(size: 13 * scale))
                .foregroundStyle(Color(hex: 0xD93B3B))
        }
    }

    private func detail(_ item: PendingAttachment) -> String {
        switch item.state {
        case .queued: return AttachmentLimits.format(item.byteCount)
        case .uploading: return "Sending…"
        case let .uploaded(path): return "In the Space · \((path as NSString).lastPathComponent)"
        case let .failed(reason): return "Failed: \(reason)"
        }
    }
}
#endif
