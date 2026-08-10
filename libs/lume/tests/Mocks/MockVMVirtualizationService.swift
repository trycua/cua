import Foundation
import Virtualization
@testable import lume

private enum MockVMTerminalEvent: Sendable {
    case stopped
    case failed(String)
}

@MainActor
final class MockVMVirtualizationService: VMVirtualizationService {
    private(set) var currentState: VZVirtualMachine.State = .stopped
    private(set) var startCallCount = 0
    private(set) var stopCallCount = 0
    private(set) var pauseCallCount = 0
    private(set) var resumeCallCount = 0
    private(set) var updateSharedDirectoriesCallCount = 0
    private(set) var sharedDirectories: [SharedDirectory] = []

    private let terminalEvents: AsyncStream<MockVMTerminalEvent>
    private let terminalEventContinuation: AsyncStream<MockVMTerminalEvent>.Continuation
    private var didTerminate = false
    private var shouldFailNextOperation = false
    private var operationError: Error = VMError.internalError("Mock operation failed")

    init() {
        var continuation: AsyncStream<MockVMTerminalEvent>.Continuation!
        terminalEvents = AsyncStream { continuation = $0 }
        terminalEventContinuation = continuation
    }

    var state: VZVirtualMachine.State { currentState }
    var displayVirtualMachine: VZVirtualMachine? { nil }

    nonisolated func configure(
        shouldFail: Bool,
        error: Error = VMError.internalError("Mock operation failed")
    ) async {
        await setConfiguration(shouldFail: shouldFail, error: error)
    }

    private func setConfiguration(shouldFail: Bool, error: Error) {
        shouldFailNextOperation = shouldFail
        operationError = error
    }

    func start() async throws {
        startCallCount += 1
        if shouldFailNextOperation { throw operationError }
        currentState = .running
    }

    func stop() async throws {
        stopCallCount += 1
        if shouldFailNextOperation { throw operationError }
        currentState = .stopped
        finish(with: .stopped)
    }

    func updateSharedDirectories(_ sharedDirectories: [SharedDirectory]) async throws {
        updateSharedDirectoriesCallCount += 1
        if shouldFailNextOperation { throw operationError }
        self.sharedDirectories = sharedDirectories
    }

    func waitForStop() async throws {
        for await event in terminalEvents {
            switch event {
            case .stopped:
                return
            case .failed(let message):
                throw VMError.internalError(message)
            }
        }
        throw CancellationError()
    }

    func simulateGuestStop() {
        currentState = .stopped
        finish(with: .stopped)
    }

    func simulateFatalStop(error: Error = VMError.internalError("Mock VM stopped fatally")) {
        currentState = .stopped
        finish(with: .failed(error.localizedDescription))
    }

    private func finish(with event: MockVMTerminalEvent) {
        guard !didTerminate else { return }
        didTerminate = true
        terminalEventContinuation.yield(event)
        terminalEventContinuation.finish()
    }

    func pause() async throws {
        pauseCallCount += 1
        if shouldFailNextOperation { throw operationError }
        currentState = .paused
    }

    func resume() async throws {
        resumeCallCount += 1
        if shouldFailNextOperation { throw operationError }
        currentState = .running
    }
}
