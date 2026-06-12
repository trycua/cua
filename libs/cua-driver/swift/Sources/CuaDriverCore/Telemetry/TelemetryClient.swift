import Foundation

/// Anonymous usage-tracking client — a port of lume's telemetry
/// with the same PostHog HTTP-capture pattern, per-install UUID,
/// env-override support, and installation-record-once behavior.
///
/// Differences from lume:
///  - Events are prefixed `cua_driver_` instead of `lume_`.
///  - Installation ID + marker live under `~/.cua-driver/` instead
///    of `~/.lume/`.
///  - Env overrides are `CUA_DRIVER_TELEMETRY_ENABLED` and
///    `CUA_DRIVER_TELEMETRY_DEBUG`.
///  - Opt-out flag is persisted inside our existing `CuaDriverConfig`
///    (`telemetryEnabled` field) via `ConfigStore`, rather than a
///    separate YAML like lume's.
///
/// Privacy posture matches lume: we send the driver version, OS,
/// OS version, CPU architecture, CI-environment flag, and a stable
/// per-install UUID. We do **not** send usernames, file paths,
/// command arguments, or anything user-typed. See `privacy` notes
/// in the package for the audit trail.
public final class TelemetryClient: @unchecked Sendable {
    // MARK: - Constants

    private enum Constants {
        static let apiKey = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14"
        static let captureURL = "https://eu.i.posthog.com/capture/"
        static let telemetryIdFileName = ".telemetry_id"
        static let installationRecordedFileName = ".installation_recorded"
        static let homeSubdirectory = "~/.cua-driver"
        static let envTelemetryEnabled = "CUA_DRIVER_TELEMETRY_ENABLED"
        static let envTelemetryDebug = "CUA_DRIVER_TELEMETRY_DEBUG"
    }

    // MARK: - Singleton

    public static let shared = TelemetryClient()

    // MARK: - Properties

    private var installationId: String?
    private let urlSession: URLSession

    // MARK: - Initialization

    private init() {
        // Short timeouts — telemetry must never block a CLI call.
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 10
        config.timeoutIntervalForResource = 10
        self.urlSession = URLSession(configuration: config)
    }

    // MARK: - Public entry points

    /// Record a telemetry event. Respects the opt-out check and
    /// becomes a no-op when telemetry is disabled.
    ///
    /// - Parameters:
    ///   - event: Event name (e.g. `"cua_driver_mcp"`).
    ///   - properties: Additional event properties merged on top of
    ///     the default envelope (version / OS / arch / etc.).
    public func record(event: String, properties: [String: Any] = [:]) {
        guard isEnabledSync() else { return }
        if installationId == nil {
            installationId = getOrCreateInstallationId()
        }
        sendEvent(event: event, properties: properties, bypassOptOut: false)
    }

    /// Record the one-time installation event.
    ///
    /// Sent exactly once per install (guarded by the
    /// `.installation_recorded` marker file), **bypassing** the
    /// opt-out check so we can count adoption even when a user
    /// disables telemetry immediately after install. Every
    /// subsequent event honors the opt-out normally.
    public func recordInstallation() {
        let homeDir = (Constants.homeSubdirectory as NSString).expandingTildeInPath
        let markerPath = "\(homeDir)/\(Constants.installationRecordedFileName)"
        if FileManager.default.fileExists(atPath: markerPath) {
            return
        }

        let installId = getOrCreateInstallationId()
        self.installationId = installId
        sendEvent(event: TelemetryEvent.install, properties: [:], bypassOptOut: true)

        // Persist marker so subsequent launches skip re-sending.
        do {
            try FileManager.default.createDirectory(
                atPath: homeDir, withIntermediateDirectories: true
            )
            try "1".write(toFile: markerPath, atomically: true, encoding: .utf8)
        } catch {
            if isDebugEnabled() {
                FileHandle.standardError.write(Data(
                    "[telemetry] failed to write install marker: \(error.localizedDescription)\n".utf8
                ))
            }
        }
    }

    /// Whether telemetry is currently enabled. Env var takes
    /// precedence; otherwise the persisted config flag is consulted.
    public func isEnabledSync() -> Bool {
        if let envValue = ProcessInfo.processInfo.environment[Constants.envTelemetryEnabled] {
            let lowercased = envValue.lowercased()
            if ["0", "false", "no", "off"].contains(lowercased) { return false }
            if ["1", "true", "yes", "on"].contains(lowercased) { return true }
        }
        // Fall back to the persisted config. `load()` is async, but
        // we need a sync answer here — use the nonisolated static
        // variant that re-reads the file directly.
        return ConfigStore.loadSync().telemetryEnabled
    }

    // MARK: - Private

    private func sendEvent(event: String, properties: [String: Any], bypassOptOut: Bool) {
        guard bypassOptOut || isEnabledSync() else { return }
        let distinctId = installationId ?? getOrCreateInstallationId()
        let version = CuaDriverCore.version

        var eventProperties = properties
        eventProperties["cua_driver_version"] = version
        eventProperties["os"] = "macos"
        eventProperties["os_version"] = ProcessInfo.processInfo.operatingSystemVersionString
        eventProperties["arch"] = Self.architecture
        eventProperties["is_ci"] = Self.isCI
        eventProperties["$lib"] = "cua-driver-swift"
        eventProperties["$lib_version"] = version

        let payload: [String: Any] = [
            "api_key": Constants.apiKey,
            "event": event,
            "distinct_id": distinctId,
            "properties": eventProperties,
            "timestamp": ISO8601DateFormatter().string(from: Date()),
        ]

        guard
            let url = URL(string: Constants.captureURL),
            let body = try? JSONSerialization.data(withJSONObject: payload)
        else {
            if isDebugEnabled() {
                FileHandle.standardError.write(Data("[telemetry] failed to build payload\n".utf8))
            }
            return
        }

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body

        let debug = isDebugEnabled()
        if debug {
            FileHandle.standardError.write(Data(
                "[telemetry] sending event: \(event)\n".utf8
            ))
        }

        // Fire and forget — telemetry must never delay the CLI
        // caller's actual work.
        Task.detached { [urlSession] in
            do {
                let (_, response) = try await urlSession.data(for: request)
                if debug {
                    let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                    FileHandle.standardError.write(Data(
                        "[telemetry] \(event) status: \(status)\n".utf8
                    ))
                }
            } catch {
                if debug {
                    FileHandle.standardError.write(Data(
                        "[telemetry] \(event) failed: \(error.localizedDescription)\n".utf8
                    ))
                }
            }
        }
    }

    private func isDebugEnabled() -> Bool {
        if let value = ProcessInfo.processInfo.environment[Constants.envTelemetryDebug] {
            return ["1", "true", "yes", "on"].contains(value.lowercased())
        }
        return false
    }

    private func getOrCreateInstallationId() -> String {
        let homeDir = (Constants.homeSubdirectory as NSString).expandingTildeInPath
        let idFilePath = "\(homeDir)/\(Constants.telemetryIdFileName)"

        // Try existing ID first.
        if let existing = try? String(contentsOfFile: idFilePath, encoding: .utf8) {
            let trimmed = existing.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty { return trimmed }
        }

        // Generate + persist a fresh UUID.
        let newId = UUID().uuidString
        do {
            try FileManager.default.createDirectory(
                atPath: homeDir, withIntermediateDirectories: true
            )
            try newId.write(toFile: idFilePath, atomically: true, encoding: .utf8)
        } catch {
            if isDebugEnabled() {
                FileHandle.standardError.write(Data(
                    "[telemetry] failed to persist install id: \(error.localizedDescription)\n".utf8
                ))
            }
        }
        return newId
    }

    // MARK: - Static environment

    private static let architecture: String = {
        #if arch(arm64)
        return "arm64"
        #elseif arch(x86_64)
        return "x86_64"
        #else
        return "unknown"
        #endif
    }()

    private static let isCI: Bool = {
        // Standard CI-environment signals across GitHub Actions,
        // GitLab, Jenkins, CircleCI, and the generic CI=true flag.
        let envVars = [
            "CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS",
            "GITLAB_CI", "JENKINS_URL", "CIRCLECI",
        ]
        let env = ProcessInfo.processInfo.environment
        return envVars.contains { env[$0] != nil }
    }()
}

// MARK: - Event names

/// Canonical event name constants. Each CLI command and MCP tool
/// records its own name so opt-in users can see which surfaces are
/// hit in aggregate without us inspecting arguments or content.
public enum TelemetryEvent {
    /// One-time installation ping. Sent regardless of opt-out (see
    /// `recordInstallation`); all other events respect the flag.
    public static let install = "cua_driver_install"

    // CLI entry points
    public static let mcp = "cua_driver_mcp"
    public static let serve = "cua_driver_serve"
    public static let stop = "cua_driver_stop"
    public static let status = "cua_driver_status"
    public static let call = "cua_driver_call"
    public static let listTools = "cua_driver_list_tools"
    public static let describe = "cua_driver_describe"
    public static let recording = "cua_driver_recording"
    public static let config = "cua_driver_config"
    public static let guiLaunch = "cua_driver_gui_launch"

    /// Prefix for per-MCP-tool events. `apiPrefix + tool_name`
    /// produces e.g. `cua_driver_api_click`.
    public static let apiPrefix = "cua_driver_api_"
}
