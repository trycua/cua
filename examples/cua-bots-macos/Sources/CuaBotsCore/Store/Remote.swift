// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// What a phone sees of one bot: written by the Mac into the bot's own Space
/// (`<home>/.cua-bots/state.json`), read by the iOS app through the Space's
/// spacesd, over the relay or directly.
public struct RemoteSnapshot: Codable, Equatable, Sendable {
    public static let path = ".cua-bots/state.json"
    public static let outbox = ".cua-bots/outbox"

    public var bot: Bot
    public var messages: [ChatMessage]
    public var tasks: [BotTask]
    public var approvals: [ApprovalRequest]
    public var notifications: [BotNotification]
    public var memory: String
    public var outputs: [String]
    public var busy: Bool
    public var written: Date

    public init(bot: Bot, messages: [ChatMessage], tasks: [BotTask], approvals: [ApprovalRequest],
                notifications: [BotNotification], memory: String, outputs: [String], busy: Bool,
                written: Date = Date()) {
        self.bot = bot
        self.messages = messages
        self.tasks = tasks
        self.approvals = approvals
        self.notifications = notifications
        self.memory = memory
        self.outputs = outputs
        self.busy = busy
        self.written = written
    }

    public static func decode(_ data: Data) throws -> RemoteSnapshot {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .deferredToDate
        return try d.decode(RemoteSnapshot.self, from: data)
    }

    public func encoded() throws -> Data {
        let e = JSONEncoder()
        e.dateEncodingStrategy = .deferredToDate
        return try e.encode(self)
    }
}

/// Something the phone asks the Mac to do, dropped into the bot's outbox
/// (`<home>/.cua-bots/outbox/<id>.json`). The Mac applies it through the same
/// store calls its own buttons make, so there is one writer.
public struct RemoteCommand: Codable, Equatable, Sendable, Identifiable {
    public enum Action: Codable, Equatable, Sendable {
        case message(String)
        case decide(approvalID: String, approve: Bool)
        case pause
        case resume
        case markRead
    }

    public var id: String
    public var botID: String
    public var action: Action
    public var sentAt: Date
    public var from: String

    public init(id: String = UUID().uuidString, botID: String, action: Action, sentAt: Date = Date(),
                from: String = "iPhone") {
        self.id = id
        self.botID = botID
        self.action = action
        self.sentAt = sentAt
        self.from = from
    }

    public static func decode(_ data: Data) throws -> RemoteCommand {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .deferredToDate
        return try d.decode(RemoteCommand.self, from: data)
    }

    public func encoded() throws -> Data {
        let e = JSONEncoder()
        e.dateEncodingStrategy = .deferredToDate
        return try e.encode(self)
    }
}

public extension BotStore {
    /// The phone's view of a bot, the last `limit` messages.
    func snapshot(_ id: String, limit: Int = 80) -> RemoteSnapshot? {
        guard let bot = bot(id) else { return nil }
        return RemoteSnapshot(
            bot: bot, messages: Array(messages(for: id).suffix(limit)), tasks: tasks(for: id),
            approvals: approvals.filter { $0.botID == id }.suffix(30),
            notifications: Array(notifications.filter { $0.botID == id }.prefix(30)),
            memory: memory(for: id), outputs: outputs(for: id).map(\.path), busy: isBusy(id))
    }

    /// Apply a command from the phone.
    func apply(_ command: RemoteCommand) async {
        guard bot(command.botID) != nil else { return }
        switch command.action {
        case .message(let text): await send(command.botID, text)
        case .decide(let a, let approve): await decide(a, approve: approve)
        case .pause: await pause(command.botID)
        case .resume: await resume(command.botID)
        case .markRead: markAllRead(bot: command.botID)
        }
    }
}
