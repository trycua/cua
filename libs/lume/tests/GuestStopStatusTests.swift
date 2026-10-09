import Foundation
import Testing

@testable import lume

@MainActor
private final class CapturingVMFactory: VMFactory {
  private(set) var vms: [VM] = []
  private(set) var services: [MockVMVirtualizationService] = []

  func createVM(vmDirContext: VMDirContext, imageLoader: ImageLoader?) throws -> VM {
    let vm = MockVM(
      vmDirContext: vmDirContext,
      virtualizationServiceFactory: { [unowned self] _ in
        let service = MockVMVirtualizationService()
        self.services.append(service)
        return service
      },
      vncServiceFactory: { MockVNCService(vmDirectory: $0) }
    )
    vms.append(vm)
    return vm
  }
}

/// A controller and a throwaway storage directory holding one stopped Linux
/// VM. The tests address the VM by that directory path, so they never read or
/// change the user's Lume configuration.
@MainActor
private func withGuestStopFixture(
  _ body: (LumeController, CapturingVMFactory, String, String) async throws -> Void
) async throws {
  let storage = FileManager.default.temporaryDirectory
    .appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: storage, withIntermediateDirectories: true)
  defer { try? FileManager.default.removeItem(at: storage) }

  let home = Home(fileManager: .default)
  let factory = CapturingVMFactory()
  let controller = LumeController(home: home, vmFactory: factory)

  let name = "guest-stop-\(UUID().uuidString.prefix(8).lowercased())"
  let vmDir = try home.getVMDirectoryFromPath(name, storagePath: storage.path)
  try FileManager.default.createDirectory(at: vmDir.dir.url, withIntermediateDirectories: true)
  try Data(repeating: 0, count: 1024).write(to: vmDir.diskPath.url)
  try Data(repeating: 0, count: 1024).write(to: vmDir.nvramPath.url)
  var config = try VMConfig(
    os: "linux", cpuCount: 1, memorySize: 1024, diskSize: 1024, display: "1024x768")
  config.setMacAddress("00:11:22:33:44:57")
  try vmDir.saveConfig(config)

  defer { SharedVM.shared.removeVM(name: name) }
  try await body(controller, factory, name, storage.path)
}

@MainActor
@Test("A VM whose guest stops by itself reports stopped and deletes on the first request (#4704)")
func guestInitiatedStopReportsStopped() async throws {
  try await withGuestStopFixture { controller, factory, name, storage in
    // `lume serve` runs the VM the same way: runVM in a task of its own.
    let run = Task { @MainActor in
      try await controller.runVM(
        name: name, noDisplay: true, storage: storage, vncPolicy: .disabled)
    }
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: .seconds(5))
    while factory.services.first?.state != .running, clock.now < deadline {
      try await Task.sleep(for: .milliseconds(10))
    }
    let service = try #require(factory.services.first)

    // Control: a running guest is reported running.
    #expect(try controller.getDetails(name: name, storage: storage).status == "running")

    // The guest powers itself off; nothing calls stop.
    service.simulateGuestStop()
    try await run.value

    #expect(try controller.getDetails(name: name, storage: storage).status == "stopped")
    #expect(try controller.list(storage: storage).first { $0.name == name }?.status == "stopped")
    #expect(SharedVM.shared.getVM(name: name) == nil)

    try await controller.delete(name: name, storage: storage)
    #expect(throws: (any Error).self) { try controller.getDetails(name: name, storage: storage) }
  }
}

@MainActor
@Test("A cached VM whose guest is not running is not reported running (#4704)")
func staleCachedVMIsNotReportedRunning() async throws {
  try await withGuestStopFixture { controller, factory, name, storage in
    // A run that ended without its cache entry being cleared.
    let run = Task { @MainActor in
      try await controller.runVM(
        name: name, noDisplay: true, storage: storage, vncPolicy: .disabled)
    }
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: .seconds(5))
    while factory.services.first?.state != .running, clock.now < deadline {
      try await Task.sleep(for: .milliseconds(10))
    }
    let service = try #require(factory.services.first)
    let vm = try #require(factory.vms.first)
    service.simulateGuestStop()
    try await run.value
    SharedVM.shared.setVM(name: name, vm: vm)

    #expect(try controller.getDetails(name: name, storage: storage).status == "stopped")
    #expect(SharedVM.shared.getVM(name: name) == nil)
  }
}
