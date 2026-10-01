import ArgumentParser
import Foundation
import Testing

@testable import lume

// MARK: - Helpers

private enum VNCPolicyTestError: Error {
  case presenterFailed
}

@MainActor
private final class RecordingPresenter: VMDisplayPresenter {
  private(set) var showCount = 0
  private(set) var hideCount = 0
  private(set) var lastContext: VMDisplayContext?
  var showError: Error?

  func show(context: VMDisplayContext) async throws {
    showCount += 1
    lastContext = context
    if let showError { throw showError }
  }

  func hide() {
    hideCount += 1
  }
}

private struct StubRunLockProbe: RunLockProbe {
  let holdersByPath: [String: [pid_t]]
  var inconclusivePaths: Swift.Set<String> = []

  func lockHolderPIDs(ofFileAt path: String) -> [pid_t]? {
    if inconclusivePaths.contains(path) { return nil }
    return holdersByPath[path] ?? []
  }
}

private func makeVMTestDirectory() throws -> (URL, VMDirContext) {
  let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
  let directory = VMDirectory(Path(root.path))
  try Data(repeating: 0, count: 1024).write(to: directory.diskPath.url)
  try Data(repeating: 0, count: 1024).write(to: directory.nvramPath.url)

  var config = try VMConfig(
    os: "mock-os",
    cpuCount: 1,
    memorySize: 1024,
    diskSize: 1024,
    display: "1024x768"
  )
  config.setMacAddress("00:11:22:33:44:55")
  try directory.saveConfig(config)

  return (
    root,
    VMDirContext(
      dir: directory,
      config: config,
      home: Home(fileManager: .default),
      storage: nil
    )
  )
}

@MainActor
private func waitUntil(
  timeout: Duration = .seconds(2),
  _ condition: @escaping @MainActor () -> Bool
) async throws {
  let clock = ContinuousClock()
  let deadline = clock.now.advanced(by: timeout)
  while !condition() {
    guard clock.now < deadline else {
      Issue.record("Timed out waiting for asynchronous VM state")
      return
    }
    try await Task.sleep(for: .milliseconds(10))
  }
}

/// Path of the VirtioFS file that publishes VNC credentials to the guest.
private func vncEnvURL(forVMNamed name: String) -> URL {
  FileManager.default.temporaryDirectory
    .appendingPathComponent("lume-config-\(name)")
    .appendingPathComponent("vnc.env")
}

// MARK: - CLI surface

@Test("run defaults to an enabled VNC server")
func vncPolicyDefaultsToEnabled() throws {
  let command = try Run.parse(["test-vm"])
  #expect(command.vnc == .enabled)
  try command.validate()
}

@Test("--vnc accepts both policies and rejects anything else")
func vncPolicyParsing() throws {
  #expect(try Run.parse(["test-vm", "--vnc", "enabled"]).vnc == .enabled)
  #expect(try Run.parse(["test-vm", "--vnc", "disabled"]).vnc == .disabled)
  #expect(throws: Error.self) {
    _ = try Run.parse(["test-vm", "--vnc", "off"])
  }
}

@Test("--vnc disabled is accepted with the non-VNC display modes")
func vncDisabledAcceptedForNonVNCDisplays() throws {
  try Run.parse(["test-vm", "--vnc", "disabled", "--display", "none"]).validate()
  try Run.parse(["test-vm", "--vnc", "disabled", "--display", "native"]).validate()
  try Run.parse(["test-vm", "--vnc", "disabled"]).validate()
  // --no-display wins over --display, so the resolved mode is not vnc.
  try Run.parse(["test-vm", "--vnc", "disabled", "--display", "vnc", "--no-display"]).validate()
}

/// Parses and validates a `run` invocation, reporting whether it was rejected.
/// ArgumentParser may surface a validation failure from either step, so both
/// are treated as one rejection.
private func runIsRejected(_ arguments: [String]) -> Bool {
  do {
    try Run.parse(arguments).validate()
    return false
  } catch {
    return true
  }
}

@Test("--vnc disabled is rejected with options that need a VNC server")
func vncDisabledRejectsConflictingOptions() throws {
  #expect(runIsRejected(["test-vm", "--vnc", "disabled", "--display", "vnc"]))
  #expect(runIsRejected(["test-vm", "--vnc", "disabled", "--vnc-port", "5900"]))
  #expect(runIsRejected(["test-vm", "--vnc", "disabled", "--vnc-password", "hunter2"]))
  // The same rejection must reach the root command parser.
  #expect(throws: Error.self) {
    _ = try Lume.parseAsRoot(["run", "test-vm", "--vnc", "disabled", "--display", "vnc"])
  }
}

@Test("enabled runs keep accepting every VNC option")
func vncEnabledKeepsExistingCombinations() throws {
  try Run.parse(["test-vm", "--display", "vnc", "--vnc-port", "5900"]).validate()
  try Run.parse(["test-vm", "--vnc", "enabled", "--vnc-password", "hunter2"]).validate()
}

@Test("Detached child arguments preserve the VNC policy")
func detachedChildArgumentsPreserveVNCPolicy() {
  let separated = DetachedVMRunner.childArguments(from: [
    "/path/to/lume", "run", "test-vm", "--detach", "--vnc", "disabled",
  ])
  #expect(separated == ["run", "test-vm", "--vnc", "disabled", "--display", "none"])

  let joined = DetachedVMRunner.childArguments(from: [
    "/path/to/lume", "run", "test-vm", "--detach", "--vnc=disabled", "--display", "native",
  ])
  #expect(joined == ["run", "test-vm", "--vnc=disabled", "--display", "native"])
}

// MARK: - Session marker compatibility

@Test("Legacy session markers still decode as VNC-enabled")
func legacySessionDecoding() throws {
  let legacy = Data(
    #"{"url":"vnc://:pass@127.0.0.1:62295"}"#.utf8)
  let session = try JSONDecoder().decode(VNCSession.self, from: legacy)

  #expect(session.url == "vnc://:pass@127.0.0.1:62295")
  #expect(session.isVNCEnabled)
  #expect(session.pid == nil)
  #expect(session.startedAt == nil)

  let withDirs = Data(
    #"{"url":"vnc://:pass@127.0.0.1:62295","sharedDirectories":[{"hostPath":"/tmp","tag":"com.apple.virtio-fs.automount","readOnly":false}]}"#
      .utf8)
  let decoded = try JSONDecoder().decode(VNCSession.self, from: withDirs)
  #expect(decoded.sharedDirectories?.count == 1)
  #expect(decoded.isVNCEnabled)
}

@Test("VNC-disabled session markers decode with their liveness fields")
func noVNCSessionDecoding() throws {
  let data = Data(#"{"vncEnabled":false,"pid":4242,"startedAt":1750000000.0}"#.utf8)
  let session = try JSONDecoder().decode(VNCSession.self, from: data)

  #expect(session.url == nil)
  #expect(!session.isVNCEnabled)
  #expect(session.pid == 4242)
  #expect(session.startedAt == 1_750_000_000.0)
}

@Test("An enabled session marker still encodes only the legacy fields")
func enabledSessionEncodingStaysLegacy() throws {
  let data = try JSONEncoder().encode(VNCSession(url: "vnc://:pass@127.0.0.1:62295"))
  let decoded = try JSONSerialization.jsonObject(with: data)
  let object = try #require(decoded as? [String: Any])

  #expect(Swift.Set(object.keys) == Swift.Set(["url"]))
}

@Test("A VNC-disabled marker round-trips through the VM directory")
func noVNCSessionRoundTrip() throws {
  let (root, context) = try makeVMTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  try context.dir.saveSession(
    VNCSession.vncDisabled(pid: 4242, startedAt: 1_750_000_000.0))
  let loaded = try context.dir.loadSession()

  #expect(loaded.url == nil)
  #expect(!loaded.isVNCEnabled)
  #expect(loaded.pid == 4242)
  #expect(loaded.startedAt == 1_750_000_000.0)
}

// MARK: - VM lifecycle

@MainActor
@Test("A VNC-disabled run never starts VNC and clears its marker on shutdown")
func disabledRunNeverStartsVNC() async throws {
  let (root, context) = try makeVMTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  let envURL = vncEnvURL(forVMNamed: context.name)

  let service = MockVMVirtualizationService()
  let vnc = MockVNCService(vmDirectory: context.dir)
  let presenter = RecordingPresenter()
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in service },
    vncServiceFactory: { _ in vnc },
    displayPresenterFactory: { _, _ in presenter }
  )

  let runTask = Task {
    try await vm.run(
      displayMode: .none, sharedDirectories: [], mount: nil, vncPolicy: .disabled)
  }
  try await waitUntil { presenter.showCount == 1 }

  #expect(vnc.startCallCount == 0)
  #expect(vm.getVNCUrl() == nil)
  let displayContext = try #require(presenter.lastContext)
  #expect(displayContext.vncURL == nil)
  #expect(!FileManager.default.fileExists(atPath: envURL.path))

  let session = try context.dir.loadSession()
  #expect(!session.isVNCEnabled)
  #expect(session.url == nil)
  #expect(session.pid == getpid())
  #expect((session.startedAt ?? 0) > 0)

  service.simulateGuestStop()
  try await runTask.value

  #expect(vnc.startCallCount == 0)
  #expect(!context.dir.sessionsPath.exists())
  #expect(!FileManager.default.fileExists(atPath: envURL.path))
}

@MainActor
@Test("Every display mode keeps starting VNC when the policy is enabled")
func enabledRunKeepsStartingVNCForEveryDisplayMode() async throws {
  for mode in DisplayMode.allCases {
    let (root, context) = try makeVMTestDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let service = MockVMVirtualizationService()
    let vnc = MockVNCService(vmDirectory: context.dir)
    let presenter = RecordingPresenter()
    let vm = MockVM(
      vmDirContext: context,
      virtualizationServiceFactory: { _ in service },
      vncServiceFactory: { _ in vnc },
      displayPresenterFactory: { _, _ in presenter }
    )

    let runTask = Task {
      try await vm.run(displayMode: mode, sharedDirectories: [], mount: nil)
    }
    try await waitUntil { presenter.showCount == 1 }

    #expect(vnc.startCallCount == 1)
    let session = try context.dir.loadSession()
    #expect(session.isVNCEnabled)
    #expect(session.url != nil)
    #expect(session.pid == nil)
    let displayContext = try #require(presenter.lastContext)
    #expect(displayContext.vncURL != nil)

    service.simulateGuestStop()
    try await runTask.value
  }
}

@MainActor
@Test("A failed start on a VNC-disabled run removes the session marker")
func disabledRunClearsMarkerOnStartFailure() async throws {
  let (root, context) = try makeVMTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let vnc = MockVNCService(vmDirectory: context.dir)
  let presenter = RecordingPresenter()
  presenter.showError = VNCPolicyTestError.presenterFailed
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in service },
    vncServiceFactory: { _ in vnc },
    displayPresenterFactory: { _, _ in presenter }
  )

  await #expect(throws: VNCPolicyTestError.self) {
    try await vm.run(
      displayMode: .none, sharedDirectories: [], mount: nil, vncPolicy: .disabled)
  }

  #expect(service.stopCallCount == 1)
  #expect(vnc.startCallCount == 0)
  #expect(!context.dir.sessionsPath.exists())
}

@MainActor
@Test("VM.run refuses a VNC-disabled run that needs a VNC server")
func vmRunRefusesConflictingVNCPolicy() async throws {
  let (root, context) = try makeVMTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let vnc = MockVNCService(vmDirectory: context.dir)
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in service },
    vncServiceFactory: { _ in vnc },
    displayPresenterFactory: { _, _ in RecordingPresenter() }
  )

  await #expect(throws: VMError.self) {
    try await vm.run(
      displayMode: .vnc, sharedDirectories: [], mount: nil, vncPolicy: .disabled)
  }
  await #expect(throws: VMError.self) {
    try await vm.run(
      displayMode: .none, sharedDirectories: [], mount: nil, vncPort: 5900,
      vncPolicy: .disabled)
  }
  #expect(vnc.startCallCount == 0)
  #expect(!context.dir.sessionsPath.exists())
}

// MARK: - Run lock probe

@Test("lsof -t output parses into holder PIDs")
func runLockProbeParsesHolders() {
  #expect(LsofRunLockProbe.parsePIDs(from: Data("123\n456\n".utf8)) == [123, 456])
  #expect(LsofRunLockProbe.parsePIDs(from: Data("".utf8)) == [])
  #expect(LsofRunLockProbe.parsePIDs(from: Data("f3\ntext\n".utf8)) == nil)
}

@Test("A recorded PID is trusted only while it holds the run lock")
func runLockProbeLiveHolderRule() {
  let probe = StubRunLockProbe(holdersByPath: ["/vm/config.json": [4242]])

  #expect(probe.isLiveLockHolder(pid: 4242, ofFileAt: "/vm/config.json") == true)
  #expect(probe.isLiveLockHolder(pid: 999, ofFileAt: "/vm/config.json") == false)
  #expect(probe.isLiveLockHolder(pid: 0, ofFileAt: "/vm/config.json") == false)
  #expect(probe.isLiveLockHolder(pid: 4242, ofFileAt: "/other/config.json") == false)
  #expect(probe.lockOwnerPID(ofFileAt: "/vm/config.json") == 4242)
  #expect(probe.lockOwnerPID(ofFileAt: "/other/config.json") == nil)
}

// MARK: - Cross-process status

/// Creates an isolated storage location holding one stopped VM.
///
/// Addressing the location by absolute path keeps these tests off the shared
/// settings file, so they never race other tests over `XDG_CONFIG_HOME`.
private func makeStoppedVMLocation(named name: String) throws -> (URL, VMDirectory) {
  let storage = FileManager.default.temporaryDirectory
    .appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: storage, withIntermediateDirectories: true)

  let vmDir = VMDirectory(Path(storage.path).directory(name))
  try FileManager.default.createDirectory(
    at: vmDir.dir.url, withIntermediateDirectories: true)
  try Data(repeating: 0, count: 1024).write(to: vmDir.diskPath.url)
  try Data(repeating: 0, count: 1024).write(to: vmDir.nvramPath.url)
  try vmDir.saveConfig(
    VMConfig(
      os: "macOS",
      cpuCount: 1,
      memorySize: 1024,
      diskSize: 1024,
      display: "1024x768"
    ))
  return (storage, vmDir)
}

@MainActor
@Test("A VNC-disabled VM reports running while its PID holds the run lock")
func noVNCStatusIsRunningForLiveLockHolder() throws {
  let (storage, vmDir) = try makeStoppedVMLocation(named: "no-vnc-live")
  defer { try? FileManager.default.removeItem(at: storage) }
  try vmDir.saveSession(
    VNCSession.vncDisabled(pid: 4242, startedAt: Date().timeIntervalSince1970))

  let controller = LumeController(
    runLockProbe: StubRunLockProbe(holdersByPath: [vmDir.configPath.path: [4242]]))

  let details = try controller.getDetails(name: "no-vnc-live", storage: storage.path)
  #expect(details.status == "running")
  #expect(details.vncUrl == nil)
  #expect(vmDir.sessionsPath.exists())

  let all = try controller.list(storage: storage.path)
  let listed = try #require(all.first)
  #expect(listed.name == "no-vnc-live")
  #expect(listed.status == "running")
  #expect(listed.vncUrl == nil)
}

@MainActor
@Test("A VNC-disabled marker whose PID no longer holds the lock is stale")
func noVNCStatusIsStoppedForDeadLockHolder() throws {
  let (storage, vmDir) = try makeStoppedVMLocation(named: "no-vnc-stale")
  defer { try? FileManager.default.removeItem(at: storage) }
  try vmDir.saveSession(
    VNCSession.vncDisabled(pid: 4242, startedAt: Date().timeIntervalSince1970))

  let controller = LumeController(runLockProbe: StubRunLockProbe(holdersByPath: [:]))

  let details = try controller.getDetails(name: "no-vnc-stale", storage: storage.path)
  #expect(details.status == "stopped")
  #expect(details.vncUrl == nil)
  #expect(!vmDir.sessionsPath.exists())
}

@MainActor
@Test("A recycled PID does not make a VNC-disabled VM look running")
func noVNCStatusRejectsRecycledPID() throws {
  let (storage, vmDir) = try makeStoppedVMLocation(named: "no-vnc-recycled")
  defer { try? FileManager.default.removeItem(at: storage) }
  try vmDir.saveSession(
    VNCSession.vncDisabled(pid: 4242, startedAt: Date().timeIntervalSince1970))

  let controller = LumeController(
    runLockProbe: StubRunLockProbe(holdersByPath: [vmDir.configPath.path: [777]]))

  let details = try controller.getDetails(name: "no-vnc-recycled", storage: storage.path)
  #expect(details.status == "stopped")
  #expect(!vmDir.sessionsPath.exists())
}

@MainActor
@Test("A VNC-disabled marker without a PID is never trusted")
func noVNCStatusRejectsMarkerWithoutPID() throws {
  let (storage, vmDir) = try makeStoppedVMLocation(named: "no-vnc-pidless")
  defer { try? FileManager.default.removeItem(at: storage) }
  try vmDir.saveSession(VNCSession(url: nil, vncEnabled: false))

  let controller = LumeController(
    runLockProbe: StubRunLockProbe(holdersByPath: [vmDir.configPath.path: [4242]]))

  let details = try controller.getDetails(name: "no-vnc-pidless", storage: storage.path)
  #expect(details.status == "stopped")
  #expect(!vmDir.sessionsPath.exists())
}

@MainActor
@Test("An inconclusive lock probe preserves a VNC-disabled marker")
func noVNCStatusPreservesMarkerWhenProbeIsInconclusive() throws {
  let (storage, vmDir) = try makeStoppedVMLocation(named: "no-vnc-inconclusive")
  defer { try? FileManager.default.removeItem(at: storage) }
  try vmDir.saveSession(
    VNCSession.vncDisabled(pid: 4242, startedAt: Date().timeIntervalSince1970))

  let controller = LumeController(
    runLockProbe: StubRunLockProbe(
      holdersByPath: [:], inconclusivePaths: [vmDir.configPath.path]))

  let details = try controller.getDetails(name: "no-vnc-inconclusive", storage: storage.path)
  #expect(details.status == "stopped")
  #expect(details.vncUrl == nil)
  #expect(vmDir.sessionsPath.exists())
}

@MainActor
@Test("A stale VNC-backed marker is still detected by its port, not by the run lock")
func vncEnabledStatusStillUsesPortProbe() throws {
  let (storage, vmDir) = try makeStoppedVMLocation(named: "vnc-stale")
  defer { try? FileManager.default.removeItem(at: storage) }
  // Port 1 is never bound by a VM, so this marker must read as stale even
  // though a process does hold the config file open.
  try vmDir.saveSession(VNCSession(url: "vnc://:pass@127.0.0.1:1"))

  let controller = LumeController(
    runLockProbe: StubRunLockProbe(holdersByPath: [vmDir.configPath.path: [getpid()]]))

  let details = try controller.getDetails(name: "vnc-stale", storage: storage.path)
  #expect(details.status == "stopped")
  #expect(details.vncUrl == nil)
  #expect(!vmDir.sessionsPath.exists())
}

// MARK: - HTTP / MCP request surfaces

private func decodeRunRequest(_ json: String) throws -> RunVMRequest {
  try JSONDecoder().decode(RunVMRequest.self, from: Data(json.utf8))
}

@Test("The run request body defaults to an enabled VNC server")
func runRequestDefaultsToEnabled() throws {
  #expect(try decodeRunRequest("{}").parseVNCPolicy() == .enabled)
  #expect(try decodeRunRequest(#"{"noDisplay":true}"#).parseVNCPolicy() == .enabled)
}

@Test("The run request body carries an explicit VNC policy")
func runRequestParsesVNCPolicy() throws {
  #expect(try decodeRunRequest(#"{"vnc":"enabled"}"#).parseVNCPolicy() == .enabled)
  #expect(try decodeRunRequest(#"{"vnc":"disabled"}"#).parseVNCPolicy() == .disabled)
  #expect(throws: ValidationError.self) {
    _ = try decodeRunRequest(#"{"vnc":"off"}"#).parseVNCPolicy()
  }
}

@Test("HTTP-style VNC-disabled runs reject the default VNC display before dispatch")
func httpRunVNCPolicyConflictIsSynchronous() throws {
  let request = try decodeRunRequest(#"{"vnc":"disabled"}"#)
  #expect(throws: VMError.self) {
    _ = try request.validatedVNCPolicy(noDisplayDefault: false)
  }
  #expect(
    try decodeRunRequest(#"{"vnc":"disabled","noDisplay":true}"#)
      .validatedVNCPolicy(noDisplayDefault: false) == .disabled)
}

@Test("The conflicting-option rule is shared by every run surface")
func conflictingOptionRule() {
  #expect(
    VNCPolicy.conflictingOption(
      policy: .enabled, displayMode: .vnc, vncPort: 5900, vncPassword: "x") == nil)
  #expect(
    VNCPolicy.conflictingOption(
      policy: .disabled, displayMode: .vnc, vncPort: 0, vncPassword: nil) == "--display vnc")
  #expect(
    VNCPolicy.conflictingOption(
      policy: .disabled, displayMode: .none, vncPort: 5900, vncPassword: nil) == "--vnc-port")
  #expect(
    VNCPolicy.conflictingOption(
      policy: .disabled, displayMode: .native, vncPort: 0, vncPassword: "x") == "--vnc-password")
  #expect(
    VNCPolicy.conflictingOption(
      policy: .disabled, displayMode: .none, vncPort: 0, vncPassword: nil) == nil)
  #expect(
    VNCPolicy.conflictingOption(
      policy: .disabled, displayMode: .native, vncPort: 0, vncPassword: nil) == nil)
}
