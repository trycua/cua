import Foundation

/// Pseudonymous, content-free product telemetry for Lume.
///
/// Telemetry is enabled by default, can be persistently disabled through
/// `lume config telemetry disable`, and can be overridden for the current
/// process with `LUME_TELEMETRY_ENABLED`. The environment always has highest
/// precedence. Installation and release events obey the same consent decision
/// as routine usage events.
final class TelemetryClient: @unchecked Sendable {
  private enum Constants {
    static let apiKey = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14"
    static let captureURL = "https://eu.i.posthog.com/capture/"
    static let telemetryIdFileName = ".telemetry_id"
    static let installationRecordedFileName = ".installation_recorded"
    static let releaseRecordedDirectoryName = ".telemetry_releases"
    static let lifecycleRetryAfterFileName = ".telemetry_retry_after"
    static let envTelemetryEnabled = "LUME_TELEMETRY_ENABLED"
    static let envTelemetryDebug = "LUME_TELEMETRY_DEBUG"
    static let envInstallChannel = "LUME_INSTALL_CHANNEL"
    static let requestTimeout: TimeInterval = 3
    static let flushTimeout: TimeInterval = 1.5
    static let lifecycleRetryBackoff: TimeInterval = 15 * 60
    static let schemaVersion = 2
    static let processSessionId = UUID().uuidString
    static let allowedInstallChannels: Swift.Set<String> = [
      "install_script", "update_apply", "first_run",
    ]
  }

  struct Status: Sendable {
    let enabled: Bool
    let source: String
    let persistedEnabled: Bool
    let installationIdPresent: Bool
  }

  private struct InstallationIdentity {
    let value: String
    let persisted: Bool
  }

  typealias Poster = @Sendable (URLRequest) async -> Bool

  static let shared = TelemetryClient()

  private var installationIdentity: InstallationIdentity?
  private let urlSession: URLSession
  private let homeDirectory: URL
  private let poster: Poster?
  private let preferenceProvider: @Sendable () -> Bool
  private let identityLock = NSLock()
  private let pendingRequests = DispatchGroup()

  init(
    settingsManager: SettingsManager = .shared,
    urlSession: URLSession? = nil,
    homeDirectory: URL? = nil,
    poster: Poster? = nil,
    preferenceProvider: (@Sendable () -> Bool)? = nil
  ) {
    self.homeDirectory =
      homeDirectory
      ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".lume")
    self.poster = poster
    self.preferenceProvider =
      preferenceProvider ?? { settingsManager.getSettings().telemetryEnabled }

    if let urlSession {
      self.urlSession = urlSession
    } else {
      let config = URLSessionConfiguration.ephemeral
      config.timeoutIntervalForRequest = Constants.requestTimeout
      config.timeoutIntervalForResource = Constants.requestTimeout
      self.urlSession = URLSession(configuration: config)
    }
  }

  // MARK: - Public API

  /// Record a routine event without blocking the command. Only fixed,
  /// typed properties survive `sanitizeProperties`; strings originating
  /// from image names, VM names, paths, prompts, or arguments are dropped.
  func record(event: String, properties: [String: Any] = [:]) {
    guard isEnabled(), let request = makeRequest(event: event, properties: properties) else {
      return
    }

    pendingRequests.enter()
    Task.detached { [self] in
      _ = await send(request: request, event: event)
      pendingRequests.leave()
    }
  }

  /// Record the legacy installation event once and the release event once
  /// per Lume version. Both events honor consent. Each marker is written
  /// only after its corresponding request receives HTTP 2xx.
  func recordInstallation(channel: String? = nil) async {
    guard isEnabled(), !lifecycleRetryDeferred() else { return }

    let installChannel = Self.normalizedInstallChannel(
      channel ?? ProcessInfo.processInfo.environment[Constants.envInstallChannel]
    )
    let installMarker = installationMarkerURL
    let releaseMarker = releaseMarkerURL(version: Lume.Version.current)
    let needsInstall = !FileManager.default.fileExists(atPath: installMarker.path)
    let needsRelease = !FileManager.default.fileExists(atPath: releaseMarker.path)

    guard needsInstall || needsRelease else { return }

    let properties: [String: Any] = [
      "install_channel": installChannel,
      "product_version": Lume.Version.current,
    ]

    if needsInstall && needsRelease,
      let installRequest = makeRequest(event: TelemetryEvent.install, properties: properties),
      let releaseRequest = makeRequest(
        event: TelemetryEvent.releaseInstalled, properties: properties)
    {
      async let installAccepted = send(request: installRequest, event: TelemetryEvent.install)
      async let releaseAccepted = send(
        request: releaseRequest, event: TelemetryEvent.releaseInstalled)
      let (didInstall, didRelease) = await (installAccepted, releaseAccepted)
      let installRecorded = didInstall && writeMarker(installMarker)
      let releaseRecorded = didRelease && writeMarker(releaseMarker)
      finishLifecycleAttempt(succeeded: installRecorded && releaseRecorded)
      return
    }

    if needsInstall,
      let request = makeRequest(event: TelemetryEvent.install, properties: properties),
      await send(request: request, event: TelemetryEvent.install)
    {
      if !writeMarker(installMarker) {
        deferLifecycleRetry()
        return
      }
    } else if needsInstall {
      deferLifecycleRetry()
      return
    }

    if needsRelease,
      let request = makeRequest(
        event: TelemetryEvent.releaseInstalled, properties: properties),
      await send(request: request, event: TelemetryEvent.releaseInstalled)
    {
      if !writeMarker(releaseMarker) {
        deferLifecycleRetry()
        return
      }
    } else if needsRelease {
      deferLifecycleRetry()
      return
    }

    clearLifecycleRetry()
  }

  /// Wait a short, bounded interval for routine events queued by a
  /// short-lived CLI command. Long-running `serve` processes do not reach
  /// this path until shutdown.
  @discardableResult
  func flush(timeout: TimeInterval = Constants.flushTimeout) async -> Bool {
    let group = pendingRequests
    return await Task.detached {
      Self.waitForPendingRequests(group, timeout: timeout)
    }.value
  }

  private static func waitForPendingRequests(
    _ group: DispatchGroup, timeout: TimeInterval
  ) -> Bool {
    group.wait(timeout: .now() + timeout) == .success
  }

  func isEnabled() -> Bool {
    effectiveState().enabled
  }

  func status() -> Status {
    let state = effectiveState()
    return Status(
      enabled: state.enabled,
      source: state.source,
      persistedEnabled: state.persistedEnabled,
      installationIdPresent: FileManager.default.fileExists(atPath: installationIdURL.path)
    )
  }

  /// Delete the pseudonymous identity and its registration/release markers.
  /// The persisted enabled/disabled preference is intentionally retained.
  func resetInstallationId() throws {
    identityLock.lock()
    installationIdentity = nil
    identityLock.unlock()

    for url in [installationIdURL, installationMarkerURL, lifecycleRetryAfterURL] {
      if FileManager.default.fileExists(atPath: url.path) {
        try FileManager.default.removeItem(at: url)
      }
    }
    if FileManager.default.fileExists(atPath: releaseMarkerDirectoryURL.path) {
      try FileManager.default.removeItem(at: releaseMarkerDirectoryURL)
    }
  }

  /// A direct release-asset/manual first run has no installer in which to
  /// show the default-on notice. Telemetry management commands skip this
  /// path so `disable` can be the first action without any event.
  func shouldShowFirstRunNotice() -> Bool {
    isEnabled() && !FileManager.default.fileExists(atPath: installationMarkerURL.path)
  }

  static func normalizedInstallChannel(_ raw: String?) -> String {
    guard let raw, Constants.allowedInstallChannels.contains(raw) else {
      return "first_run"
    }
    return raw
  }

  // MARK: - Consent and identity

  private func effectiveState() -> (enabled: Bool, source: String, persistedEnabled: Bool) {
    let persisted = preferenceProvider()
    if let raw = ProcessInfo.processInfo.environment[Constants.envTelemetryEnabled],
      let enabled = Self.parseBoolean(raw)
    {
      return (enabled, "environment", persisted)
    }
    return (persisted, "persisted", persisted)
  }

  private static func parseBoolean(_ raw: String) -> Bool? {
    switch raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
    case "0", "false", "no", "off": false
    case "1", "true", "yes", "on": true
    default: nil
    }
  }

  private func getOrCreateInstallationIdentity() -> InstallationIdentity {
    identityLock.lock()
    defer { identityLock.unlock() }

    if let installationIdentity { return installationIdentity }

    if let existing = try? String(contentsOf: installationIdURL, encoding: .utf8) {
      let trimmed = existing.trimmingCharacters(in: .whitespacesAndNewlines)
      if let parsed = UUID(uuidString: trimmed) {
        let identity = InstallationIdentity(value: parsed.uuidString, persisted: true)
        installationIdentity = identity
        return identity
      }
    }

    let newId = UUID().uuidString
    var persisted = false
    do {
      try FileManager.default.createDirectory(
        at: homeDirectory, withIntermediateDirectories: true)
      try newId.write(to: installationIdURL, atomically: true, encoding: .utf8)
      persisted = true
    } catch {
      debugLog("Failed to persist installation ID: \(error.localizedDescription)")
    }

    let identity = InstallationIdentity(value: newId, persisted: persisted)
    installationIdentity = identity
    return identity
  }

  // MARK: - Request construction and delivery

  private func makeRequest(event: String, properties: [String: Any]) -> URLRequest? {
    let identity = getOrCreateInstallationIdentity()
    var eventProperties = sanitizeProperties(properties)
    eventProperties["lume_version"] = Lume.Version.current
    eventProperties["product_version"] = Lume.Version.current
    eventProperties["telemetry_schema_version"] = Constants.schemaVersion
    eventProperties["os_family"] = "macos"
    eventProperties["os_major"] = String(ProcessInfo.processInfo.operatingSystemVersion.majorVersion)
    eventProperties["arch"] = architecture()
    eventProperties["is_ci"] = isCI()
    eventProperties["transport"] = transport(event: event, properties: properties)
    eventProperties["process_session_id"] = Constants.processSessionId
    eventProperties["id_persisted"] = identity.persisted
    eventProperties["$lib"] = "lume-swift"
    eventProperties["$lib_version"] = Lume.Version.current
    eventProperties["$process_person_profile"] = false
    eventProperties["$geoip_disable"] = true

    let payload: [String: Any] = [
      "api_key": Constants.apiKey,
      "event": event,
      "distinct_id": identity.value,
      "properties": eventProperties,
    ]

    guard let url = URL(string: Constants.captureURL),
      let jsonData = try? JSONSerialization.data(withJSONObject: payload)
    else {
      debugLog("Failed to serialize event payload")
      return nil
    }

    var request = URLRequest(url: url, timeoutInterval: Constants.requestTimeout)
    request.httpMethod = "POST"
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = jsonData
    return request
  }

  private func transport(event: String, properties: [String: Any]) -> String {
    if event.hasPrefix("lume_api_") { return "http" }
    if event == TelemetryEvent.serve, let mode = properties["mode"] as? String,
      mode == "mcp" || mode == "http"
    {
      return mode
    }
    return "cli"
  }

  private func sanitizeProperties(_ properties: [String: Any]) -> [String: Any] {
    let allowedKeys: Swift.Set<String> = [
      "cpu", "disk_size", "disk_size_gb", "has_unattended", "headless",
      "install_channel", "memory", "memory_gb", "mode", "os_type",
      "product_version",
    ]
    var sanitized: [String: Any] = [:]
    for (key, value) in properties where allowedKeys.contains(key) {
      switch value {
      case let value as Bool:
        sanitized[key] = value
      case let value as Int:
        sanitized[key] = value
      case let value as Int64:
        sanitized[key] = value
      case let value as Double where value.isFinite:
        sanitized[key] = value
      case let value as String:
        // Only bounded enums are accepted. User-derived strings such
        // as image names and custom preset names never pass through.
        let allowedValues: Swift.Set<String> = [
          "custom", "first_run", "install_script", "linux", "macos",
          "offline", "update_apply", "mcp", "http",
        ]
        sanitized[key] = allowedValues.contains(value) ? value : "unknown"
      default:
        continue
      }
    }
    return sanitized
  }

  private func send(request: URLRequest, event: String) async -> Bool {
    if let poster {
      return await poster(request)
    }

    do {
      let (_, response) = try await urlSession.data(for: request)
      let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 0
      debugLog("Event sent: \(event), status: \(statusCode)")
      return (200..<300).contains(statusCode)
    } catch {
      debugLog("Failed to send event \(event): \(error.localizedDescription)")
      return false
    }
  }

  // MARK: - Paths and platform fields

  private var installationIdURL: URL {
    homeDirectory.appendingPathComponent(Constants.telemetryIdFileName)
  }

  private var installationMarkerURL: URL {
    homeDirectory.appendingPathComponent(Constants.installationRecordedFileName)
  }

  private var releaseMarkerDirectoryURL: URL {
    homeDirectory.appendingPathComponent(Constants.releaseRecordedDirectoryName)
  }

  private var lifecycleRetryAfterURL: URL {
    homeDirectory.appendingPathComponent(Constants.lifecycleRetryAfterFileName)
  }

  private func releaseMarkerURL(version: String) -> URL {
    let safeVersion = version.map { character -> Character in
      character.isLetter || character.isNumber || character == "." || character == "-"
        ? character : "_"
    }
    return releaseMarkerDirectoryURL.appendingPathComponent(String(safeVersion))
  }

  @discardableResult
  private func writeMarker(_ url: URL) -> Bool {
    do {
      try FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
      try "1".write(to: url, atomically: true, encoding: .utf8)
      return true
    } catch {
      debugLog("Failed to write marker: \(error.localizedDescription)")
      return false
    }
  }

  private func lifecycleRetryDeferred(now: Date = Date()) -> Bool {
    guard
      let raw = try? String(contentsOf: lifecycleRetryAfterURL, encoding: .utf8),
      let retryAfter = TimeInterval(raw.trimmingCharacters(in: .whitespacesAndNewlines))
    else { return false }
    let remaining = retryAfter - now.timeIntervalSince1970
    return remaining > 0 && remaining <= Constants.lifecycleRetryBackoff
  }

  private func deferLifecycleRetry(now: Date = Date()) {
    let retryAfter = now.timeIntervalSince1970 + Constants.lifecycleRetryBackoff
    do {
      try FileManager.default.createDirectory(at: homeDirectory, withIntermediateDirectories: true)
      try String(retryAfter).write(
        to: lifecycleRetryAfterURL, atomically: true, encoding: .utf8)
    } catch {
      debugLog("Failed to defer lifecycle retry: \(error.localizedDescription)")
    }
  }

  private func clearLifecycleRetry() {
    guard FileManager.default.fileExists(atPath: lifecycleRetryAfterURL.path) else { return }
    do {
      try FileManager.default.removeItem(at: lifecycleRetryAfterURL)
    } catch {
      debugLog("Failed to clear lifecycle retry: \(error.localizedDescription)")
    }
  }

  private func finishLifecycleAttempt(succeeded: Bool) {
    if succeeded {
      clearLifecycleRetry()
    } else {
      deferLifecycleRetry()
    }
  }

  private func architecture() -> String {
    #if arch(arm64)
      "arm64"
    #elseif arch(x86_64)
      "x86_64"
    #else
      "unknown"
    #endif
  }

  private func isCI() -> Bool {
    let variables = [
      "CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS", "GITLAB_CI",
      "JENKINS_URL", "CIRCLECI", "BUILDKITE", "TF_BUILD", "TEAMCITY_VERSION",
    ]
    return variables.contains { ProcessInfo.processInfo.environment[$0] != nil }
  }

  private func debugLog(_ message: String) {
    guard let value = ProcessInfo.processInfo.environment[Constants.envTelemetryDebug],
      Self.parseBoolean(value) == true
    else { return }
    Logger.info("[Telemetry] \(message)")
  }
}

// MARK: - Telemetry Events

enum TelemetryEvent {
  static let install = "lume_install"
  static let releaseInstalled = "lume_release_installed"

  static let create = "lume_create"
  static let run = "lume_run"
  static let stop = "lume_stop"
  static let delete = "lume_delete"
  static let clone = "lume_clone"
  static let pull = "lume_pull"
  static let push = "lume_push"
  static let serve = "lume_serve"
  static let setup = "lume_setup"
  static let config = "lume_config"

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
