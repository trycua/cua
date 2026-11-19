import Foundation
import Virtualization

/// Turns SIGINT and SIGTERM into a guest shutdown. The first signal asks the
/// guest to shut down; if the VM is still running after timeout, it is forced
/// off and forced is set. Later signals are ignored: the timeout bounds the
/// stop. Without one, the default handlers end the process at once.
@MainActor
final class GracefulStop {
    private let service: VMVirtualizationService
    private let timeout: TimeInterval
    private let name: String
    private var sources: [DispatchSourceSignal] = []
    private var requested = false
    /// The VM was forced off: the guest did not shut down by itself.
    private(set) var forced = false

    init(service: VMVirtualizationService, timeout: TimeInterval, name: String) {
        self.service = service
        self.timeout = timeout
        self.name = name
        for sig in [SIGINT, SIGTERM] {
            signal(sig, SIG_IGN)  // the dispatch source receives it instead
            let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            source.setEventHandler { [weak self] in
                MainActor.assumeIsolated { self?.stop(signal: sig) }
            }
            source.resume()
            sources.append(source)
        }
    }

    private func stop(signal: Int32) {
        guard !requested else { return }
        requested = true
        Logger.info(
            "Asking the guest to shut down",
            metadata: ["name": name, "signal": "\(signal)", "timeout": "\(Int(timeout)) s"])
        do {
            try service.requestStop()
        } catch {
            Logger.error(
                "The guest cannot be asked to shut down",
                metadata: ["name": name, "error": error.localizedDescription])
            force()
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.service.state != .stopped else { return }
                Logger.error(
                    "The guest did not shut down in time", metadata: ["name": self.name])
                self.force()
            }
        }
    }

    private func force() {
        forced = true
        forceStop(service, name: name)
    }
}

/// Forces the VM off. If that fails while the VM still runs, the process exits,
/// and the VM ends with it.
@MainActor
func forceStop(_ service: VMVirtualizationService, name: String) {
    Task { @MainActor in
        do {
            try await service.stop()
        } catch {
            // A VM that stopped by itself, with an error or not, ends the run as usual.
            guard service.state != .stopped, service.state != .error else { return }
            Logger.error(
                "Forced stop failed; exiting", metadata: ["name": name, "error": error.localizedDescription])
            Foundation.exit(EXIT_FAILURE)
        }
    }
}
