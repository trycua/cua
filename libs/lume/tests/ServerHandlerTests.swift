import Foundation
import Testing

@testable import lume

private func details(_ name: String, os: String, status: String = "running") -> VMDetails {
  VMDetails(
    name: name,
    os: os,
    cpuCount: 1,
    memorySize: 1024,
    diskSize: DiskSize(allocated: 0, total: 1024),
    display: "1024x768",
    status: status,
    vncUrl: nil,
    ipAddress: nil,
    locationName: "home"
  )
}

@MainActor
@Test("Starting a macOS guest is refused when the host runs the most macOS guests")
func runVMRefusesMacOSGuestAtLimit() {
  let running = [details("mac-a", os: "macOS"), details("mac-b", os: "macOS")]
  let count = Server.runningMacOSGuestCount(running, excluding: "mac-c")
  #expect(count == Server.maxRunningMacOSGuests)
  #expect(
    Server.macOSGuestCapacityError(targetOS: "macOS", runningMacOSGuests: count)
      == "VM start rejected: host limit of 2 running macOS guests reached")
  #expect(Server.macOSGuestCapacityError(targetOS: "macOS", runningMacOSGuests: 1) == nil)
}

@MainActor
@Test("Linux guests are never refused and do not count toward the macOS guest limit")
func runVMIgnoresLinuxGuestsForTheLimit() {
  let running = [
    details("linux-a", os: "linux"),
    details("linux-b", os: "linux"),
    details("linux-c", os: "linux"),
    details("mac-a", os: "macOS"),
    details("mac-stopped", os: "macOS", status: "stopped"),
  ]
  let count = Server.runningMacOSGuestCount(running, excluding: "mac-b")
  #expect(count == 1)
  #expect(Server.macOSGuestCapacityError(targetOS: "macOS", runningMacOSGuests: count) == nil)
  #expect(
    Server.macOSGuestCapacityError(
      targetOS: "linux", runningMacOSGuests: Server.maxRunningMacOSGuests) == nil)
  // A VM is not counted against its own start.
  #expect(Server.runningMacOSGuestCount(running, excluding: "mac-a") == 0)
}

@MainActor
private final class GuestStopVMFactory: VMFactory {
  private(set) var services: [MockVMVirtualizationService] = []

  func createVM(vmDirContext: VMDirContext, imageLoader: ImageLoader?) throws -> VM {
    MockVM(
      vmDirContext: vmDirContext,
      virtualizationServiceFactory: { [unowned self] _ in
        let service = MockVMVirtualizationService()
        self.services.append(service)
        return service
      },
      vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )
  }
}

@MainActor
@Test("A macOS guest that stopped by itself is not counted as running")
func guestStoppedVMIsNotCountedAsRunning() async throws {
  let tempConfigDir = FileManager.default.temporaryDirectory
    .appendingPathComponent(UUID().uuidString)
  let tempHomeDir = FileManager.default.temporaryDirectory
    .appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: tempConfigDir, withIntermediateDirectories: true)
  try FileManager.default.createDirectory(at: tempHomeDir, withIntermediateDirectories: true)
  defer {
    try? FileManager.default.removeItem(at: tempConfigDir)
    try? FileManager.default.removeItem(at: tempHomeDir)
  }

  let previousXDGConfigHome = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"]
  setenv("XDG_CONFIG_HOME", tempConfigDir.path, 1)
  defer {
    if let previousXDGConfigHome {
      setenv("XDG_CONFIG_HOME", previousXDGConfigHome, 1)
    } else {
      unsetenv("XDG_CONFIG_HOME")
    }
  }

  let settingsManager = SettingsManager(fileManager: .default)
  try settingsManager.setHomeDirectory(path: tempHomeDir.path)
  let home = Home(settingsManager: settingsManager, fileManager: .default)
  let factory = GuestStopVMFactory()
  let controller = LumeController(home: home, vmFactory: factory)

  let name = "guest-stop-\(UUID().uuidString.prefix(8))"
  let vmDir = try home.getVMDirectory(name)
  try FileManager.default.createDirectory(at: vmDir.dir.url, withIntermediateDirectories: true)
  try Data(repeating: 0, count: 1024).write(to: vmDir.diskPath.url)
  try Data(repeating: 0, count: 1024).write(to: vmDir.nvramPath.url)
  var config = try VMConfig(
    os: "macOS", cpuCount: 1, memorySize: 1024, diskSize: 1024, display: "1024x768")
  config.setMacAddress("00:11:22:33:44:56")
  try vmDir.saveConfig(config)

  let run = Task { @MainActor in
    try await controller.runVM(name: name, noDisplay: true, vncPolicy: .disabled)
  }
  let clock = ContinuousClock()
  let deadline = clock.now.advanced(by: .seconds(5))
  while factory.services.first?.state != .running, clock.now < deadline {
    try await Task.sleep(for: .milliseconds(10))
  }
  let service = try #require(factory.services.first)
  #expect(service.state == .running)
  #expect(Server.runningMacOSGuestCount(try controller.runningVMs()) == 1)

  // The guest powers itself off; nothing calls stop.
  service.simulateGuestStop()
  try await run.value

  #expect(try controller.runningVMs().isEmpty)
  #expect(SharedVM.shared.getVM(name: name) == nil)
}
