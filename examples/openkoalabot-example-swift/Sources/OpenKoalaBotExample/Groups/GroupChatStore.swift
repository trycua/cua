// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation

// `GroupChat`, `GroupChatStore` and the `GroupMessenger` seam are the SDK's
// (`CuaSpaces`). This app supplies the live messenger over its Bot store.

// MARK: - The live messenger

/// Fans a group message out over the real per-Bot agent threads in the one
/// shared Space.
@MainActor
final class BotStoreGroupMessenger: GroupMessenger {
    private let store: BotStore
    init(store: BotStore) { self.store = store }

    func deliver(_ text: String, to botID: String) async -> GroupDelivery {
        let presence = store.presence(for: botID)
        if !presence.hasThread {
            do {
                _ = try await store.hire(botID, prompt: text)
                return GroupDelivery(botID: botID, accepted: true, reason: "started for this group")
            } catch {
                return GroupDelivery(botID: botID, accepted: false, reason: "\(error)")
            }
        }
        let outcome = await store.send(text, to: botID)
        return GroupDelivery(botID: botID, accepted: outcome.accepted, reason: outcome.reason)
    }

    func latestReply(from botID: String) async -> String? {
        await store.refresh(botID)
        let messages = store.thread(for: botID).messages
        for m in messages.reversed() where m.sender == .bot {
            if case .prose(let text) = m.body { return text }
        }
        return nil
    }

    func isWorking(_ botID: String) -> Bool {
        store.presence(for: botID).state == .running
    }

    func displayName(_ botID: String) -> String {
        store.bot(botID)?.name ?? botID
    }
}
