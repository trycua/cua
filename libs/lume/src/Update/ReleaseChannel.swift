import Foundation

enum LumeReleaseChannel: String, Codable, CaseIterable {
    case stable
    case nightly

    static let fileName = "release-channel"

    static func current(for version: String) -> LumeReleaseChannel? {
        if plainVersion(version) != nil { return .stable }
        if nightlyVersion(version) != nil { return .nightly }
        return nil
    }

    static func selected() throws -> LumeReleaseChannel {
        try selected(at: stateFileURL())
    }

    static func selected(at url: URL) throws -> LumeReleaseChannel {
        guard FileManager.default.fileExists(atPath: url.path) else { return .stable }
        let value = try String(contentsOf: url, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let channel = LumeReleaseChannel(rawValue: value) else {
            throw LumeReleaseChannelError.invalid(value: value, path: url.path)
        }
        return channel
    }

    static func set(_ channel: LumeReleaseChannel) throws {
        try set(channel, at: stateFileURL())
    }

    static func set(_ channel: LumeReleaseChannel, at url: URL) throws {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data("\(channel.rawValue)\n".utf8).write(to: url, options: .atomic)
    }

    static func stateFileURL() -> URL {
        if let override = ProcessInfo.processInfo.environment["LUME_HOME"], !override.isEmpty {
            return URL(fileURLWithPath: override).appendingPathComponent(fileName)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".lume")
            .appendingPathComponent(fileName)
    }

    static func plainVersion(_ version: String) -> [Int]? {
        let parts = version.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3,
            parts.allSatisfy({ !$0.isEmpty && $0.allSatisfy(\.isNumber) }),
            parts.allSatisfy({ $0 == "0" || !$0.hasPrefix("0") })
        else { return nil }
        let numbers = parts.compactMap { Int($0) }
        return numbers.count == 3 ? numbers : nil
    }

    static func nightlyVersion(_ version: String) -> [Int]? {
        let pieces = version.components(separatedBy: "-nightly.")
        guard pieces.count == 2, let base = plainVersion(pieces[0]) else { return nil }
        let suffix = pieces[1].split(separator: ".", omittingEmptySubsequences: false)
        guard suffix.count == 2,
            suffix[0].count == 8,
            suffix[0].allSatisfy(\.isNumber),
            let date = Int(suffix[0]),
            let run = Int(suffix[1]),
            !suffix[1].hasPrefix("0"),
            run > 0
        else { return nil }
        return base + [date, run]
    }
}

enum LumeReleaseChannelError: LocalizedError {
    case invalid(value: String, path: String)

    var errorDescription: String? {
        switch self {
        case .invalid(let value, let path):
            return "Invalid release channel '\(value)' in \(path); run `lume channel set stable` or `lume channel set nightly` to repair it."
        }
    }
}
