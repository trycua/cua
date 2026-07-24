import Darwin
import Foundation

enum TelemetryTransport: String, Sendable {
  case cli
  case http
  case mcpStdio = "mcp_stdio"
}

enum TelemetryErrorClass: String, Sendable {
  case none
  case invalidParams = "invalid_params"
  case notFound = "not_found"
  case conflict
  case permissionDenied = "permission_denied"
  case unavailable
  case transportError = "transport_error"
  case operationError = "operation_error"
  case internalError = "internal_error"
  case unknownTool = "unknown_tool"
}

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
    static let lifecycleLockFileName = ".telemetry_lifecycle.lock"
    static let envTelemetryEnabled = "LUME_TELEMETRY_ENABLED"
    static let envTelemetryDebug = "LUME_TELEMETRY_DEBUG"
    static let envTelemetrySynthetic = "LUME_TELEMETRY_SYNTHETIC"
    static let envInstallChannel = "LUME_INSTALL_CHANNEL"
    static let requestTimeout: TimeInterval = 3
    static let flushTimeout: TimeInterval = 1.5
    static let lifecycleRetryBackoff: TimeInterval = 15 * 60
    static let schemaVersion = 3
    static let routineEventLimitPerHour = 1_000
    static let routineEventWindow: TimeInterval = 60 * 60
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
  private let rateLimitLock = NSLock()
  private var rateLimitWindowStarted = Date()
  private var rateLimitCaptured = 0
  private var rateLimitCapturedValueEvent = false
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
    guard TelemetryEvent.allowed.contains(event), isEnabled(),
      shouldCaptureRoutineEvent(event: event),
      let request = makeRequest(event: event, properties: properties)
    else {
      return
    }

    pendingRequests.enter()
    Task.detached { [self] in
      _ = await send(request: request, event: event)
      pendingRequests.leave()
    }
  }

  func recordOperationCompleted(
    operation: String,
    transport: TelemetryTransport,
    success: Bool,
    errorClass: TelemetryErrorClass,
    elapsed: TimeInterval,
    guestOS: String? = nil,
    phase: String = "completed"
  ) {
    var properties: [String: Any] = [
      "operation": Self.normalizedOperation(operation),
      "transport": transport.rawValue,
      "success": success,
      "error_class": success ? TelemetryErrorClass.none.rawValue : errorClass.rawValue,
      "duration_bucket": Self.durationBucket(elapsed),
      "phase": phase == "accepted" ? "accepted" : "completed",
    ]
    if let guestOS {
      properties["guest_os"] = Self.normalizedGuestOS(guestOS)
    }
    record(event: TelemetryEvent.operationCompleted, properties: properties)
  }

  func recordMCPToolCompleted(
    toolName: String,
    success: Bool,
    errorClass: TelemetryErrorClass,
    elapsed: TimeInterval
  ) {
    record(
      event: TelemetryEvent.mcpToolCompleted,
      properties: [
        "tool_name": Self.normalizedMCPTool(toolName),
        "operation": Self.normalizedMCPToolOperation(toolName),
        "transport": TelemetryTransport.mcpStdio.rawValue,
        "success": success,
        "error_class": success ? TelemetryErrorClass.none.rawValue : errorClass.rawValue,
        "duration_bucket": Self.durationBucket(elapsed),
      ])
  }

  func recordMCPSessionStarted() {
    record(
      event: TelemetryEvent.mcpSessionStarted,
      properties: [
        "transport": TelemetryTransport.mcpStdio.rawValue,
        "protocol": "mcp",
      ])
  }

  func recordVMStarted(transport: TelemetryTransport, guestOS: String) {
    record(
      event: TelemetryEvent.vmStarted,
      properties: [
        "transport": transport.rawValue,
        "guest_os": Self.normalizedGuestOS(guestOS),
        "success": true,
      ])
  }

  func recordProvisioningStarted(
    transport: TelemetryTransport, guestOS: String, asynchronous: Bool
  ) {
    record(
      event: TelemetryEvent.provisioningStarted,
      properties: [
        "transport": transport.rawValue,
        "guest_os": Self.normalizedGuestOS(guestOS),
        "asynchronous": asynchronous,
      ])
  }

  func recordProvisioningCompleted(
    transport: TelemetryTransport,
    guestOS: String,
    asynchronous: Bool,
    success: Bool,
    errorClass: TelemetryErrorClass,
    elapsed: TimeInterval
  ) {
    record(
      event: TelemetryEvent.provisioningCompleted,
      properties: [
        "transport": transport.rawValue,
        "guest_os": Self.normalizedGuestOS(guestOS),
        "asynchronous": asynchronous,
        "success": success,
        "error_class": success ? TelemetryErrorClass.none.rawValue : errorClass.rawValue,
        "duration_bucket": Self.provisioningDurationBucket(elapsed),
      ])
  }

  func recordUpdateChecked(
    source: String, outcome: String, targetVersion: String?, cacheHit: Bool
  ) {
    var properties: [String: Any] = [
      "source": Self.normalizedUpdateSource(source),
      "outcome": Self.normalizedUpdateCheckOutcome(outcome),
      "cache_hit": cacheHit,
      "transport": source == "mcp"
        ? TelemetryTransport.mcpStdio.rawValue : TelemetryTransport.cli.rawValue,
    ]
    if let targetVersion = Self.strictReleaseVersion(targetVersion) {
      properties["target_version"] = targetVersion
    }
    record(event: TelemetryEvent.updateChecked, properties: properties)
  }

  func recordUpdateApplyStarted(targetVersion: String) {
    var properties: [String: Any] = [
      "transport": TelemetryTransport.cli.rawValue,
      "daemon_was_running": false,
    ]
    if let version = Self.strictReleaseVersion(targetVersion) {
      properties["target_version"] = version
    }
    record(event: TelemetryEvent.updateApplyStarted, properties: properties)
  }

  func recordUpdateApplyCompleted(
    targetVersion: String?, success: Bool, failureClass: String, elapsed: TimeInterval
  ) {
    var properties: [String: Any] = [
      "transport": TelemetryTransport.cli.rawValue,
      "outcome": success ? "success" : "failed",
      "failure_class": success ? "none" : Self.normalizedUpdateFailureClass(failureClass),
      "daemon_was_running": false,
      "duration_bucket": Self.durationBucket(elapsed),
    ]
    if let version = Self.strictReleaseVersion(targetVersion) {
      properties["target_version"] = version
    }
    record(event: TelemetryEvent.updateApplyCompleted, properties: properties)
  }

  /// Record the legacy installation event once and the release event once
  /// per Lume version. Both events honor consent. Each marker is written
  /// only after its corresponding request receives HTTP 2xx.
  func recordInstallation(channel: String? = nil) async {
    guard isEnabled(), !lifecycleRetryDeferred() else { return }

    guard let lockFile = acquireLifecycleLock() else { return }
    defer {
      flock(lockFile, LOCK_UN)
      close(lockFile)
    }

    // A concurrent process may have recorded the lifecycle while this process
    // waited for the lock, so marker state must only be read after acquisition.
    guard !lifecycleRetryDeferred() else { return }

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

  static func commandOperation(arguments: [String]) -> String {
    guard let raw = arguments.first else { return "help" }
    switch raw {
    case "create", "pull", "push", "convert", "images", "clone", "get", "set", "run",
      "stop", "ssh", "sip", "ipsw", "serve", "delete", "prune", "config", "logs",
      "update", "setup":
      return raw
    case "ls": return "list"
    case "check-update": return "check_update"
    case "dump-docs": return "dump_docs"
    case "help", "--help", "-h": return "help"
    default: return "other"
    }
  }

  static func httpOperation(method: String, routePath: String) -> String {
    let key = "\(method.uppercased()) \(routePath)"
    return [
      "GET /lume/vms": "list", "GET /lume/vms/:name": "get",
      "DELETE /lume/vms/:name": "delete", "POST /lume/vms": "create",
      "POST /lume/vms/clone": "clone", "PATCH /lume/vms/:name": "set",
      "POST /lume/vms/:name/run": "run", "POST /lume/vms/:name/stop": "stop",
      "POST /lume/vms/:name/setup": "setup", "GET /lume/ipsw": "ipsw",
      "POST /lume/pull": "pull", "POST /lume/pull/start": "pull_start",
      "POST /lume/prune": "prune", "GET /lume/images": "images",
      "GET /lume/config": "config_get", "POST /lume/config": "config_update",
      "GET /lume/config/locations": "locations_list",
      "POST /lume/config/locations": "locations_add",
      "DELETE /lume/config/locations/:name": "locations_remove",
      "POST /lume/config/locations/default/:name": "locations_default",
      "GET /lume/logs": "logs", "POST /lume/vms/push": "push",
      "GET /lume/host/status": "host_status",
    ][key] ?? "other"
  }

  static func durationBucket(_ elapsed: TimeInterval) -> String {
    switch max(0, elapsed) {
    case ..<0.1: "lt_100ms"
    case ..<0.5: "100_499ms"
    case ..<2: "500ms_1_999ms"
    case ..<10: "2s_9_999ms"
    default: "gte_10s"
    }
  }

  static func provisioningDurationBucket(_ elapsed: TimeInterval) -> String {
    switch max(0, elapsed) {
    case ..<60: "lt_1min"
    case ..<300: "1_4min"
    case ..<900: "5_14min"
    case ..<1_800: "15_29min"
    default: "gte_30min"
    }
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
    eventProperties["os_major"] = String(
      ProcessInfo.processInfo.operatingSystemVersion.majorVersion)
    eventProperties["arch"] = architecture()
    eventProperties["is_ci"] = isCI()
    eventProperties["is_synthetic"] = isSynthetic()
    eventProperties["transport"] =
      eventProperties["transport"] as? String ?? transport(event: event, properties: properties)
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
      return mode == "mcp" ? TelemetryTransport.mcpStdio.rawValue : TelemetryTransport.http.rawValue
    }
    return "cli"
  }

  private func sanitizeProperties(_ properties: [String: Any]) -> [String: Any] {
    let allowedKeys: Swift.Set<String> = [
      "asynchronous", "cache_hit", "cpu", "daemon_was_running", "disk_size",
      "disk_size_gb", "duration_bucket", "error_class", "failure_class", "guest_os",
      "has_unattended", "headless", "install_channel", "memory", "memory_gb", "mode",
      "operation", "os_type", "outcome", "phase", "product_version", "protocol", "source",
      "success", "target_version", "tool_name", "transport",
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
        if key == "target_version" {
          if let version = Self.strictReleaseVersion(value) {
            sanitized[key] = version
          }
        } else if Self.allowedPropertyValue(key: key, value: value) {
          sanitized[key] = value
        } else {
          sanitized[key] = "unknown"
        }
      default:
        continue
      }
    }
    return sanitized
  }

  private static func allowedPropertyValue(key: String, value: String) -> Bool {
    switch key {
    case "duration_bucket":
      return [
        "lt_100ms", "100_499ms", "500ms_1_999ms", "2s_9_999ms", "gte_10s",
        "lt_1min", "1_4min", "5_14min", "15_29min", "gte_30min",
      ].contains(value)
    case "error_class":
      return [
        "none", "invalid_params", "not_found", "conflict", "permission_denied",
        "unavailable", "transport_error", "operation_error", "internal_error", "unknown_tool",
      ].contains(value)
    case "failure_class":
      return ["none", "installer_exit", "installer_launch", "unknown"].contains(value)
    case "guest_os", "os_type":
      return ["macos", "linux", "windows", "unknown"].contains(value)
    case "install_channel":
      return Constants.allowedInstallChannels.contains(value)
    case "mode":
      return ["custom", "offline", "mcp", "http"].contains(value)
    case "operation":
      return normalizedOperation(value) == value
    case "outcome":
      return ["up_to_date", "available", "unavailable", "success", "failed"].contains(value)
    case "phase":
      return ["accepted", "completed"].contains(value)
    case "protocol":
      return value == "mcp"
    case "source":
      return ["cli", "mcp", "background"].contains(value)
    case "tool_name":
      return normalizedMCPTool(value) == value
    case "transport":
      return ["cli", "http", "mcp_stdio"].contains(value)
    case "product_version":
      return strictReleaseVersion(value) != nil
    default:
      return false
    }
  }

  private static func normalizedOperation(_ raw: String) -> String {
    let allowed: Swift.Set<String> = [
      "check_update", "clone", "config", "config_get", "config_update", "convert", "create",
      "delete", "dump_docs", "exec", "get", "help", "host_status", "images", "ipsw", "list",
      "locations_add", "locations_default", "locations_list", "locations_remove", "logs", "pull",
      "pull_start", "prune", "push", "resize", "run", "serve", "set", "setup", "sip", "ssh",
      "stop", "update",
    ]
    return allowed.contains(raw) ? raw : "other"
  }

  private static func normalizedMCPTool(_ raw: String) -> String {
    let allowed: Swift.Set<String> = [
      "check_for_update", "lume_clone_vm", "lume_create_vm", "lume_delete_vm", "lume_exec",
      "lume_get_vm", "lume_list_vms", "lume_resize_disk", "lume_run_vm", "lume_stop_vm",
    ]
    return allowed.contains(raw) ? raw : "other"
  }

  private static func normalizedMCPToolOperation(_ tool: String) -> String {
    [
      "check_for_update": "check_update", "lume_clone_vm": "clone", "lume_create_vm": "create",
      "lume_delete_vm": "delete", "lume_exec": "exec", "lume_get_vm": "get",
      "lume_list_vms": "list", "lume_resize_disk": "resize", "lume_run_vm": "run",
      "lume_stop_vm": "stop",
    ][tool] ?? "other"
  }

  private static func normalizedGuestOS(_ raw: String) -> String {
    let value = raw.lowercased()
    return ["macos", "linux", "windows"].contains(value) ? value : "unknown"
  }

  private static func normalizedUpdateSource(_ raw: String) -> String {
    ["cli", "mcp", "background"].contains(raw) ? raw : "background"
  }

  private static func normalizedUpdateCheckOutcome(_ raw: String) -> String {
    ["up_to_date", "available", "unavailable"].contains(raw) ? raw : "unavailable"
  }

  private static func normalizedUpdateFailureClass(_ raw: String) -> String {
    ["installer_exit", "installer_launch"].contains(raw) ? raw : "unknown"
  }

  static func strictReleaseVersion(_ raw: String?) -> String? {
    guard let raw else { return nil }
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    let normalized = value.hasPrefix("v") ? String(value.dropFirst()) : value
    guard !normalized.isEmpty, normalized.count <= 64, !normalized.contains("+") else { return nil }
    let split = normalized.split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
    let core = split[0].split(separator: ".", omittingEmptySubsequences: false)
    guard core.count == 3,
      core.allSatisfy({
        !$0.isEmpty && $0.count <= 6 && $0.allSatisfy({ $0.isASCII && $0.isNumber })
      })
    else { return nil }
    if split.count == 2 {
      let suffix = split[1]
      guard !suffix.isEmpty, suffix.count <= 32,
        suffix.allSatisfy({
          $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "." || $0 == "-")
        })
      else { return nil }
    }
    return normalized
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

  private func shouldCaptureRoutineEvent(event: String, now: Date = Date()) -> Bool {
    rateLimitLock.lock()
    defer { rateLimitLock.unlock() }
    if now.timeIntervalSince(rateLimitWindowStarted) >= Constants.routineEventWindow {
      rateLimitWindowStarted = now
      rateLimitCaptured = 0
      rateLimitCapturedValueEvent = false
    }
    let isValueEvent = event == TelemetryEvent.vmStarted
    guard rateLimitCaptured < Constants.routineEventLimitPerHour else {
      guard isValueEvent, !rateLimitCapturedValueEvent else { return false }
      rateLimitCapturedValueEvent = true
      return true
    }
    rateLimitCaptured += 1
    rateLimitCapturedValueEvent = rateLimitCapturedValueEvent || isValueEvent
    return true
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

  private var lifecycleLockURL: URL {
    homeDirectory.appendingPathComponent(Constants.lifecycleLockFileName)
  }

  private func acquireLifecycleLock() -> Int32? {
    do {
      try FileManager.default.createDirectory(at: homeDirectory, withIntermediateDirectories: true)
    } catch {
      debugLog("Failed to create lifecycle lock directory: \(error.localizedDescription)")
      return nil
    }
    let descriptor = Darwin.open(lifecycleLockURL.path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
    guard descriptor >= 0 else { return nil }
    guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
      close(descriptor)
      return nil
    }
    return descriptor
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

  private func isSynthetic() -> Bool {
    ProcessInfo.processInfo.environment[Constants.envTelemetrySynthetic]
      .flatMap(Self.parseBoolean) ?? false
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
  static let convert = "lume_convert"
  static let pull = "lume_pull"
  static let push = "lume_push"
  static let importUTM = "lume_import_utm"
  static let exportUTM = "lume_export_utm"
  static let serve = "lume_serve"
  static let setup = "lume_setup"
  static let config = "lume_config"
  static let sip = "lume_sip"
  static let ssh = "lume_ssh"

  static let operationCompleted = "lume_operation_completed"
  static let mcpSessionStarted = "lume_mcp_session_started"
  static let mcpToolCompleted = "lume_mcp_tool_completed"
  static let vmStarted = "lume_vm_started"
  static let provisioningStarted = "lume_provisioning_started"
  static let provisioningCompleted = "lume_provisioning_completed"
  static let updateChecked = "lume_update_checked"
  static let updateApplyStarted = "lume_update_apply_started"
  static let updateApplyCompleted = "lume_update_apply_completed"

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

  static let allowed: Swift.Set<String> = [
    install, releaseInstalled, create, run, stop, delete, clone, convert, pull, push, serve, setup,
    config,
    sip, ssh, operationCompleted, mcpSessionStarted, mcpToolCompleted, vmStarted,
    provisioningStarted,
    provisioningCompleted, updateChecked, updateApplyStarted, updateApplyCompleted, apiVMList,
    apiVMGet, apiVMCreate, apiVMRun, apiVMStop, apiVMDelete, apiVMClone, apiVMUpdate, apiPull,
    apiPush, apiImages,
  ]
}
