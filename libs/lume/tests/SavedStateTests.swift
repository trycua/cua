import Foundation
import Testing

@testable import lume

@MainActor
private final class QuietPresenter: VMDisplayPresenter {
  private(set) var showCount = 0
  func show(context: VMDisplayContext) async throws { showCount += 1 }
  func hide() {}
}

private func makeStateTestDirectory() throws -> (URL, VMDirContext) {
  let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
  let directory = VMDirectory(Path(root.path))
  try Data(repeating: 0, count: 1024).write(to: directory.diskPath.url)
  try Data(repeating: 0, count: 1024).write(to: directory.nvramPath.url)
  var config = try VMConfig(
    os: "mock-os", cpuCount: 1, memorySize: 1024, diskSize: 1024, display: "1024x768")
  config.setMacAddress("00:11:22:33:44:55")
  try directory.saveConfig(config)
  return (
    root,
    VMDirContext(dir: directory, config: config, home: Home(fileManager: .default), storage: nil)
  )
}

@MainActor
private func waitFor(_ condition: @escaping @MainActor () -> Bool) async throws {
  let clock = ContinuousClock()
  let deadline = clock.now.advanced(by: .seconds(2))
  while !condition() {
    guard clock.now < deadline else {
      Issue.record("Timed out waiting for asynchronous VM state")
      return
    }
    try await Task.sleep(for: .milliseconds(10))
  }
}

@MainActor
private func makeVM(_ context: VMDirContext, services: [MockVMVirtualizationService])
  -> (MockVM, QuietPresenter)
{
  var pending = services
  let presenter = QuietPresenter()
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in pending.removeFirst() },
    vncServiceFactory: { MockVNCService(vmDirectory: $0) },
    displayPresenterFactory: { _, _ in presenter }
  )
  return (vm, presenter)
}

@MainActor
@Test("A run resumes the VM's saved state instead of booting it, and consumes the state")
func runResumesSavedState() async throws {
  let (root, context) = try makeStateTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  try Data("state".utf8).write(to: context.dir.statePath.url)

  let service = MockVMVirtualizationService()
  let (vm, presenter) = makeVM(context, services: [service])
  let run = Task { try await vm.run(displayMode: .none, sharedDirectories: [], mount: nil) }
  try await waitFor { presenter.showCount == 1 }

  #expect(service.restoredStates.map(\.path) == [context.dir.statePath.path])
  #expect(service.startCallCount == 0)
  #expect(service.resumeCallCount == 1)
  #expect(!context.dir.hasSavedState())
  service.simulateGuestStop()
  try await run.value
}

@MainActor
@Test("A state that does not restore boots the VM instead and is dropped")
func unrestorableStateBoots() async throws {
  let (root, context) = try makeStateTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  try Data("state".utf8).write(to: context.dir.statePath.url)

  let refusing = MockVMVirtualizationService()
  await refusing.configure(shouldFail: true)
  let fresh = MockVMVirtualizationService()
  let (vm, presenter) = makeVM(context, services: [refusing, fresh])
  let run = Task { try await vm.run(displayMode: .none, sharedDirectories: [], mount: nil) }
  try await waitFor { presenter.showCount == 1 }

  #expect(refusing.restoredStates.isEmpty)
  #expect(fresh.startCallCount == 1)
  #expect(!context.dir.hasSavedState())
  fresh.simulateGuestStop()
  try await run.value
}

@MainActor
@Test("Suspend saves the running VM's state next to its disk and stops it")
func suspendSavesStateAndStops() async throws {
  let (root, context) = try makeStateTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let (vm, presenter) = makeVM(context, services: [service])
  let run = Task { try await vm.run(displayMode: .none, sharedDirectories: [], mount: nil) }
  try await waitFor { presenter.showCount == 1 }

  try await vm.suspend()
  try await run.value
  #expect(service.pauseCallCount == 1)
  #expect(service.savedStates.count == 1)
  #expect(service.stopCallCount == 1)
  #expect(context.dir.hasSavedState())
  // Written under a temporary name and moved into place.
  #expect(service.savedStates.first?.lastPathComponent == "state.vzvmsave.partial")
  #expect(
    !FileManager.default.fileExists(atPath: context.dir.statePath.path + ".partial"))
}

@MainActor
@Test("A failed save keeps the VM running and leaves no state")
func failedSaveKeepsRunning() async throws {
  let (root, context) = try makeStateTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let (vm, presenter) = makeVM(context, services: [service])
  let run = Task { try await vm.run(displayMode: .none, sharedDirectories: [], mount: nil) }
  try await waitFor { presenter.showCount == 1 }

  await service.configure(shouldFail: true)
  await #expect(throws: (any Error).self) { try await vm.suspend() }
  #expect(!context.dir.hasSavedState())
  #expect(service.stopCallCount == 0)
  await service.configure(shouldFail: false)
  service.simulateGuestStop()
  try await run.value
}

@MainActor
@Test("A stop that arrives while the state is being restored waits for it, then stops the VM")
func stopDuringRestoreWaits() async throws {
  let (root, context) = try makeStateTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }
  try Data("state".utf8).write(to: context.dir.statePath.url)

  let service = MockVMVirtualizationService()
  service.restoreDelayNanos = 300_000_000
  let (vm, _) = makeVM(context, services: [service])
  let run = Task { try await vm.run(displayMode: .none, sharedDirectories: [], mount: nil) }
  try await Task.sleep(for: .milliseconds(50))
  try await vm.stop()
  try await run.value

  #expect(service.restoredStates.count == 1)
  #expect(service.stopCallCount == 1)
  #expect(service.state == .stopped)
}
