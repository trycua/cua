// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
@testable import CuaBotsRemote
import Foundation
import Testing

struct EndpointTests {
    @Test func parsesDirectAndRelayLinks() {
        #expect(RemoteEndpoint.parse("cuabots://direct?url=http://10.0.0.5:3211&token=abc")
                == .direct(url: "http://10.0.0.5:3211", token: "abc"))
        #expect(RemoteEndpoint.parse("cuabots://relay?machine=m-123", accountToken: "tok")
                == .relay(relayURL: nil, accountToken: "tok", machineID: "m-123"))
        #expect(RemoteEndpoint.parse("cuabots://relay?machine=m-123") == nil)
        #expect(RemoteEndpoint.parse("cuabots://direct?url=http://x") == nil)
        #expect(RemoteEndpoint.parse("https://example.com") == nil)
    }
}

struct WireTests {
    @Test func snapshotAndCommandRoundTrip() throws {
        let bot = Bot(name: "Ada", owner: "sam")
        let snap = RemoteSnapshot(bot: bot, messages: [ChatMessage(botID: "ada", role: .bot, text: "Hi")],
                                  tasks: [], approvals: [ApprovalRequest(botID: "ada", action: "Buy a desk")],
                                  notifications: [], memory: "# m", outputs: ["agents/ada/outputs/a.md"], busy: false,
                                  written: Date(timeIntervalSince1970: 1_790_000_000))
        #expect(try RemoteSnapshot.decode(snap.encoded()) == snap)
        let cmd = RemoteCommand(botID: "ada", action: .decide(approvalID: "x", approve: true),
                                sentAt: Date(timeIntervalSince1970: 1_790_000_000))
        #expect(try RemoteCommand.decode(cmd.encoded()) == cmd)
    }
}

struct FrameTests {
    @Test func turnsBGRAIntoAnImage() {
        // A 2x1 frame: one blue pixel, one red pixel (BGRA byte order).
        let data = Data([255, 0, 0, 255, 0, 0, 255, 255])
        let img = FrameCollector.imageFromBGRA(width: 2, height: 1, stride: 8, data: data)
        #expect(img?.width == 2)
        #expect(img?.height == 1)
        #expect(FrameCollector.imageFromBGRA(width: 4, height: 4, stride: 16, data: data) == nil)
    }
}

@MainActor
@Suite(.serialized)
struct StoreRemoteTests {
    @Test func appliesPhoneCommandsThroughTheStore() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("cua-bots-remote-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = BotStore(volume: LocalVolume(root: root.appendingPathComponent("d")),
                             dataDirectory: root.appendingPathComponent("s"))
        let bot = await store.createBot(name: "Ada", avatar: .default(for: "Ada"), harness: .hermes, placement: .local)
        await store.apply(RemoteCommand(botID: bot.id, action: .pause))
        #expect(store.bot(bot.id)?.isPaused == true)
        await store.apply(RemoteCommand(botID: bot.id, action: .message("hello from the phone")))
        #expect(store.messages(for: bot.id).last?.role == .system)  // paused: the bot says so
        await store.apply(RemoteCommand(botID: bot.id, action: .resume))
        let snap = try #require(store.snapshot(bot.id))
        #expect(snap.bot.isPaused == false)
        #expect(snap.messages.contains { $0.text == "hello from the phone" })
    }
}
