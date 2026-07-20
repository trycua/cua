import Foundation
import Testing

@testable import lume

private enum PresenterTestError: Error {
  case showFailed
}

@MainActor
private final class MockDisplayPresenter: VMDisplayPresenter {
  private(set) var showCount = 0
  private(set) var hideCount = 0
  var showError: Error?

  func show(context: VMDisplayContext) async throws {
    showCount += 1
    if let showError { throw showError }
  }

  func hide() {
    hideCount += 1
  }
}

private func makeDisplayTestDirectory() throws -> (URL, VMDirectory, VMDirContext) {
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
  try Data().write(to: directory.dir.file(".initialized").url)

  return (
    root,
    directory,
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

@MainActor
@Test("Every display mode uses the selected presenter and cleans it up once")
func presenterSelectionAndGuestShutdownCleanup() async throws {
  for mode in DisplayMode.allCases {
    let (root, _, context) = try makeDisplayTestDirectory()
    defer { try? FileManager.default.removeItem(at: root) }

    let service = MockVMVirtualizationService()
    let vnc = MockVNCService(vmDirectory: context.dir)
    let presenter = MockDisplayPresenter()
    var selectedMode: DisplayMode?
    let vm = MockVM(
      vmDirContext: context,
      virtualizationServiceFactory: { _ in service },
      vncServiceFactory: { _ in vnc },
      displayPresenterFactory: { mode, _ in
        selectedMode = mode
        return presenter
      }
    )

    let runTask = Task {
      try await vm.run(displayMode: mode, sharedDirectories: [], mount: nil)
    }
    try await waitUntil { presenter.showCount == 1 }
    service.simulateGuestStop()
    try await runTask.value

    #expect(selectedMode == mode)
    #expect(presenter.showCount == 1)
    #expect(presenter.hideCount == 1)
    #expect(vnc.startCallCount == 1)
    #expect(vnc.stopCallCount == 1)
    #expect(service.stopCallCount == 0)
  }
}

@MainActor
@Test("Fatal guest stop releases presenter and VNC resources")
func fatalGuestStopCleanup() async throws {
  let (root, _, context) = try makeDisplayTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let vnc = MockVNCService(vmDirectory: context.dir)
  let presenter = MockDisplayPresenter()
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in service },
    vncServiceFactory: { _ in vnc },
    displayPresenterFactory: { _, _ in presenter }
  )

  let runTask = Task {
    try await vm.run(displayMode: .native, sharedDirectories: [], mount: nil)
  }
  try await waitUntil { presenter.showCount == 1 }
  service.simulateFatalStop()
  await #expect(throws: VMError.self) {
    try await runTask.value
  }

  #expect(service.stopCallCount == 0)
  #expect(presenter.hideCount == 1)
  #expect(vnc.stopCallCount == 1)
}

@MainActor
@Test("Native presenter creation failure stops the VM and releases session resources")
func presenterFailureCleanup() async throws {
  let (root, _, context) = try makeDisplayTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let vnc = MockVNCService(vmDirectory: context.dir)
  let presenter = MockDisplayPresenter()
  presenter.showError = PresenterTestError.showFailed
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in service },
    vncServiceFactory: { _ in vnc },
    displayPresenterFactory: { _, _ in presenter }
  )

  await #expect(throws: PresenterTestError.self) {
    try await vm.run(displayMode: .native, sharedDirectories: [], mount: nil)
  }
  #expect(service.stopCallCount == 1)
  #expect(presenter.hideCount == 1)
  #expect(vnc.stopCallCount == 1)
}

@MainActor
@Test("Cancelling VM run stops the guest and cleans up exactly once")
func cancellationCleanup() async throws {
  let (root, _, context) = try makeDisplayTestDirectory()
  defer { try? FileManager.default.removeItem(at: root) }

  let service = MockVMVirtualizationService()
  let vnc = MockVNCService(vmDirectory: context.dir)
  let presenter = MockDisplayPresenter()
  let vm = MockVM(
    vmDirContext: context,
    virtualizationServiceFactory: { _ in service },
    vncServiceFactory: { _ in vnc },
    displayPresenterFactory: { _, _ in presenter }
  )

  let runTask = Task {
    try await vm.run(displayMode: .none, sharedDirectories: [], mount: nil)
  }
  try await waitUntil { presenter.showCount == 1 }
  runTask.cancel()
  await #expect(throws: CancellationError.self) {
    try await runTask.value
  }

  #expect(service.stopCallCount == 1)
  #expect(presenter.hideCount == 1)
  #expect(vnc.stopCallCount == 1)
}
