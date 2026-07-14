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

  func hasV2Envelope(event: String) -> Bool {
    guard
      let payload = payloads.first(where: { $0["event"] as? String == event }),
      let properties = payload["properties"] as? [String: Any]
    else { return false }
    return properties["telemetry_schema_version"] as? Int == 2
      && properties["os_family"] as? String == "macos"
      && Int(properties["os_major"] as? String ?? "") != nil
      && properties["os"] == nil
      && properties["os_version"] == nil
      && properties["transport"] as? String == "cli"
      && (properties["process_session_id"] as? String).flatMap(UUID.init(uuidString:)) != nil
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

@Test("failed installation requests leave markers absent for retry")
func failedInstallationRequestsDoNotWriteMarkers() async throws {
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

  #expect(await recorder.hasV2Envelope(event: TelemetryEvent.pull))
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
