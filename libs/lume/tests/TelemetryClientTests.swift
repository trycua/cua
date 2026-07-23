import Foundation
import Testing

@testable import lume

private actor TelemetryRequestRecorder {
  private(set) var payloads: [[String: Any]] = []
  var accepted = true

  func post(_ request: URLRequest) -> Bool {
    if let data = request.httpBody,
      let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    {
      payloads.append(payload)
    }
    return accepted
  }

  func count() -> Int { payloads.count }

  func setAccepted(_ value: Bool) { accepted = value }

  func events() -> [String] {
    payloads.compactMap { $0["event"] as? String }
  }

  func hasContentFreePayload(event: String) -> Bool {
    guard
      let payload = payloads.first(where: { $0["event"] as? String == event }),
      let properties = payload["properties"] as? [String: Any]
    else { return false }
    return properties["image_name"] == nil
      && properties["headless"] as? Bool == true
      && properties["$process_person_profile"] as? Bool == false
      && properties["$geoip_disable"] as? Bool == true
  }

  func hasV3Envelope(event: String, transport: String = "cli") -> Bool {
    guard
      let payload = payloads.first(where: { $0["event"] as? String == event }),
      let properties = payload["properties"] as? [String: Any]
    else { return false }
    return properties["telemetry_schema_version"] as? Int == 3
      && properties["os_family"] as? String == "macos"
      && Int(properties["os_major"] as? String ?? "") != nil
      && properties["os"] == nil
      && properties["os_version"] == nil
      && properties["transport"] as? String == transport
      && properties["is_synthetic"] as? Bool == false
      && (properties["process_session_id"] as? String).flatMap(UUID.init(uuidString:)) != nil
  }

  func stringProperty(event: String, key: String) -> String? {
    guard
      let payload = payloads.first(where: { $0["event"] as? String == event }),
      let properties = payload["properties"] as? [String: Any]
    else { return nil }
    return properties[key] as? String
  }

  func boolProperty(event: String, key: String) -> Bool? {
    guard
      let payload = payloads.first(where: { $0["event"] as? String == event }),
      let properties = payload["properties"] as? [String: Any]
    else { return nil }
    return properties[key] as? Bool
  }

  func distinctIds() -> [String] {
    payloads.compactMap { $0["distinct_id"] as? String }
  }
}

private func telemetryTempDirectory() throws -> URL {
  let url = FileManager.default.temporaryDirectory.appendingPathComponent(
    "lume-telemetry-tests-\(UUID().uuidString)", isDirectory: true)
  try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
  return url
}

@Test("installation and release markers are written only after accepted requests")
func installationMarkersRequireAcceptedRequests() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { true }
  )

  await client.recordInstallation(channel: "install_script")

  let events = await recorder.events()
  #expect(
    Swift.Set(events) == Swift.Set([TelemetryEvent.install, TelemetryEvent.releaseInstalled]))
  #expect(
    FileManager.default.fileExists(
      atPath: home.appendingPathComponent(".installation_recorded").path))
  #expect(
    FileManager.default.fileExists(
      atPath: home.appendingPathComponent(".telemetry_releases/\(Lume.Version.current)").path
    ))

  await client.recordInstallation(channel: "install_script")
  #expect(await recorder.count() == 2)
}

@Test("concurrent lifecycle registration is serialized by the filesystem lock")
func concurrentLifecycleRegistrationIsSerialized() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  let poster: TelemetryClient.Poster = { request in await recorder.post(request) }
  let first = TelemetryClient(
    homeDirectory: home, poster: poster, preferenceProvider: { true })
  let second = TelemetryClient(
    homeDirectory: home, poster: poster, preferenceProvider: { true })

  async let firstRegistration: Void = first.recordInstallation(channel: "install_script")
  async let secondRegistration: Void = second.recordInstallation(channel: "install_script")
  _ = await (firstRegistration, secondRegistration)

  #expect(await recorder.count() == 2)
  #expect(
    Swift.Set(await recorder.events())
      == Swift.Set([TelemetryEvent.install, TelemetryEvent.releaseInstalled]))
}

@Test("failed installation requests defer retries without writing success markers")
func failedInstallationRequestsDeferRetries() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  await recorder.setAccepted(false)
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { true }
  )

  await client.recordInstallation(channel: "install_script")

  #expect(await recorder.count() == 2)
  #expect(
    !FileManager.default.fileExists(
      atPath: home.appendingPathComponent(".installation_recorded").path))
  #expect(
    !FileManager.default.fileExists(atPath: home.appendingPathComponent(".telemetry_releases").path)
  )
  #expect(
    FileManager.default.fileExists(
      atPath: home.appendingPathComponent(".telemetry_retry_after").path))

  await recorder.setAccepted(true)
  await client.recordInstallation(channel: "install_script")
  #expect(await recorder.count() == 2)
}

@Test("disabled telemetry creates no ID, request, or marker")
func disabledTelemetryHasNoSideEffects() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { false }
  )

  await client.recordInstallation(channel: "install_script")
  client.record(event: TelemetryEvent.run, properties: ["headless": true])
  await client.flush()

  #expect(await recorder.count() == 0)
  #expect(
    !FileManager.default.fileExists(atPath: home.appendingPathComponent(".telemetry_id").path))
}

@Test("reset deletes identity and markers while preserving preference")
func resetIdentityPreservesPreference() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { true }
  )
  await client.recordInstallation(channel: "install_script")

  try client.resetInstallationId()

  let status = client.status()
  #expect(status.persistedEnabled)
  #expect(!status.installationIdPresent)
  #expect(
    !FileManager.default.fileExists(
      atPath: home.appendingPathComponent(".installation_recorded").path))
  #expect(
    !FileManager.default.fileExists(atPath: home.appendingPathComponent(".telemetry_releases").path)
  )
}

@Test("routine payload drops user-derived string properties")
func routinePayloadIsContentFree() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { true }
  )

  client.record(
    event: TelemetryEvent.pull,
    properties: ["image_name": "private/customer-image", "headless": true])
  #expect(await client.flush())

  #expect(await recorder.hasContentFreePayload(event: TelemetryEvent.pull))

  #expect(await recorder.hasV3Envelope(event: TelemetryEvent.pull))
}

@Test("schema v3 completion events contain only bounded comparison fields")
func completionEventsUseBoundedSchema() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let recorder = TelemetryRequestRecorder()
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { true }
  )

  client.recordOperationCompleted(
    operation: "run",
    transport: .http,
    success: true,
    errorClass: .none,
    elapsed: 0.75,
    guestOS: "windows"
  )
  client.recordMCPToolCompleted(
    toolName: "lume_exec",
    success: false,
    errorClass: .operationError,
    elapsed: 12
  )
  client.recordVMStarted(transport: .mcpStdio, guestOS: "macos")
  client.recordUpdateChecked(
    source: "mcp", outcome: "available", targetVersion: "v1.2.3", cacheHit: true)
  #expect(await client.flush())

  #expect(
    await recorder.hasV3Envelope(event: TelemetryEvent.operationCompleted, transport: "http"))
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.operationCompleted, key: "operation")
      == "run")
  #expect(
    await recorder.boolProperty(event: TelemetryEvent.operationCompleted, key: "success") == true)
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.operationCompleted, key: "duration_bucket")
      == "500ms_1_999ms")
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.operationCompleted, key: "guest_os")
      == "windows")

  #expect(
    await recorder.stringProperty(event: TelemetryEvent.mcpToolCompleted, key: "tool_name")
      == "lume_exec")
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.mcpToolCompleted, key: "operation")
      == "exec")
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.mcpToolCompleted, key: "error_class")
      == "operation_error")
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.mcpToolCompleted, key: "duration_bucket")
      == "gte_10s")

  #expect(await recorder.boolProperty(event: TelemetryEvent.vmStarted, key: "success") == true)
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.vmStarted, key: "guest_os") == "macos")

  #expect(
    await recorder.stringProperty(event: TelemetryEvent.updateChecked, key: "target_version")
      == "1.2.3")
  #expect(
    await recorder.boolProperty(event: TelemetryEvent.updateChecked, key: "cache_hit") == true)
  #expect(
    await recorder.stringProperty(event: TelemetryEvent.updateChecked, key: "transport")
      == "mcp_stdio")
}

@Test("telemetry mappings remain bounded and stable")
func telemetryMappingsAreBounded() {
  let commands = [
    "create": "create", "pull": "pull", "push": "push", "convert": "convert",
    "images": "images", "clone": "clone", "get": "get", "set": "set", "ls": "list",
    "run": "run", "stop": "stop", "ssh": "ssh", "sip": "sip", "ipsw": "ipsw",
    "serve": "serve", "delete": "delete", "prune": "prune", "config": "config",
    "logs": "logs", "check-update": "check_update", "update": "update", "setup": "setup",
    "dump-docs": "dump_docs",
  ]
  for (command, operation) in commands {
    #expect(TelemetryClient.commandOperation(arguments: [command]) == operation)
  }
  #expect(TelemetryClient.commandOperation(arguments: ["private-command"]) == "other")
  let routes = [
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
    "DELETE /lume/config/locations/:name": "locations_remove", "GET /lume/logs": "logs",
    "POST /lume/config/locations/default/:name": "locations_default",
    "POST /lume/vms/push": "push", "GET /lume/host/status": "host_status",
  ]
  for (route, operation) in routes {
    let pieces = route.split(separator: " ", maxSplits: 1).map(String.init)
    #expect(TelemetryClient.httpOperation(method: pieces[0], routePath: pieces[1]) == operation)
  }
  #expect(TelemetryClient.durationBucket(0.099) == "lt_100ms")
  #expect(TelemetryClient.durationBucket(0.1) == "100_499ms")
  #expect(TelemetryClient.provisioningDurationBucket(900) == "15_29min")
  #expect(TelemetryClient.strictReleaseVersion("v1.2.3") == "1.2.3")
  #expect(TelemetryClient.strictReleaseVersion("customer/path") == nil)
}

@Test("invalid persisted installation ID is replaced and never emitted")
func invalidPersistedIdentityIsReplaced() async throws {
  let home = try telemetryTempDirectory()
  defer { try? FileManager.default.removeItem(at: home) }
  let secret = "private-customer-secret"
  try secret.write(
    to: home.appendingPathComponent(".telemetry_id"), atomically: true, encoding: .utf8)
  let recorder = TelemetryRequestRecorder()
  let client = TelemetryClient(
    homeDirectory: home,
    poster: { request in await recorder.post(request) },
    preferenceProvider: { true }
  )

  client.record(event: TelemetryEvent.run, properties: ["headless": true])
  #expect(await client.flush())

  let ids = await recorder.distinctIds()
  #expect(ids.count == 1)
  #expect(ids.first != secret)
  #expect(ids.first.flatMap(UUID.init(uuidString:)) != nil)
}
