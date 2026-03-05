import Foundation

struct Logger {
    typealias Metadata = [String: String]

    enum Level: String, Comparable {
        case debug
        case info
        case error

        static func < (lhs: Level, rhs: Level) -> Bool {
            lhs.order < rhs.order
        }

        fileprivate var order: Int {
            switch self {
            case .debug: return 0
            case .info:  return 1
            case .error: return 2
            }
        }
    }

    /// Set via LUME_LOG_LEVEL env var ("debug", "info", "error"). Defaults to "info".
    static let minLevel: Level = {
        if let env = ProcessInfo.processInfo.environment["LUME_LOG_LEVEL"],
           let level = Level(rawValue: env.lowercased()) {
            return level
        }
        return .info
    }()

    static func info(_ message: String, metadata: Metadata = [:]) {
        log(.info, message, metadata)
    }

    static func error(_ message: String, metadata: Metadata = [:]) {
        log(.error, message, metadata)
    }

    static func debug(_ message: String, metadata: Metadata = [:]) {
        log(.debug, message, metadata)
    }

    private static func log(_ level: Level, _ message: String, _ metadata: Metadata) {
        guard level >= minLevel else { return }
        let timestamp = ISO8601DateFormatter().string(from: Date())
        let metadataString = metadata.isEmpty ? "" : " " + metadata.map { "\($0.key)=\($0.value)" }.joined(separator: " ")
        print("[\(timestamp)] \(level.rawValue.uppercased()): \(message)\(metadataString)")
    }
}