import Combine
import Foundation

/// A **group chat**: one human and two to six Bots in a single thread.
///
/// The bound is a product rule, not an implementation limit, and it is enforced
/// here, in the model, so no caller can construct an out-of-bounds group by
/// going round the UI.
///
/// Every Bot in the group still runs on its own long-lived agent thread in the
/// one shared Space: a group chat is a fan-out over those threads plus a
/// merged, attributed transcript, not a new kind of agent. The same model as
/// `@trycua/cua/spaces/groups` and `cua_spaces::groups`.
public struct GroupChat: Identifiable, Hashable, Sendable {
    /// Two is the floor because a "group" of one is a thread, six the ceiling
    /// because every message fans out to every member.
    public static let minBots = 2
    public static let maxBots = 6

    public var id: String = UUID().uuidString
    public var title: String
    /// Bot ids, in the order they joined. The human is implicit.
    public private(set) var memberIDs: [String]
    public var createdAt: Date = Date()
    public var messages: [GroupMessage] = []

    /// The only way to build one. Throws rather than clamping: silently
    /// dropping the seventh Bot would be a worse failure than refusing.
    public init(id: String = UUID().uuidString, title: String, members: [String],
                createdAt: Date = Date()) throws {
        let deduped = members.reduce(into: [String]()) { acc, id in
            if !acc.contains(id) { acc.append(id) }
        }
        guard deduped.count >= Self.minBots else {
            throw GroupChatError.tooFewBots(have: deduped.count)
        }
        guard deduped.count <= Self.maxBots else {
            throw GroupChatError.tooManyBots(have: deduped.count)
        }
        self.id = id
        self.title = title
        self.memberIDs = deduped
        self.createdAt = createdAt
    }

    public var isFull: Bool { memberIDs.count >= Self.maxBots }
    public var isAtFloor: Bool { memberIDs.count <= Self.minBots }
    /// "4 of 6 bots": the ceiling is visible before a user hits it.
    public var membershipLabel: String { "\(memberIDs.count) of \(Self.maxBots) bots" }
    /// Free seats remaining.
    public var remainingSeats: Int { max(0, Self.maxBots - memberIDs.count) }

    public mutating func add(_ botID: String) throws {
        guard !memberIDs.contains(botID) else {
            throw GroupChatError.alreadyAMember(botID)
        }
        guard !isFull else { throw GroupChatError.full(limit: Self.maxBots) }
        memberIDs.append(botID)
    }

    public mutating func remove(_ botID: String) throws {
        guard memberIDs.contains(botID) else { throw GroupChatError.notAMember(botID) }
        guard !isAtFloor else { throw GroupChatError.atFloor(limit: Self.minBots) }
        memberIDs.removeAll { $0 == botID }
    }
}

/// What can go wrong with membership. Every case carries the number that was
/// violated so the UI can say *why*, not just "no".
public enum GroupChatError: Error, Equatable, CustomStringConvertible, Sendable {
    case tooFewBots(have: Int)
    case tooManyBots(have: Int)
    case full(limit: Int)
    case atFloor(limit: Int)
    case alreadyAMember(String)
    case notAMember(String)
    case unknownChat(String)

    public var description: String {
        switch self {
        case .tooFewBots(let n):
            return "A group chat needs at least \(GroupChat.minBots) bots; \(n) selected."
        case .tooManyBots(let n):
            return "A group chat holds at most \(GroupChat.maxBots) bots; \(n) selected."
        case .full(let limit):
            return "This group is full: \(limit) bots is the limit. Remove one to add another."
        case .atFloor(let limit):
            return "A group chat needs at least \(limit) bots. Add one before removing this one."
        case .alreadyAMember(let id):
            return "\(id) is already in this group."
        case .notAMember(let id):
            return "\(id) is not in this group."
        case .unknownChat(let id):
            return "No such group chat: \(id)"
        }
    }
}

/// Who said a thing in a group. The human is one case, not a Bot with a
/// special id, because the transcript's whole job here is attribution.
public enum GroupSpeaker: Hashable, Sendable {
    case human
    case bot(String)
    /// The group itself: joins, leaves, limit notices, delivery failures.
    case system

    public var botID: String? { if case .bot(let id) = self { return id }; return nil }
}

/// One line of a group transcript.
public struct GroupMessage: Identifiable, Hashable, Sendable {
    public let id = UUID()
    public var speaker: GroupSpeaker
    public var text: String
    public var at: Date = Date()
    /// Set when this line records a message that did **not** reach its Bot.
    public var undelivered: Bool = false
    public var reaction: String? = nil

    public init(speaker: GroupSpeaker, text: String, at: Date = Date(),
                undelivered: Bool = false, reaction: String? = nil) {
        self.speaker = speaker
        self.text = text
        self.at = at
        self.undelivered = undelivered
        self.reaction = reaction
    }
}

/// The result of fanning one message out to one member.
public struct GroupDelivery: Identifiable, Equatable, Sendable {
    public var id: String { botID }
    public var botID: String
    public var accepted: Bool
    public var reason: String

    public init(botID: String, accepted: Bool, reason: String) {
        self.botID = botID
        self.accepted = accepted
        self.reason = reason
    }
}

/// How a group chat reaches the Bots in it.
///
/// A seam: the group logic (membership bounds, fan-out, attribution, refusal
/// handling) is worth testing without a Space, and the live path is worth
/// testing *with* one.
@MainActor
public protocol GroupMessenger: AnyObject {
    /// Deliver the group's message to one member. Never throws: a refusal is a
    /// result, not an error.
    func deliver(_ text: String, to botID: String) async -> GroupDelivery
    /// The Bot's most recent utterance, or `nil` if it has not spoken since it
    /// was last asked.
    func latestReply(from botID: String) async -> String?
    /// Whether this Bot is producing output right now: drives the typing row.
    func isWorking(_ botID: String) -> Bool
    /// Display name for the transcript.
    func displayName(_ botID: String) -> String
}

/// Group chats: membership, fan-out, and the merged transcript.
@MainActor
public final class GroupChatStore: ObservableObject {

    @Published public private(set) var chats: [GroupChat] = []
    /// The last membership complaint, for the view to show inline. Cleared on
    /// the next successful mutation.
    @Published public var lastError: String? = nil
    /// Bots currently producing output, per chat: the typing indicator's input.
    @Published public private(set) var working: [String: Set<String>] = [:]

    /// Held strongly: the messenger owns the app's Bot store, not the other way
    /// round, so there is no cycle; a deallocated messenger would degrade every
    /// send to "not connected to a Space".
    private var messenger: GroupMessenger?
    /// chatID -> botID -> the last reply text already folded into the
    /// transcript, so a re-poll cannot duplicate a Bot's line.
    private var consumed: [String: [String: String]] = [:]

    public init(messenger: GroupMessenger? = nil) { self.messenger = messenger }

    public func attach(messenger: GroupMessenger) { self.messenger = messenger }

    public func chat(_ id: String) -> GroupChat? { chats.first { $0.id == id } }

    // MARK: - Membership

    /// Create a group. The 2..6 bound is enforced at construction, so a group
    /// that exists is always valid.
    @discardableResult
    public func create(title: String, members: [String]) throws -> GroupChat {
        let chat = try GroupChat(title: title, members: members)
        chats.append(chat)
        lastError = nil
        return chat
    }

    /// Whether the UI should let the user tap "Create" yet.
    public static func canCreate(with members: [String]) -> Bool {
        let n = Set(members).count
        return n >= GroupChat.minBots && n <= GroupChat.maxBots
    }

    public func add(_ botID: String, to chatID: String) throws {
        guard let i = chats.firstIndex(where: { $0.id == chatID }) else {
            throw GroupChatError.unknownChat(chatID)
        }
        do {
            try chats[i].add(botID)
            chats[i].messages.append(GroupMessage(
                speaker: .system, text: "\(name(botID)) joined, \(chats[i].membershipLabel)."))
            lastError = nil
        } catch {
            // Surfaced, not swallowed: hitting the ceiling has to say what the
            // ceiling is.
            lastError = "\(error)"
            chats[i].messages.append(GroupMessage(speaker: .system, text: "\(error)",
                                                  undelivered: true))
            throw error
        }
    }

    public func remove(_ botID: String, from chatID: String) throws {
        guard let i = chats.firstIndex(where: { $0.id == chatID }) else {
            throw GroupChatError.unknownChat(chatID)
        }
        do {
            try chats[i].remove(botID)
            chats[i].messages.append(GroupMessage(
                speaker: .system, text: "\(name(botID)) left, \(chats[i].membershipLabel)."))
            lastError = nil
        } catch {
            lastError = "\(error)"
            chats[i].messages.append(GroupMessage(speaker: .system, text: "\(error)",
                                                  undelivered: true))
            throw error
        }
    }

    // MARK: - Talking to the group

    /// Send one message to every Bot in the group.
    ///
    /// Each member is told who else is in the room: there is no shared context
    /// in the harness, so the context has to be carried in the text. Delivery
    /// is per member and partial success is normal; every refusal lands in the
    /// transcript as an attributed, marked line rather than disappearing.
    @discardableResult
    public func send(_ text: String, in chatID: String) async -> [GroupDelivery] {
        guard let i = chats.firstIndex(where: { $0.id == chatID }) else { return [] }
        let chat = chats[i]
        chats[i].messages.append(GroupMessage(speaker: .human, text: text))

        var deliveries: [GroupDelivery] = []
        for botID in chat.memberIDs {
            let framed = Self.frame(text, for: botID, in: chat, names: name)
            let d = await messenger?.deliver(framed, to: botID)
                ?? GroupDelivery(botID: botID, accepted: false,
                                 reason: "not connected to a Space")
            deliveries.append(d)
            if !d.accepted, let j = chats.firstIndex(where: { $0.id == chatID }) {
                chats[j].messages.append(GroupMessage(
                    speaker: .bot(botID),
                    text: "Did not receive that message: \(d.reason)",
                    undelivered: true))
            }
        }
        refreshWorking(chatID)
        return deliveries
    }

    /// The group framing: the room, then the human's words on the last line.
    public static func frame(_ text: String, for botID: String, in chat: GroupChat,
                             names: (String) -> String) -> String {
        let others = chat.memberIDs.filter { $0 != botID }.map(names).joined(separator: ", ")
        return """
        [group:\(chat.title)] You are in a group chat with the user and \(others). \
        Answer for your own area only and keep it to a few lines.
        \(text)
        """
    }

    /// Fold whatever the members have said since the last poll into the
    /// transcript, attributed. Returns the new lines.
    @discardableResult
    public func collectReplies(in chatID: String) async -> [GroupMessage] {
        guard let chat = chat(chatID) else { return [] }
        var added: [GroupMessage] = []
        for botID in chat.memberIDs {
            guard let reply = await messenger?.latestReply(from: botID) else { continue }
            let trimmed = reply.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { continue }
            if consumed[chatID]?[botID] == trimmed { continue }
            consumed[chatID, default: [:]][botID] = trimmed
            guard let i = chats.firstIndex(where: { $0.id == chatID }) else { return added }
            let line = GroupMessage(speaker: .bot(botID), text: trimmed)
            chats[i].messages.append(line)
            added.append(line)
        }
        refreshWorking(chatID)
        return added
    }

    /// Attach a reaction to the last line in the group.
    public func react(_ emoji: String, in chatID: String) {
        guard let i = chats.firstIndex(where: { $0.id == chatID }),
              !chats[i].messages.isEmpty else { return }
        chats[i].messages[chats[i].messages.count - 1].reaction = emoji
    }

    public func refreshWorking(_ chatID: String) {
        guard let chat = chat(chatID), let m = messenger else { return }
        working[chatID] = Set(chat.memberIDs.filter { m.isWorking($0) })
    }

    public func workingBots(in chatID: String) -> [String] {
        (working[chatID] ?? []).sorted()
    }

    private func name(_ botID: String) -> String {
        messenger?.displayName(botID) ?? botID
    }

    public func displayName(_ botID: String) -> String { name(botID) }
}
