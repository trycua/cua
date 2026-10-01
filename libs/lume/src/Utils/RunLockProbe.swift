import Darwin
import Foundation

/// Resolves which processes currently hold a VM's `config.json` open.
///
/// `lume run` keeps that file open under an exclusive `flock` for the entire
/// lifetime of a session, so its holder is the authoritative cross-process
/// liveness signal. `stop` and `attach` already trust it; VNC-disabled runs
/// reuse it because they have no VNC port to probe.
protocol RunLockProbe: Sendable {
    /// Process identifiers holding the file at `path` open, in `lsof` order.
    /// Empty means no process has it open. Nil means the probe was inconclusive.
    func lockHolderPIDs(ofFileAt path: String) -> [pid_t]?
}

extension RunLockProbe {
    /// The single owning process, matching the "first holder wins" rule that
    /// `stop` and `attach` have always used.
    func lockOwnerPID(ofFileAt path: String) -> pid_t? {
        lockHolderPIDs(ofFileAt: path)?.first
    }

    /// True only when `pid` is a live holder of the run lock.
    ///
    /// A recorded PID is never trusted on its own: the process may have exited
    /// and had its identifier recycled by something unrelated.
    func isLiveLockHolder(pid: pid_t, ofFileAt path: String) -> Bool? {
        guard pid > 0 else { return false }
        return lockHolderPIDs(ofFileAt: path)?.contains(pid)
    }
}

/// Carries a reference across a `@Sendable` boundary where the compiler cannot
/// prove safety but the callee's use is thread-safe.
private final class UncheckedBox<Value>: @unchecked Sendable {
    let value: Value

    init(_ value: Value) {
        self.value = value
    }
}

/// `lsof`-backed probe with a hard wall-clock bound.
///
/// `lume list` runs this once per VM that recorded a VNC-disabled session, so a
/// slow or wedged `lsof` must never stall the caller indefinitely.
struct LsofRunLockProbe: RunLockProbe {
    static let defaultTimeout: TimeInterval = 2.0

    let timeout: TimeInterval

    init(timeout: TimeInterval = LsofRunLockProbe.defaultTimeout) {
        self.timeout = timeout
    }

    func lockHolderPIDs(ofFileAt path: String) -> [pid_t]? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = ["-n", "-P", "-w", "-t", path]
        let output = Pipe()
        process.standardOutput = output
        // Share one pipe so exit 1 with no bytes means "no holders", while an
        // lsof diagnostic cannot be mistaken for that definitive empty result.
        process.standardError = output

        do {
            try process.run()
        } catch {
            Logger.debug(
                "Could not run lsof to resolve the VM run lock owner",
                metadata: ["path": path, "error": error.localizedDescription])
            return nil
        }

        // Terminating the child closes the pipe, so the pending read below
        // always unblocks even if lsof itself is stuck. `Process` is
        // thread-safe for termination but not `Sendable`, so it crosses into
        // the watchdog through an explicit box.
        let box = UncheckedBox(process)
        let watchdog = DispatchWorkItem {
            let child = box.value
            if child.isRunning {
                child.terminate()
            }
        }
        DispatchQueue.global(qos: .userInitiated).asyncAfter(
            deadline: .now() + timeout, execute: watchdog)
        let data = (try? output.fileHandleForReading.readToEnd()) ?? Data()
        process.waitUntilExit()
        watchdog.cancel()

        if process.terminationReason == .uncaughtSignal {
            return nil
        }
        if process.terminationStatus == 1 && data.isEmpty {
            return []
        }
        guard process.terminationStatus == 0 else {
            return nil
        }
        return Self.parsePIDs(from: data)
    }

    /// Parses `lsof -t` output, which emits one decimal PID per holder.
    static func parsePIDs(from data: Data) -> [pid_t]? {
        guard let text = String(data: data, encoding: .utf8) else { return nil }
        let lines = text.split(whereSeparator: \.isNewline)
        var result: [pid_t] = []
        for line in lines {
            guard let pid = pid_t(line), pid > 0 else {
                return nil
            }
            result.append(pid)
        }
        return result
    }
}
