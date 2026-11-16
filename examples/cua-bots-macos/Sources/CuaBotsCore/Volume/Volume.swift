// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The Cua Volume layout this sample keeps a bot's persistent state in:
///
/// ```text
/// public/                 shared reference every bot reads
/// agents/<name>/          one bot's home
///   identity/bot.json     name, avatar, harness, where it runs
///   <instructions file>   CLAUDE.md, SOUL.md or AGENTS.md: who it is, its rules
///   memory/MEMORY.md      what it remembers
///   rules.yaml            its custom rules
///   tasks.json            scheduled and ongoing work
///   outputs/              "Your research is ready": finished files
///   inbox/                files handed to it
/// spaces/<space>/         per-Space scratch
/// ```
public enum VolumeLayout {
    public static let publicFolder = "public/"
    public static func home(_ agent: String) -> String { "agents/\(agent)/" }
    public static func identity(_ agent: String) -> String { "agents/\(agent)/identity/bot.json" }
    public static func memory(_ agent: String) -> String { "agents/\(agent)/memory/MEMORY.md" }
    public static func rules(_ agent: String) -> String { "agents/\(agent)/rules.yaml" }
    public static func tasks(_ agent: String) -> String { "agents/\(agent)/tasks.json" }
    public static func outputs(_ agent: String) -> String { "agents/\(agent)/outputs/" }
    public static func inbox(_ agent: String) -> String { "agents/\(agent)/inbox/" }
    public static func instructions(_ agent: String, harness: Harness) -> String {
        "agents/\(agent)/\(harness.instructionsFile)"
    }

    /// `local:bot-ada` is `spaces/local-bot-ada/`, the way `cua-volume` names
    /// Space folders.
    public static func space(_ spaceID: String) -> String {
        let mapped = spaceID.trimmingCharacters(in: .whitespaces).map { c -> Character in
            c.isASCII && (c.isLetter || c.isNumber || c == "." || c == "_" || c == "-") ? c : "-"
        }
        let name = String(mapped).trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        return "spaces/\(name.isEmpty ? "space" : name)/"
    }
}

/// One Volume entry.
public struct VolumeEntry: Identifiable, Hashable, Sendable {
    public var id: String { path }
    /// The key; folders end in `/`.
    public var path: String
    public var name: String
    public var isFolder: Bool
    public var size: UInt64
    public var modified: Date?
}

public enum VolumeError: Error, Equatable, LocalizedError {
    case invalid(String)
    case secretDetected(path: String, line: Int)

    public var errorDescription: String? {
        switch self {
        case .invalid(let s): "Invalid Volume path: \(s)"
        case .secretDetected(let path, let line):
            "\(path) line \(line) looks like a secret. Secrets belong in the Keyvault, not the Volume."
        }
    }
}

/// The Volume operations the app uses, named as `cua-volume`'s session names
/// them (`ls`, `read`, `write`, `delete`).
public protocol VolumeStore: Sendable {
    func ls(_ path: String) throws -> [VolumeEntry]
    func read(_ path: String) throws -> Data?
    func write(_ path: String, _ data: Data) throws
    func delete(_ path: String) throws
    /// Every file under a folder, recursively.
    func walk(_ path: String) throws -> [VolumeEntry]
}

public extension VolumeStore {
    func readText(_ path: String) throws -> String? {
        try read(path).map { String(decoding: $0, as: UTF8.self) }
    }

    func writeText(_ path: String, _ text: String) throws {
        try write(path, Data(text.utf8))
    }

    func readJSON<T: Decodable>(_ type: T.Type, _ path: String) throws -> T? {
        guard let data = try read(path) else { return nil }
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return try d.decode(T.self, from: data)
    }

    func writeJSON<T: Encodable>(_ value: T, _ path: String) throws {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        e.dateEncodingStrategy = .iso8601
        try write(path, try e.encode(value))
    }
}

/// The Volume as a folder on this machine. It stands in for `cua volume`
/// until the Volume is in the Swift SDK: same layout, same key rules, and the
/// same refusal to store secrets under `agents/`.
public struct LocalVolume: VolumeStore {
    public let root: URL

    public init(root: URL) {
        self.root = root
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    func url(_ path: String) throws -> URL {
        let key = path.hasPrefix("/") ? String(path.dropFirst()) : path
        guard !key.split(separator: "/").contains(where: { $0 == ".." || $0 == "." }) else {
            throw VolumeError.invalid(path)
        }
        return key.isEmpty ? root : root.appendingPathComponent(key)
    }

    public func ls(_ path: String) throws -> [VolumeEntry] {
        let dir = try url(path)
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: dir.path) else { return [] }
        let prefix = path.isEmpty || path.hasSuffix("/") ? path : path + "/"
        return names.filter { !$0.hasPrefix(".") }.sorted().map { name in
            let u = dir.appendingPathComponent(name)
            let attrs = try? fm.attributesOfItem(atPath: u.path)
            let isDir = (attrs?[.type] as? FileAttributeType) == .typeDirectory
            return VolumeEntry(path: prefix + name + (isDir ? "/" : ""), name: name, isFolder: isDir,
                              size: (attrs?[.size] as? NSNumber)?.uint64Value ?? 0,
                              modified: attrs?[.modificationDate] as? Date)
        }
    }

    public func walk(_ path: String) throws -> [VolumeEntry] {
        try ls(path).flatMap { $0.isFolder ? try walk($0.path) : [$0] }
    }

    public func read(_ path: String) throws -> Data? {
        try? Data(contentsOf: try url(path))
    }

    public func write(_ path: String, _ data: Data) throws {
        if path.hasPrefix("agents/"), let line = SecretScanner.firstHit(in: data) {
            throw VolumeError.secretDetected(path: path, line: line)
        }
        let u = try url(path)
        try FileManager.default.createDirectory(at: u.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: u, options: .atomic)
    }

    public func delete(_ path: String) throws {
        let u = try url(path)
        if FileManager.default.fileExists(atPath: u.path) { try FileManager.default.removeItem(at: u) }
    }
}

/// The patterns `cua-volume`'s scanner refuses under `agents/`: provider keys,
/// cloud keys, private keys and bearer tokens.
public enum SecretScanner {
    static let patterns: [NSRegularExpression] = [
        #"sk-[A-Za-z0-9_\-]{20,}"#,
        #"sk-ant-[A-Za-z0-9_\-]{20,}"#,
        #"AKIA[0-9A-Z]{16}"#,
        #"ghp_[A-Za-z0-9]{30,}"#,
        #"xox[abpr]-[A-Za-z0-9-]{10,}"#,
        #"-----BEGIN [A-Z ]*PRIVATE KEY-----"#,
        #"(?i)bearer\s+[A-Za-z0-9._\-]{24,}"#,
    ].compactMap { try? NSRegularExpression(pattern: $0) }

    public static func firstHit(in data: Data) -> Int? {
        let text = String(decoding: data, as: UTF8.self)
        for (i, line) in text.components(separatedBy: "\n").enumerated() {
            let r = NSRange(line.startIndex..., in: line)
            if patterns.contains(where: { $0.firstMatch(in: line, range: r) != nil }) { return i + 1 }
        }
        return nil
    }
}
