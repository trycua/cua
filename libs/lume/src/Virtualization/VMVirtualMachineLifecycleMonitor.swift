import Foundation
import Virtualization

private enum VMTerminalEvent: Sendable {
  case stopped
  case failed(String)
}

private struct VMFatalStopError: LocalizedError {
  let message: String
  var errorDescription: String? { "The virtual machine stopped with an error: \(message)" }
}

/// Converts VZVirtualMachineDelegate terminal callbacks into one cancellable lifecycle wait.
final class VMVirtualMachineLifecycleMonitor: NSObject, @unchecked Sendable,
  VZVirtualMachineDelegate
{
  private let events: AsyncStream<VMTerminalEvent>
  private let eventContinuation: AsyncStream<VMTerminalEvent>.Continuation
  private var finished = false

  override init() {
    var continuation: AsyncStream<VMTerminalEvent>.Continuation!
    events = AsyncStream { continuation = $0 }
    eventContinuation = continuation
    super.init()
  }

  func waitForStop() async throws {
    for await event in events {
      switch event {
      case .stopped:
        return
      case .failed(let message):
        throw VMFatalStopError(message: message)
      }
    }
    throw CancellationError()
  }

  func stoppedByHost() {
    finish(with: .stopped)
  }

  func guestDidStop(_ virtualMachine: VZVirtualMachine) {
    finish(with: .stopped)
  }

  func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: any Error) {
    finish(with: .failed(error.localizedDescription))
  }

  private func finish(with event: VMTerminalEvent) {
    guard !finished else { return }
    finished = true
    eventContinuation.yield(event)
    eventContinuation.finish()
  }
}
