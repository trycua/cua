import Darwin
import Foundation
import MCP

/// Long-running daemon that accepts `cua-driver call`/`list-tools`/`describe`
/// requests over a Unix domain socket and dispatches them against a single
/// shared `ToolRegistry`. Because `AppStateRegistry` is module-level state,
/// every connected client sees the same AppStateEngine / FocusGuard /
/// SystemFocusStealPreventer — which is the whole point. A one-shot
/// `cua-driver call get_window_state` followed by a one-shot
/// `cua-driver call click` would otherwise hit "No cached state for pid X"
/// because the AppStateEngine dies with the short-lived CLI process.
public actor DaemonServer {
    public let socketPath: String
    public let pidFilePath: String?
    private let registry: ToolRegistry
    private var listenFD: Int32 = -1
    // Self-pipe used to break the accept loop out of poll() when shutdown()
    // fires. Writing a single byte to pipeWriteFD flips POLLIN on
    // pipeReadFD, which the loop notices alongside EAGAIN on the listener.
    private var pipeReadFD: Int32 = -1
    private var pipeWriteFD: Int32 = -1
    private var running: Bool = true
    private var shutdownContinuation: CheckedContinuation<Void, Never>?
    // Retained DispatchSourceSignal handles for SIGINT/SIGTERM. Dispatch
    // sources stop firing the instant they're deallocated, so we hold them
    // here for the daemon's lifetime. The process exits when run() returns,
    // at which point the OS reclaims these; no explicit .cancel() needed.
    private var signalSources: [any DispatchSourceSignal] = []

    public init(
        socketPath: String = DaemonPaths.defaultSocketPath(),
        pidFilePath: String? = DaemonPaths.defaultPidFilePath(),
        registry: ToolRegistry = .default
    ) {
        self.socketPath = socketPath
        self.pidFilePath = pidFilePath
        self.registry = registry
    }

    /// Bind the listen socket, write the pid file, and run the accept loop
    /// until `shutdown()` or a signal fires. The call returns only after
    /// the server has fully stopped.
    public func run() async throws {
        try prepareSocketDirectory()
        removeStaleSocketFile()
        try bindListener()
        try createShutdownPipe()
        try writePidFile()

        // Graceful-shutdown-on-signal. SIGINT/SIGTERM flip `running` and
        // close the listen fd so `accept()` returns; no pthread signal
        // tricks needed since the accept loop runs on a background task.
        installSignalHandlers()

        FileHandle.standardOutput.write(
            Data("cua-driver daemon started on \(socketPath)\n".utf8)
        )

        // Accept loop runs on a detached task so we can await its completion
        // while the main run() awaits the shutdown continuation.
        let socketPathSnapshot = self.socketPath
        let pidFileSnapshot = self.pidFilePath
        let listenFDSnapshot = self.listenFD
        let pipeReadSnapshot = self.pipeReadFD
        let daemonRef = self

        let registrySnapshot = self.registry
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            self.shutdownContinuation = continuation
            Task.detached { [daemonRef] in
                _ = await DaemonServer.acceptLoopStatic(
                    listenFD: listenFDSnapshot,
                    shutdownPipeFD: pipeReadSnapshot,
                    registry: registrySnapshot
                )
                // acceptLoopStatic only returns once shutdown is requested.
                // Clean up filesystem artifacts and wake run().
                Self.cleanupFilesystem(
                    socketPath: socketPathSnapshot, pidFilePath: pidFileSnapshot
                )
                await daemonRef.notifyShutdown()
            }
        }
    }

    /// Flip the shutdown flag and poke the wakeup pipe so the accept loop's
    /// poll() returns immediately. Idempotent — extra pokes are harmless.
    public func shutdown() {
        guard running else { return }
        running = false
        // Writing to the wakeup pipe is what breaks the poll() in the
        // accept loop. close(listenFD) alone doesn't wake blocked accept()
        // on Darwin.
        if pipeWriteFD >= 0 {
            var byte: UInt8 = 0x00
            _ = write(pipeWriteFD, &byte, 1)
        }
    }

    private func createShutdownPipe() throws {
        var fds: [Int32] = [0, 0]
        let result = fds.withUnsafeMutableBufferPointer { buffer -> Int32 in
            pipe(buffer.baseAddress!)
        }
        guard result == 0 else {
            throw DaemonError.systemCall("pipe", errno: errno)
        }
        pipeReadFD = fds[0]
        pipeWriteFD = fds[1]
    }

    fileprivate func notifyShutdown() {
        if let continuation = shutdownContinuation {
            shutdownContinuation = nil
            continuation.resume()
        }
    }

    // MARK: - setup

    private func prepareSocketDirectory() throws {
        let url = URL(fileURLWithPath: (socketPath as NSString).deletingLastPathComponent)
        try FileManager.default.createDirectory(
            at: url, withIntermediateDirectories: true, attributes: nil
        )
    }

    private func removeStaleSocketFile() {
        // Any prior bind leaves the socket file in place even after the
        // owning process dies. Must unlink before bind() or the kernel
        // returns EADDRINUSE.
        _ = unlink(socketPath)
    }

    private func bindListener() throws {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw DaemonError.systemCall("socket", errno: errno)
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = Array(socketPath.utf8)
        let maxLen = MemoryLayout.size(ofValue: addr.sun_path) - 1
        guard pathBytes.count <= maxLen else {
            close(fd)
            throw DaemonError.socketPathTooLong(socketPath, limit: maxLen)
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            let buffer = raw.bindMemory(to: UInt8.self)
            for (index, byte) in pathBytes.enumerated() { buffer[index] = byte }
            buffer[pathBytes.count] = 0
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let bindResult = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                Darwin.bind(fd, sockPtr, addrLen)
            }
        }
        guard bindResult == 0 else {
            let savedErrno = errno
            close(fd)
            throw DaemonError.systemCall("bind", errno: savedErrno)
        }

        // 0600 — only the owning user gets to talk to this socket.
        _ = chmod(socketPath, 0o600)

        guard listen(fd, 8) == 0 else {
            let savedErrno = errno
            close(fd)
            _ = unlink(socketPath)
            throw DaemonError.systemCall("listen", errno: savedErrno)
        }

        listenFD = fd
    }

    private func writePidFile() throws {
        guard let pidFilePath else { return }
        let pid = ProcessInfo.processInfo.processIdentifier
        let bytes = "\(pid)\n".data(using: .utf8) ?? Data()
        let url = URL(fileURLWithPath: pidFilePath)
        try bytes.write(to: url, options: .atomic)
    }

    private func installSignalHandlers() {
        // SIGPIPE stays ignored — we need write() to return EPIPE, not
        // get interrupted by a signal handler.
        signal(SIGPIPE, SIG_IGN)

        // Mask SIGINT/SIGTERM from default delivery so DispatchSource is
        // the only consumer. Without this, libdispatch and the default
        // handler both race for the signal.
        signal(SIGINT, SIG_IGN)
        signal(SIGTERM, SIG_IGN)

        // DispatchSourceSignal fires its event handler on the given queue,
        // OUTSIDE the POSIX signal-handler context. That makes it safe to
        // call into Swift concurrency (Task.detached, actors) from the
        // handler — unlike a raw `signal()` C-convention handler, where
        // Swift runtime calls are undefined behavior.
        let queue = DispatchQueue.global(qos: .userInitiated)
        for sig in [SIGINT, SIGTERM] {
            let source = DispatchSource.makeSignalSource(signal: sig, queue: queue)
            source.setEventHandler {
                Task.detached { await DaemonSignal.fireShutdown() }
            }
            source.resume()
            signalSources.append(source)
        }

        DaemonSignal.register(self)
    }

    // MARK: - accept loop

    /// `nonisolated` because it blocks in `poll()` — if this ran on the
    /// actor, no other actor-isolated call (including `shutdown()`) would
    /// get a chance to touch `pipeWriteFD` to wake it up. The loop only
    /// reads the fds passed in, so the nonisolated escape hatch is safe.
    fileprivate nonisolated static func acceptLoopStatic(
        listenFD: Int32,
        shutdownPipeFD: Int32,
        registry: ToolRegistry
    ) async -> Int32 {
        // Darwin doesn't interrupt a blocked accept() on close(), so we
        // drive accept from poll() and treat a wakeup on the shutdown
        // pipe as "stop listening".
        var localListenFD = listenFD
        var localPipeFD = shutdownPipeFD
        while true {
            var fds = [
                pollfd(fd: localListenFD, events: Int16(POLLIN), revents: 0),
                pollfd(fd: localPipeFD, events: Int16(POLLIN), revents: 0),
            ]
            let pollResult = fds.withUnsafeMutableBufferPointer { buffer -> Int32 in
                poll(buffer.baseAddress, nfds_t(buffer.count), -1)
            }
            if pollResult < 0 {
                if errno == EINTR { continue }
                // Any other poll error — bail.
                closeFDs(listenFD: &localListenFD, pipeFD: &localPipeFD)
                return 0
            }
            if fds[1].revents & Int16(POLLIN) != 0 {
                // Shutdown signaled — close fds and return.
                closeFDs(listenFD: &localListenFD, pipeFD: &localPipeFD)
                return 0
            }
            guard fds[0].revents & Int16(POLLIN) != 0 else { continue }

            var clientAddr = sockaddr_un()
            var clientLen = socklen_t(MemoryLayout<sockaddr_un>.size)
            let clientFD = withUnsafeMutablePointer(to: &clientAddr) { addrPtr in
                addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                    accept(localListenFD, sockPtr, &clientLen)
                }
            }
            if clientFD < 0 {
                // Transient accept() failure — log and continue.
                continue
            }

            // Each connection is serviced on its own detached task. ToolRegistry
            // is Sendable and AppStateRegistry members are actors, so concurrent
            // tool calls across clients are safe; the underlying AX actions
            // serialize through those actors.
            Task.detached {
                await DaemonServer.serveConnection(clientFD: clientFD, registry: registry)
            }
        }
    }

    fileprivate nonisolated static func closeFDs(listenFD: inout Int32, pipeFD: inout Int32) {
        if listenFD >= 0 {
            close(listenFD)
            listenFD = -1
        }
        if pipeFD >= 0 {
            close(pipeFD)
            pipeFD = -1
        }
    }

    fileprivate static func serveConnection(clientFD: Int32, registry: ToolRegistry) async {
        defer { close(clientFD) }

        var readBuffer = Data()
        var shouldShutdownDaemon = false

        readLoop: while true {
            var chunk = [UInt8](repeating: 0, count: 4096)
            let n = chunk.withUnsafeMutableBufferPointer { buf -> Int in
                read(clientFD, buf.baseAddress, buf.count)
            }
            if n <= 0 { break }
            readBuffer.append(contentsOf: chunk[0..<n])

            while let newlineIndex = readBuffer.firstIndex(of: 0x0A) {
                let lineData = readBuffer.prefix(upTo: newlineIndex)
                readBuffer.removeSubrange(0...newlineIndex)
                if lineData.isEmpty { continue }

                let response: DaemonResponse
                do {
                    let request = try JSONDecoder().decode(DaemonRequest.self, from: lineData)
                    if request.method == "shutdown" {
                        response = DaemonResponse(ok: true)
                        _ = writeLine(fd: clientFD, response: response)
                        shouldShutdownDaemon = true
                        break readLoop
                    }
                    response = await DaemonServer.dispatch(request: request, registry: registry)
                } catch {
                    response = DaemonResponse(
                        ok: false,
                        error: "invalid request line: \(error)",
                        exitCode: DaemonExit.dataError
                    )
                }
                if !writeLine(fd: clientFD, response: response) { break readLoop }
            }
        }

        if shouldShutdownDaemon {
            await DaemonSignal.fireShutdown()
        }
    }

    fileprivate static func dispatch(
        request: DaemonRequest, registry: ToolRegistry
    ) async -> DaemonResponse {
        switch request.method {
        case "list":
            return DaemonResponse(ok: true, result: .list(registry.allTools))
        case "describe":
            guard let name = request.name else {
                return DaemonResponse(
                    ok: false, error: "describe: missing 'name'", exitCode: DaemonExit.usage
                )
            }
            guard let handler = registry.handlers[name] else {
                return DaemonResponse(
                    ok: false, error: "Unknown tool: \(name)", exitCode: DaemonExit.usage
                )
            }
            return DaemonResponse(ok: true, result: .describe(handler.tool))
        case "call":
            guard let name = request.name else {
                return DaemonResponse(
                    ok: false, error: "call: missing 'name'", exitCode: DaemonExit.usage
                )
            }
            guard registry.handlers[name] != nil else {
                return DaemonResponse(
                    ok: false, error: "Unknown tool: \(name)", exitCode: DaemonExit.usage
                )
            }
            do {
                // Route through `registry.call(...)` (not the handler
                // directly) so the recording hook fires — daemon-side
                // tool invocations need to be recorded just like MCP
                // ones.
                let result = try await registry.call(name, arguments: request.args)
                return DaemonResponse(ok: true, result: .call(result))
            } catch {
                return DaemonResponse(
                    ok: false,
                    error: "Tool \(name) threw: \(error)",
                    exitCode: DaemonExit.software
                )
            }
        default:
            return DaemonResponse(
                ok: false,
                error: "unknown method: \(request.method)",
                exitCode: DaemonExit.usage
            )
        }
    }

    @discardableResult
    fileprivate static func writeLine(fd: Int32, response: DaemonResponse) -> Bool {
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.withoutEscapingSlashes]
            var data = try encoder.encode(response)
            data.append(0x0A)
            return data.withUnsafeBytes { raw -> Bool in
                guard let base = raw.baseAddress else { return false }
                var remaining = raw.count
                var offset = 0
                while remaining > 0 {
                    let n = write(fd, base.advanced(by: offset), remaining)
                    if n <= 0 { return false }
                    offset += n
                    remaining -= n
                }
                return true
            }
        } catch {
            FileHandle.standardError.write(
                Data("daemon: response encode failed: \(error)\n".utf8)
            )
            return false
        }
    }

    fileprivate static func cleanupFilesystem(
        socketPath: String, pidFilePath: String?
    ) {
        _ = unlink(socketPath)
        if let pidFilePath {
            _ = unlink(pidFilePath)
        }
    }
}

/// Module-level bridge so the C signal handler can reach the running daemon.
/// We only ever have one daemon per process, so a single weak reference is fine.
public actor DaemonSignal {
    public static let shared = DaemonSignal()
    private weak var current: DaemonServer?

    public static func register(_ server: DaemonServer) {
        Task { await shared.setCurrent(server) }
    }

    public static func fireShutdown() async {
        await shared.triggerShutdown()
    }

    private func setCurrent(_ server: DaemonServer) {
        self.current = server
    }

    private func triggerShutdown() async {
        await current?.shutdown()
    }
}

public enum DaemonError: Error, CustomStringConvertible {
    case systemCall(String, errno: Int32)
    case socketPathTooLong(String, limit: Int)

    public var description: String {
        switch self {
        case .systemCall(let name, let code):
            return "\(name) failed: \(String(cString: strerror(code))) (errno \(code))"
        case .socketPathTooLong(let path, let limit):
            return "socket path exceeds sun_path limit (\(limit) bytes): \(path)"
        }
    }
}
