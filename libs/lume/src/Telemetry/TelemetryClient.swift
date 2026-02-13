import Foundation

/// Singleton telemetry client for anonymous usage tracking
/// Uses PostHog HTTP API directly for CLI compatibility
final class TelemetryClient: @unchecked Sendable {
    // MARK: - Constants

    private enum Constants {
        static let apiKey = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14"
        static let captureURL = "https://eu.i.posthog.com/capture/"
        static let telemetryIdFileName = ".telemetry_id"
        static let installationRecordedFileName = ".installation_recorded"
        static let envTelemetryEnabled = "LUME_TELEMETRY_ENABLED"
        static let envTelemetryDebug = "LUME_TELEMETRY_DEBUG"
    }

    // MARK: - Singleton

    static let shared = TelemetryClient()

    // MARK: - Properties

    private var installationId: String?
    private let settingsManager: SettingsManager
    private let urlSession: URLSession

    // MARK: - Initialization

    private init(settingsManager: SettingsManager = .shared) {
        self.settingsManager = settingsManager

        // Configure URL session with short timeout (don't block CLI)
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 10
        config.timeoutIntervalForResource = 10
        self.urlSession = URLSession(configuration: config)
    }

    // MARK: - Public Methods

    /// Record a telemetry event
    /// - Parameters:
    ///   - event: Event name (e.g., "lume_create", "lume_api_vm_run")
    ///   - properties: Additional event properties
    func record(event: String, properties: [String: Any] = [:]) {
        guard isEnabled() else { return }

        // Lazy load installation ID
        if installationId == nil {
            installationId = getOrCreateInstallationId()
        }

        sendEvent(event: event, properties: properties, bypassOptOut: false)
    }

    /// Record installation event - sent ONCE on first run, regardless of telemetry opt-out
    /// This tracks adoption while respecting user's choice to opt out of ongoing telemetry
    func recordInstallation() {
        let homeDir = (("~/.lume") as NSString).expandingTildeInPath
        let markerPath = "\(homeDir)/\(Constants.installationRecordedFileName)"

        // Check if we've already recorded installation
        if FileManager.default.fileExists(atPath: markerPath) {
            return
        }

        // Get or create installation ID
        let installId = getOrCreateInstallationId()
        self.installationId = installId

        // Send installation event (bypasses isEnabled check)
        sendEvent(event: TelemetryEvent.install, properties: [:], bypassOptOut: true)

        // Mark as recorded
        do {
            try FileManager.default.createDirectory(atPath: homeDir, withIntermediateDirectories: true)
            try "1".write(toFile: markerPath, atomically: true, encoding: .utf8)
        } catch {
            if isDebugEnabled() {
                Logger.info("[Telemetry] Failed to write installation marker: \(error.localizedDescription)")
            }
        }

        if isDebugEnabled() {
            Logger.info("[Telemetry] Installation event recorded for: \(installId)")
        }
    }

    /// Check if telemetry is enabled
    /// - Returns: true if telemetry should be collected
    func isEnabled() -> Bool {
        // Check environment variable first (highest priority for CI/CD)
        if let envValue = ProcessInfo.processInfo.environment[Constants.envTelemetryEnabled] {
            let lowercased = envValue.lowercased()
            if ["0", "false", "no", "off"].contains(lowercased) {
                return false
            }
            if ["1", "true", "yes", "on"].contains(lowercased) {
                return true
            }
        }

        // Check settings
        return settingsManager.getSettings().telemetryEnabled
    }

    // MARK: - Private Methods

    private func sendEvent(event: String, properties: [String: Any], bypassOptOut: Bool) {
        guard bypassOptOut || isEnabled() else { return }

        let distinctId = installationId ?? getOrCreateInstallationId()

        // Build event properties
        var eventProperties = properties
        eventProperties["lume_version"] = Lume.Version.current
        eventProperties["os"] = "macos"
        eventProperties["os_version"] = ProcessInfo.processInfo.operatingSystemVersionString
        eventProperties["arch"] = getArchitecture()
        eventProperties["is_ci"] = isCI()
        eventProperties["$lib"] = "lume-swift"
        eventProperties["$lib_version"] = Lume.Version.current

        // Build PostHog capture payload
        let payload: [String: Any] = [
            "api_key": Constants.apiKey,
            "event": event,
            "distinct_id": distinctId,
            "properties": eventProperties,
            "timestamp": ISO8601DateFormatter().string(from: Date())
        ]

        guard let url = URL(string: Constants.captureURL),
              let jsonData = try? JSONSerialization.data(withJSONObject: payload) else {
            if isDebugEnabled() {
                Logger.info("[Telemetry] Failed to serialize event payload")
            }
            return
        }

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = jsonData

        if isDebugEnabled() {
            Logger.info("[Telemetry] Sending event: \(event)", metadata: ["properties": "\(eventProperties)"])
        }

        // Fire and forget - don't block CLI execution
        let debugEnabled = isDebugEnabled()
        Task.detached {
            do {
                let (_, response) = try await self.urlSession.data(for: request)
                if debugEnabled {
                    let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 0
                    Logger.info("[Telemetry] Event sent: \(event), status: \(statusCode)")
                }
            } catch {
                if debugEnabled {
                    Logger.info("[Telemetry] Failed to send event \(event): \(error.localizedDescription)")
                }
            }
        }
    }

    private func isDebugEnabled() -> Bool {
        if let envValue = ProcessInfo.processInfo.environment[Constants.envTelemetryDebug] {
            return ["on", "true", "1", "yes"].contains(envValue.lowercased())
        }
        return false
    }

    private func getOrCreateInstallationId() -> String {
        let homeDir = (("~/.lume") as NSString).expandingTildeInPath
        let idFilePath = "\(homeDir)/\(Constants.telemetryIdFileName)"

        // Try to read existing ID
        if let existingId = try? String(contentsOfFile: idFilePath, encoding: .utf8) {
            let trimmedId = existingId.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmedId.isEmpty {
                return trimmedId
            }
        }

        // Generate new ID
        let newId = UUID().uuidString

        // Try to persist it
        do {
            try FileManager.default.createDirectory(
                atPath: homeDir,
                withIntermediateDirectories: true
            )
            try newId.write(toFile: idFilePath, atomically: true, encoding: .utf8)
        } catch {
            if isDebugEnabled() {
                Logger.info("[Telemetry] Failed to persist installation ID: \(error.localizedDescription)")
            }
        }

        return newId
    }

    private func getArchitecture() -> String {
        #if arch(arm64)
        return "arm64"
        #elseif arch(x86_64)
        return "x86_64"
        #else
        return "unknown"
        #endif
    }

    private func isCI() -> Bool {
        let ciEnvVars = ["CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS", "GITLAB_CI", "JENKINS_URL", "CIRCLECI"]
        for envVar in ciEnvVars {
            if ProcessInfo.processInfo.environment[envVar] != nil {
                return true
            }
        }
        return false
    }
}

// MARK: - Telemetry Events

/// Standard telemetry event names
enum TelemetryEvent {
    // Installation (sent once, regardless of opt-out)
    static let install = "lume_install"

    // CLI Commands
    static let create = "lume_create"
    static let run = "lume_run"
    static let stop = "lume_stop"
    static let delete = "lume_delete"
    static let clone = "lume_clone"
    static let resize = "lume_resize"
    static let pull = "lume_pull"
    static let push = "lume_push"
    static let serve = "lume_serve"
    static let setup = "lume_setup"
    static let config = "lume_config"

    // API Endpoints
    static let apiVMList = "lume_api_vm_list"
    static let apiVMGet = "lume_api_vm_get"
    static let apiVMCreate = "lume_api_vm_create"
    static let apiVMRun = "lume_api_vm_run"
    static let apiVMStop = "lume_api_vm_stop"
    static let apiVMDelete = "lume_api_vm_delete"
    static let apiVMClone = "lume_api_vm_clone"
    static let apiVMUpdate = "lume_api_vm_update"
    static let apiPull = "lume_api_pull"
    static let apiPush = "lume_api_push"
    static let apiImages = "lume_api_images"
}
