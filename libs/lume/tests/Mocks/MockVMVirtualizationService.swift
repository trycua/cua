import Foundation
import Virtualization
@testable import lume

@MainActor
final class MockVMVirtualizationService: VMVirtualizationService {
    private(set) var currentState: VZVirtualMachine.State = .stopped
    private(set) var startCallCount = 0
    private(set) var stopCallCount = 0
    private(set) var pauseCallCount = 0
    private(set) var resumeCallCount = 0
    
    var state: VZVirtualMachine.State {
        currentState
    }
    
    private var _shouldFailNextOperation = false
    private var _operationError: Error = VMError.internalError("Mock operation failed")
    
    nonisolated func configure(shouldFail: Bool, error: Error = VMError.internalError("Mock operation failed")) async {
        await setConfiguration(shouldFail: shouldFail, error: error)
    }
    
    @MainActor
    private func setConfiguration(shouldFail: Bool, error: Error) {
        _shouldFailNextOperation = shouldFail
        _operationError = error
    }
    
    func start() async throws {
        startCallCount += 1
        if _shouldFailNextOperation {
            throw _operationError
        }
        currentState = .running
    }
    
    func stop() async throws {
        stopCallCount += 1
        if _shouldFailNextOperation {
            throw _operationError
        }
        currentState = .stopped
    }
    
    func pause() async throws {
        pauseCallCount += 1
        if _shouldFailNextOperation {
            throw _operationError
        }
        currentState = .paused
    }
    
    func resume() async throws {
        resumeCallCount += 1
        if _shouldFailNextOperation {
            throw _operationError
        }
        currentState = .running
    }
    
    func getVirtualMachine() -> Any {
        return "mock_vm"
    }

    private var guestStopContinuation: CheckedContinuation<Error?, Never>?
    private var pendingGuestStopResult: Error??

    func waitForGuestStop() async -> Error? {
        if let result = pendingGuestStopResult {
            pendingGuestStopResult = nil
            currentState = .stopped
            return result
        }
        return await withCheckedContinuation { continuation in
            guestStopContinuation = continuation
        }
    }

    /// Simulate a normal guest shutdown.
    func simulateGuestStop() {
        currentState = .stopped
        if let continuation = guestStopContinuation {
            guestStopContinuation = nil
            continuation.resume(returning: nil)
        } else {
            pendingGuestStopResult = .some(nil)
        }
    }

    /// Simulate a guest crash with the given error.
    func simulateGuestError(_ error: Error) {
        currentState = .stopped
        if let continuation = guestStopContinuation {
            guestStopContinuation = nil
            continuation.resume(returning: error)
        } else {
            pendingGuestStopResult = .some(error)
        }
    }
}
