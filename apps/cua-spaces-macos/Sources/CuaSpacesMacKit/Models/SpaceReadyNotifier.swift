// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
import Foundation
import UserNotifications

/// A Space the onboarding created has come up: what the notification says.
struct SpaceReadyNote: Equatable {
    let spaceId: String
    let name: String
    let os: AppSpaceOs

    /// "<Space name> is ready".
    var title: String { "\(name) is ready" }

    /// "A fresh macOS computer inside this Mac. Your agent can use it now."
    /// (no OS word when the Space's is not known).
    var body: String {
        let word: String
        switch os {
        case .macos: word = "macOS "
        case .linux: word = "Linux "
        case .windows: word = "Windows "
        case .unknown: word = ""
        }
        return "A fresh \(word)computer inside this Mac. Your agent can use it now."
    }

    /// The notification's identifier; clicking it carries the Space's id back.
    var identifier: String { Self.identifierPrefix + spaceId }

    static let identifierPrefix = "space-ready-"

    /// The Space a clicked notification (its identifier) is about.
    static func spaceId(fromIdentifier id: String) -> String? {
        guard id.hasPrefix(identifierPrefix), id.count > identifierPrefix.count else { return nil }
        return String(id.dropFirst(identifierPrefix.count))
    }
}

/// The system notification for a Space created from the first run
/// ("<Space> is ready"): posted whether or not a window is open, carrying
/// the Cua key as its mark. Only a bundled app posts it (a test process has
/// no notification identity); its click is `DeviceNotifier`'s `onOpen`.
@MainActor
final class SpaceReadyNotifier {
    static let shared = SpaceReadyNotifier()

    func post(_ note: SpaceReadyNote) {
        guard DeviceNotifier.available else { return }
        let center = UNUserNotificationCenter.current()
        center.delegate = DeviceNotifier.shared
        let content = UNMutableNotificationContent()
        content.title = note.title
        content.body = note.body
        content.sound = .default
        // Adding an attachment moves its file: give it a copy.
        if let copy = Self.attachmentCopy(of: CuaKey.url),
           let attachment = try? UNNotificationAttachment(identifier: "cua-key", url: copy,
                                                          options: [UNNotificationAttachmentOptionsTypeHintKey: "public.png"]) {
            content.attachments = [attachment]
        }
        let request = UNNotificationRequest(identifier: note.identifier, content: content, trigger: nil)
        Task {
            guard await PermissionRequester.notificationsAuthorized(center) else { return }
            try? await center.add(request)
        }
    }

    /// The image as a file of its own in the temporary directory (nil when
    /// there is none or it can't be copied).
    static func attachmentCopy(of source: URL?, in directory: URL = FileManager.default.temporaryDirectory) -> URL? {
        guard let source else { return nil }
        let copy = directory.appendingPathComponent("cua-notification-\(UUID().uuidString).png")
        do {
            try FileManager.default.copyItem(at: source, to: copy)
            return copy
        } catch {
            return nil
        }
    }
}
