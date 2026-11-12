import Foundation

/// Lume's log lines. Every line goes through `Console`, which also owns the
/// live progress, so the two never write over each other.
struct Logger {
    typealias Metadata = [String: String]

    enum Level: String, Comparable {
        case debug
        case info
        case warning
        case error

        static func < (lhs: Level, rhs: Level) -> Bool {
            lhs.order < rhs.order
        }

        /// `LUME_LOG_LEVEL` value: `debug`, `info`, `warning` (or `warn`), `error`.
        init?(name: String) {
            let name = name.lowercased()
            self.init(rawValue: name == "warn" ? "warning" : name)
        }

        fileprivate var order: Int {
            switch self {
            case .debug: return 0
            case .info: return 1
            case .warning: return 2
            case .error: return 3
            }
        }
    }

    /// Set via LUME_LOG_LEVEL env var, or call setVerbose().
    nonisolated(unsafe) static var minLevel: Level = {
        if let env = ProcessInfo.processInfo.environment["LUME_LOG_LEVEL"],
            let level = Level(name: env)
        {
            return level
        }
        return .info
    }()

    /// Enable debug logging (called by --verbose flag).
    static func setVerbose() {
        minLevel = .debug
    }

    static func debug(_ message: String, metadata: Metadata = [:]) {
        log(.debug, message, metadata)
    }

    static func info(_ message: String, metadata: Metadata = [:]) {
        log(.info, message, metadata)
    }

    static func warning(_ message: String, metadata: Metadata = [:]) {
        log(.warning, message, metadata)
    }

    static func error(_ message: String, metadata: Metadata = [:]) {
        log(.error, message, metadata)
    }

    private static func log(_ level: Level, _ message: String, _ metadata: Metadata) {
        guard level >= minLevel else { return }
        Console.shared.log(level, message, metadata: metadata)
    }
}
