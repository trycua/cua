import ArgumentParser
import CuaDriverCore
import CuaDriverServer
import Darwin
import Foundation
import MCP

/// `cua-driver serve` — binds a Unix domain socket, accepts line-delimited
/// JSON requests, and dispatches them against a process-global ToolRegistry
/// whose state lives for the daemon's lifetime. The shared AppStateEngine
/// is why this exists: element_index workflows need the per-pid cache to
/// survive between CLI invocations.
struct ServeCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "serve",
        abstract: "Run cua-driver as a long-running daemon on a Unix domain socket.",
        discussion: """
            Listens on --socket (default: ~/Library/Caches/cua-driver/cua-driver.sock).
            Subsequent `cua-driver call/list-tools/describe` invocations auto-detect
            the socket and forward their requests, so the AppStateEngine's per-pid
            element_index cache survives across CLI calls.

            Protocol: line-delimited JSON. Each request is
              {"method":"call","name":"<tool>","args":{...}} |
              {"method":"list"} | {"method":"describe","name":"..."} |
              {"method":"shutdown"}
            Responses: {"ok":true,"result":...} or
                       {"ok":false,"error":"...","exitCode":64|65|70|1}.

            TCC responsibility chain: when invoked via the `~/.local/bin/cua-driver`
            symlink from a shell that itself lacks Accessibility + Screen Recording
            grants (any IDE terminal — Claude Code, Cursor, VS Code, Conductor),
            macOS attributes the serve process to the parent shell/IDE, not to
            CuaDriver.app. AX probes no-op silently and the daemon never becomes
            useful. To sidestep, `serve` detects that context and re-execs itself
            via `open -n -g -a CuaDriver --args serve`, which relaunches under
            LaunchServices so TCC attributes the process to com.trycua.driver.
            Pass `--no-relaunch` (or set `CUA_DRIVER_NO_RELAUNCH=1`) to opt out
            and stay in the current process — useful when you know the caller
            already has the right TCC context or you're deliberately testing the
            in-process path.
            """
    )

    @Option(name: .long, help: "Override the Unix socket path.")
    var socket: String?

    @Flag(
        name: .long,
        help: """
            Stay in the current process instead of re-execing via `open -n -g -a CuaDriver`. \
            Use when the calling context already has the right TCC responsibility \
            (running inside CuaDriver.app directly, or from a shell that's been \
            granted Accessibility + Screen Recording itself). Also toggleable via \
            CUA_DRIVER_NO_RELAUNCH=1.
            """
    )
    var noRelaunch: Bool = false

    func run() throws {
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()
        let pidFilePath = socket == nil ? DaemonPaths.defaultPidFilePath() : nil

        // TCC responsibility chain sidestep. Only applies to the default
        // socket — an explicit --socket means the caller is orchestrating
        // their own daemon layout and knows what they're doing.
        if socket == nil && shouldRelaunchViaOpen() {
            try relaunchViaOpen(socketPath: socketPath)
            return
        }

        // Fast-fail if another daemon is already answering on this
        // socket. Without this check the second `cua-driver serve`
        // would happily `unlink()` the live socket file and bind its
        // own — leaving the original daemon orphaned (still running,
        // no socket) and callers hitting the new one. The probe speaks
        // the real protocol (trivial `list` request), so a stale socket
        // file from a crashed daemon correctly returns "not listening"
        // and lets startup proceed.
        if DaemonClient.isDaemonListening(socketPath: socketPath) {
            let pidHint: String
            if let pidFilePath, let pid = readDaemonPid(from: pidFilePath) {
                pidHint = " (pid \(pid))"
            } else {
                pidHint = ""
            }
            FileHandle.standardError.write(
                Data(
                    "cua-driver daemon is already running on \(socketPath)\(pidHint). "
                        .utf8))
            FileHandle.standardError.write(
                Data("Run `cua-driver stop` first.\n".utf8))
            throw ExitCode(1)
        }

        // Daemon runs for the lifetime of the process, so we bootstrap
        // AppKit here just like MCPCommand does — the agent cursor
        // overlay (disabled by default, enabled via
        // `set_agent_cursor_enabled`) needs a live NSApplication event
        // loop. Idle cost when the cursor is never enabled is one
        // run-loop thread, which is cheap compared to the daemon's
        // socket + AX work.
        let useDefaultSocket = (socket == nil)

        AppKitBootstrap.runBlockingAppKitWith {
            // Preflight TCC grants BEFORE we acquire the daemon lock —
            // otherwise a first-run user who needs to grant perms would
            // be blocked by "another daemon starting" if they ran
            // `serve` once, saw the permissions panel, and triggered
            // any sibling probe. Panel flow is idempotent and cheap
            // (<50ms) when grants are already live.
            let granted = await MainActor.run {
                PermissionsGate.shared
            }.ensureGranted()
            if !granted {
                FileHandle.standardError.write(
                    Data(
                        "cua-driver: required permissions (Accessibility + Screen Recording) not granted; daemon exiting.\n"
                            .utf8))
                throw AppKitBootstrapError.permissionsDenied
            }

            // Advisory flock on a dedicated lock file — only applied
            // for the default socket path (passing `--socket` is an
            // explicit opt-in to running a second daemon elsewhere,
            // and that caller takes responsibility). `_lockFD` is
            // intentionally retained for the whole daemon lifetime:
            // the kernel releases the flock when this fd closes,
            // including on process exit. See
            // `acquireDaemonLockOrExit` for why we don't reuse the
            // pid file.
            let _lockFD: Int32?
            if useDefaultSocket {
                _lockFD = try acquireDaemonLockOrExit()
            } else {
                _lockFD = nil
            }
            _ = _lockFD  // silence "variable never used" in release

            // Warm the persistent config so any decode warnings surface
            // in the daemon's stderr at startup rather than on the first
            // tool call. A missing or malformed file falls through to
            // defaults inside `ConfigStore.load()` — no failure here.
            let config = await ConfigStore.shared.load()

            // Non-blocking version hint — prints to stderr if a newer release
            // exists on GitHub. Fails silently when offline.
            VersionCheck.warnIfOutdated()

            // Propagate persisted agent-cursor preferences into the live
            // singleton so the daemon boots in the same state the user
            // last wrote. `setAgentCursorEnabled` / `setAgentCursorMotion`
            // keep mutating live state AND writing back through
            // ConfigStore, so this boot-time apply closes the loop.
            await MainActor.run {
                AgentCursor.shared.apply(config: config.agentCursor)
            }

            let server = DaemonServer(
                socketPath: socketPath, pidFilePath: pidFilePath)
            try await server.run()
        }
    }
}

extension ServeCommand {
    /// Decide whether the current `serve` invocation should re-exec itself
    /// via `/usr/bin/open -n -g -a CuaDriver --args serve`. True when all of
    /// the following hold:
    ///
    ///   - `--no-relaunch` is NOT set and `CUA_DRIVER_NO_RELAUNCH` is not
    ///     truthy in the environment.
    ///   - `Bundle.main.bundlePath` does NOT end in `.app`. That's the
    ///     signal we were invoked as a bare binary — almost always the
    ///     `~/.local/bin/cua-driver` symlink from a shell — rather
    ///     than as the main executable of a loaded `.app` bundle. The
    ///     `open -n -g -a` path always lands in the second form
    ///     (bundlePath ends in `/CuaDriver.app`), so checking for its
    ///     absence distinguishes "shell-spawned via symlink" from
    ///     "already relaunched by LaunchServices" without a loop risk.
    ///   - The symlink / argv path resolves (via `realpath`) to a file
    ///     living inside some `CuaDriver.app/Contents/MacOS/`. This
    ///     rules out raw `swift run cua-driver serve` dev invocations,
    ///     where the resolved binary lives under `.build/<config>/` —
    ///     no `.app` to relaunch into.
    ///   - `getppid() != 1`: we were spawned by a regular process
    ///     (shell, IDE, another daemon), not by `launchd` / LaunchServices.
    ///     `open -n -g -a` always reparents the launched app to launchd,
    ///     so a process with ppid == 1 already came in through the
    ///     LaunchServices path. This is a belt-and-suspenders check on
    ///     top of the bundlePath heuristic.
    ///
    /// The point: shell-spawned subprocesses inherit the parent shell /
    /// IDE's TCC responsibility chain, which means AX + Screen Recording
    /// checks are evaluated against the IDE's bundle id — not
    /// com.trycua.driver — and the daemon's AppKitBootstrap silently
    /// no-ops. Bouncing through `open` relaunches under LaunchServices so
    /// TCC attributes the fresh process to CuaDriver.app.
    fileprivate func shouldRelaunchViaOpen() -> Bool {
        if noRelaunch { return false }
        if isEnvTruthy(ProcessInfo.processInfo.environment["CUA_DRIVER_NO_RELAUNCH"]) {
            return false
        }
        // When Bundle.main.bundlePath ends in `.app` we're already the
        // main executable of a loaded bundle — either LaunchServices
        // launched us (good, no relaunch needed) or the user invoked
        // `/Applications/CuaDriver.app/Contents/MacOS/cua-driver`
        // directly (the symlink-less path, also fine to leave alone).
        if Bundle.main.bundlePath.hasSuffix(".app") { return false }
        // Otherwise: we're running from some bare path. Only relaunch
        // if that bare path actually resolves into a CuaDriver.app
        // bundle on disk — the symlink case. Raw `swift run` dev
        // invocations resolve into `.build/<config>/cua-driver`
        // instead, and have no bundle to relaunch into.
        guard resolvedExecutableIsInsideCuaDriverApp() else { return false }
        // ppid == 1 means we're already a LaunchServices-spawned process
        // (or orphaned into init, in which case relaunching wouldn't
        // change anything useful anyway).
        if getppid() == 1 { return false }
        return true
    }

    /// Spawn `/usr/bin/open -n -g -a CuaDriver --args serve [--socket …]`,
    /// then wait (up to 5s) for the canonical daemon socket to accept a
    /// protocol-speaking probe. The `open` CLI returns immediately once
    /// LaunchServices accepts the request, which is well before the
    /// daemon has bound its socket — hence the poll.
    ///
    /// On success: prints a confirmation to stdout and returns so the
    /// CLI exits 0. On failure: writes a diagnostic and throws
    /// `ExitCode(1)`, explicitly pointing at `--no-relaunch` as the
    /// escape hatch.
    fileprivate func relaunchViaOpen(socketPath: String) throws {
        FileHandle.standardError.write(
            Data(
                "cua-driver: relaunching via `open -n -g -a CuaDriver --args serve` for correct TCC context. Pass --no-relaunch to stay in this process.\n"
                    .utf8))

        // If --socket was ever passed through to `serve`, forward it to
        // the relaunched instance so the caller still hits the same
        // daemon. Today we only relaunch when socket == nil (the default
        // socket path), but carrying the flag through keeps the helper
        // correct if that guard ever loosens.
        var extraArgs: [String] = []
        if let socket = socket {
            extraArgs += ["--socket", socket]
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        // -n: force a new instance. Critical — `open -a CuaDriver`
        //     against an already-running CuaDriver process (e.g. a
        //     `cua-driver mcp` started by an MCP client) would re-use
        //     that instance and drop our `--args serve`, leaving us
        //     with "launched something but no serve daemon appeared".
        // -g: keep the new instance in the background. CuaDriver.app is
        //     LSUIElement=true so it wouldn't take focus anyway, but this
        //     makes that explicit.
        process.arguments = ["-n", "-g", "-a", "CuaDriver", "--args", "serve"] + extraArgs
        // Discard `open`'s own stdout/stderr — on success it's silent,
        // on failure the exit code is what we care about.
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
        } catch {
            FileHandle.standardError.write(
                Data(
                    "cua-driver: failed to exec `/usr/bin/open`: \(error)\n"
                        .utf8))
            throw ExitCode(1)
        }
        process.waitUntilExit()
        if process.terminationStatus != 0 {
            FileHandle.standardError.write(
                Data(
                    "cua-driver: `open -n -g -a CuaDriver --args serve` exited \(process.terminationStatus). Check that `/Applications/CuaDriver.app` is installed, or pass --no-relaunch to bypass.\n"
                        .utf8))
            throw ExitCode(1)
        }

        // Poll for the daemon to come up. 5s covers a cold launch
        // including the first-run PermissionsGate preflight; on a warm
        // machine the socket usually appears in <500ms.
        let deadline = Date().addingTimeInterval(5.0)
        var probeReachable = false
        while Date() < deadline {
            if DaemonClient.isDaemonListening(socketPath: socketPath) {
                probeReachable = true
                break
            }
            usleep(100_000)  // 100ms
        }

        if probeReachable {
            FileHandle.standardOutput.write(
                Data(
                    "cua-driver daemon is running (relaunched via CuaDriver.app)\n  socket: \(socketPath)\n"
                        .utf8))
            return
        }

        FileHandle.standardError.write(
            Data(
                "cua-driver: relaunched CuaDriver.app but no daemon appeared on \(socketPath) within 5s. Check Accessibility + Screen Recording grants for CuaDriver.app, or re-run with --no-relaunch to see in-process errors.\n"
                    .utf8))
        throw ExitCode(1)
    }

    /// True when the argv[0] / executablePath resolves (through any
    /// symlinks) to a binary physically living inside some
    /// `CuaDriver.app/Contents/MacOS/` directory. That's the "installed
    /// via install-local.sh / install.sh" shape — `~/.local/bin/cua-driver`
    /// is a symlink into `/Applications/CuaDriver.app`, and `realpath`
    /// walks into the bundle.
    ///
    /// Returns false for `swift run` / raw `.build/<config>/cua-driver`
    /// dev invocations, which have no installed bundle to relaunch into.
    private func resolvedExecutableIsInsideCuaDriverApp() -> Bool {
        // Prefer Foundation's executablePath (stable, absolute).
        // Fall back to argv[0] when unset, which realpath() still
        // resolves via $PATH lookup at the shell level — good enough
        // for the cases we care about.
        let candidate = Bundle.main.executablePath
            ?? CommandLine.arguments.first
            ?? ""
        guard !candidate.isEmpty else { return false }

        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        guard realpath(candidate, &buffer) != nil else { return false }
        let resolved = String(cString: buffer)
        return resolved.contains("/CuaDriver.app/Contents/MacOS/")
    }

    /// Accepts the same truthy-value conventions the rest of the CLI
    /// uses for env overrides (see `UpdateCommand` / `TelemetryClient`).
    private func isEnvTruthy(_ value: String?) -> Bool {
        guard let value = value?.lowercased() else { return false }
        return ["1", "true", "yes", "on"].contains(value)
    }
}

/// Open (or create) the shared lock file and acquire a non-blocking
/// exclusive flock. On contention, emits a helpful error pointing the
/// user at `cua-driver status` / `cua-driver stop` and throws
/// `ExitCode(1)`. Returns the held fd; callers must keep it alive for
/// as long as the lock should persist — the kernel releases flocks
/// when the owning fd is closed (including by process exit).
private func acquireDaemonLockOrExit() throws -> Int32 {
    let lockPath = DaemonPaths.defaultLockFilePath()
    // Mirror the directory creation the DaemonServer does — we may be
    // the first code touching the cache dir on a fresh install.
    let dir = URL(fileURLWithPath: (lockPath as NSString).deletingLastPathComponent)
    try FileManager.default.createDirectory(
        at: dir, withIntermediateDirectories: true, attributes: nil)

    let fd = open(lockPath, O_RDWR | O_CREAT, 0o600)
    guard fd >= 0 else {
        FileHandle.standardError.write(
            Data(
                "cua-driver: failed to open lock file \(lockPath): \(String(cString: strerror(errno)))\n"
                    .utf8))
        throw ExitCode(1)
    }
    // LOCK_EX: exclusive. LOCK_NB: fail instead of block when held by
    // another process. We intentionally don't retry — a contended lock
    // always means "another serve is in play" and we want fail-fast.
    if flock(fd, LOCK_EX | LOCK_NB) != 0 {
        let err = errno
        close(fd)
        if err == EWOULDBLOCK {
            FileHandle.standardError.write(
                Data(
                    "cua-driver: another daemon is starting or running (lock held on \(lockPath)). "
                        .utf8))
            FileHandle.standardError.write(
                Data("Run `cua-driver status` to check.\n".utf8))
        } else {
            FileHandle.standardError.write(
                Data(
                    "cua-driver: flock(\(lockPath)) failed: \(String(cString: strerror(err)))\n"
                        .utf8))
        }
        throw ExitCode(1)
    }
    return fd
}

/// `cua-driver stop` — sends `{"method":"shutdown"}` to a running daemon
/// and polls for the socket file to disappear. Exits 1 if no daemon was
/// running in the first place.
struct StopCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "stop",
        abstract: "Ask the running cua-driver daemon to exit gracefully."
    )

    @Option(name: .long, help: "Override the Unix socket path.")
    var socket: String?

    func run() async throws {
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()

        guard DaemonClient.isDaemonListening(socketPath: socketPath) else {
            FileHandle.standardError.write(
                Data("cua-driver daemon is not running\n".utf8)
            )
            throw ExitCode(1)
        }

        let result = DaemonClient.sendRequest(
            DaemonRequest(method: "shutdown"),
            socketPath: socketPath
        )
        switch result {
        case .ok:
            break
        case .noDaemon:
            FileHandle.standardError.write(
                Data("cua-driver daemon disappeared before shutdown request\n".utf8)
            )
            throw ExitCode(1)
        case .error(let message):
            FileHandle.standardError.write(Data("stop: \(message)\n".utf8))
            throw ExitCode(1)
        }

        // Poll for the socket file to vanish — up to 2s. The daemon unlinks
        // it during its shutdown handler, so the absence is proof of clean
        // exit rather than "probably gone".
        let deadline = Date().addingTimeInterval(2.0)
        while Date() < deadline {
            if !FileManager.default.fileExists(atPath: socketPath) {
                return
            }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        FileHandle.standardError.write(
            Data("cua-driver daemon did not release socket within 2s\n".utf8)
        )
        throw ExitCode(1)
    }
}

/// `cua-driver status` — prints whether a daemon is currently reachable
/// on the socket plus its pid when known. Exits 1 when no daemon is up.
struct StatusCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "status",
        abstract: "Report whether a cua-driver daemon is running."
    )

    @Option(name: .long, help: "Override the Unix socket path.")
    var socket: String?

    @Option(name: .long, help: "Override the daemon pid-file path.")
    var pidFile: String?

    func run() async throws {
        let socketPath = socket ?? DaemonPaths.defaultSocketPath()
        let pidFilePath = pidFile ?? DaemonPaths.defaultPidFilePath()

        // Probe by sending a trivial `list` — connecting alone doesn't
        // prove the peer speaks our protocol. If list succeeds, we're
        // talking to a live cua-driver daemon.
        let probe = DaemonClient.sendRequest(
            DaemonRequest(method: "list"), socketPath: socketPath
        )

        switch probe {
        case .ok(let response) where response.ok:
            FileHandle.standardOutput.write(
                Data("cua-driver daemon is running\n".utf8)
            )
            FileHandle.standardOutput.write(
                Data("  socket: \(socketPath)\n".utf8)
            )
            if let pid = readDaemonPid(from: pidFilePath) {
                FileHandle.standardOutput.write(
                    Data("  pid: \(pid)\n".utf8)
                )
            } else {
                FileHandle.standardOutput.write(
                    Data("  pid: unknown (no pid file)\n".utf8)
                )
            }
        default:
            FileHandle.standardError.write(
                Data("cua-driver daemon is not running\n".utf8)
            )
            throw ExitCode(1)
        }
    }
}
