import Foundation

// MARK: - Sharing a Space through the relay

/// Who a Space is shared with, as the server reports it (the whole answer,
/// kept whole, plus the accounts by role).
public struct SpaceSharing: Sendable, Hashable {
    public let space: SpaceID
    /// `email or account id` -> `viewer` or `editor`.
    public let people: [String: String]
    public let raw: JSONValue

    init(_ d: [String: JSONValue]) {
        space = SpaceID(d["space"]?.stringValue ?? "")
        var people: [String: String] = [:]
        for entry in d["shares"]?.arrayValue ?? [] {
            guard let o = entry.objectValue, let who = o["who"]?.stringValue else { continue }
            people[who] = o["role"]?.stringValue ?? "viewer"
        }
        self.people = people
        raw = .object(d)
    }
}

extension Space {
    /// `share_space`: let an account (verified email or account id) watch
    /// (`viewer`) or also use (`editor`) this Space, through the relay.
    @discardableResult
    public func share(with who: String, role: String = "viewer") async throws -> SpaceSharing {
        SpaceSharing(try await connection.object(
            "share_space",
            ["space": .string(id.rawValue), "who": .string(who), "role": .string(role)]))
    }

    /// `unshare_space`: take one account's access back (everyone's when nil).
    @discardableResult
    public func unshare(_ who: String? = nil) async throws -> SpaceSharing {
        var args: [String: JSONValue] = ["space": .string(id.rawValue)]
        if let who { args["who"] = .string(who) }
        return SpaceSharing(try await connection.object("unshare_space", args))
    }

    /// `space_shares`.
    public func shares() async throws -> SpaceSharing {
        SpaceSharing(try await connection.object("space_shares", ["space": .string(id.rawValue)]))
    }

    /// `relay_register_space`: make this Space reachable through the relay
    /// (from a phone off this Mac's network). Returns its `relay:` id.
    @discardableResult
    public func registerWithRelay() async throws -> SpaceID {
        let d = try await connection.object("relay_register_space", ["space": .string(id.rawValue)])
        return SpaceID(d["relay_space"]?.stringValue ?? "")
    }

    /// `relay_unregister_space`.
    public func unregisterFromRelay() async throws {
        _ = try await connection.object("relay_unregister_space", ["space": .string(id.rawValue)])
    }
}
