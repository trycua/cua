import Foundation

/// Reads the daemon's log files for `GET /lume/logs`. The paths come from
/// `LumeChannel` (so `lume-local` reads its own, wherever `LUME_LOCAL_LOG_DIR`
/// put them), and are passed in so the reader can be tested against any
/// directory.
enum DaemonLogs {
    /// `info` and/or `error`, per `type` (`info`, `error`, or `all`, the
    /// default), each the last `lines` lines of its file when given.
    static func response(
        type: String?,
        lines: Int?,
        infoPath: String,
        errorPath: String
    ) -> [String: String] {
        let logType = type?.lowercased() ?? "all"
        var response: [String: String] = [:]

        if logType == "info" || logType == "all" {
            response["info"] = read(path: infoPath, lines: lines) ?? "Info log file not found"
        }
        if logType == "error" || logType == "all" {
            response["error"] = read(path: errorPath, lines: lines) ?? "Error log file not found"
        }
        return response
    }

    private static func read(path: String, lines: Int?) -> String? {
        guard FileManager.default.fileExists(atPath: path) else {
            return nil
        }

        do {
            let content = try String(contentsOfFile: path, encoding: .utf8)

            // If lines parameter is provided, return only the specified number of lines from the end
            if let lineCount = lines {
                let allLines = content.components(separatedBy: .newlines)
                let startIndex = max(0, allLines.count - lineCount)
                let lastLines = Array(allLines[startIndex...])
                return lastLines.joined(separator: "\n")
            }

            return content
        } catch {
            return "Error reading log file: \(error.localizedDescription)"
        }
    }
}
