import CuaDriverCore
import Foundation
import MCP

public enum CuaDriverMCPServer {
    /// Build an MCP Server actor with every CuaDriver tool registered.
    /// The caller is responsible for calling ``Server/start(transport:initializeHook:)``
    /// and ``Server/waitUntilCompleted()``.
    public static func make(
        serverName: String = "cua-driver",
        version: String = CuaDriverCore.version,
        registry: ToolRegistry = .default
    ) async -> Server {
        let server = Server(
            name: serverName,
            version: version,
            capabilities: Server.Capabilities(tools: .init(listChanged: false))
        )

        await server.withMethodHandler(ListTools.self) { _ in
            ListTools.Result(tools: registry.allTools)
        }

        await server.withMethodHandler(CallTool.self) { params in
            try await registry.call(params.name, arguments: params.arguments)
        }

        return server
    }

    /// Build an MCP Server whose `ListTools` / `CallTool` handlers forward
    /// every request to a running `cua-driver serve` daemon over its Unix
    /// domain socket. Used by the `mcp` subcommand's TCC-sidestep path:
    /// when stdio MCP is spawned from an IDE terminal, the process inherits
    /// the terminal's TCC responsibility chain so AX probes silently fail.
    /// Proxying through the daemon — which runs under LaunchServices and is
    /// correctly attributed to `com.trycua.driver` — gives MCP clients
    /// identical behavior without requiring an external Python bridge.
    ///
    /// `claudeCodeComputerUseCompat` advertises the compat tool set in
    /// `ListTools`, but every `CallTool` still hits the daemon. The daemon
    /// always exposes the full native registry; the shim is purely a
    /// client-side rename of `screenshot` and is implemented entirely by
    /// the in-process MCP layer. When proxying, we therefore rewrite the
    /// `screenshot` tool advertised to the client into its compat-mode
    /// shape and translate inbound `screenshot` calls back into the
    /// equivalent native daemon call.
    public static func makeProxy(
        serverName: String = "cua-driver",
        version: String = CuaDriverCore.version,
        socketPath: String,
        claudeCodeComputerUseCompat: Bool = false
    ) async throws -> Server {
        let server = Server(
            name: serverName,
            version: version,
            capabilities: Server.Capabilities(tools: .init(listChanged: false))
        )

        // Cache the tool list once at startup. Daemon registries are
        // static — every connected client sees the same handlers — so a
        // single fetch is enough for the life of the stdio MCP session.
        // Fail fast on a missing/unhealthy daemon so the MCP client sees
        // a clear startup error instead of a "successful" handshake that
        // advertises zero tools and then errors on every `CallTool`.
        let cachedToolList = try await fetchProxyToolList(
            socketPath: socketPath,
            claudeCodeComputerUseCompat: claudeCodeComputerUseCompat
        )

        await server.withMethodHandler(ListTools.self) { _ in
            ListTools.Result(tools: cachedToolList)
        }

        await server.withMethodHandler(CallTool.self) { params in
            let (name, args) = rewriteForProxy(
                name: params.name,
                arguments: params.arguments,
                claudeCodeComputerUseCompat: claudeCodeComputerUseCompat
            )
            return try await forwardCallToDaemon(
                name: name,
                arguments: args,
                socketPath: socketPath
            )
        }

        return server
    }

    /// Translate `(name, arguments)` from the MCP client's view of the
    /// compat tool surface into the native daemon registry's view.
    ///
    /// Compat-mode `screenshot` takes `{pid, window_id}` and returns a
    /// JPEG; the daemon's native `screenshot` takes `{window_id, format,
    /// quality}` and defaults to PNG. We map the former onto the latter
    /// by dropping the unused `pid` and pinning `format: "jpeg",
    /// quality: 85` to match the compat shim's output shape.
    ///
    /// Non-compat mode passes through unchanged.
    private static func rewriteForProxy(
        name: String,
        arguments: [String: Value]?,
        claudeCodeComputerUseCompat: Bool
    ) -> (String, [String: Value]?) {
        guard claudeCodeComputerUseCompat else { return (name, arguments) }
        if name == "screenshot" {
            var rewritten: [String: Value] = [:]
            if let windowID = arguments?["window_id"] {
                rewritten["window_id"] = windowID
            }
            rewritten["format"] = .string("jpeg")
            rewritten["quality"] = .int(85)
            return (name, rewritten)
        }
        return (name, arguments)
    }

    /// One-shot daemon `list` over the UDS, with the compat-mode rename
    /// applied client-side. Throws a descriptive `MCPError.internalError`
    /// if the daemon is unreachable, transport-failed, or returned an
    /// unexpected envelope — surfacing the failure during `makeProxy`'s
    /// init rather than producing a proxy that advertises zero tools and
    /// errors on every subsequent `CallTool`.
    private static func fetchProxyToolList(
        socketPath: String,
        claudeCodeComputerUseCompat: Bool
    ) async throws -> [Tool] {
        let request = DaemonRequest(method: "list")
        let result = DaemonClient.sendRequest(request, socketPath: socketPath)
        let tools: [Tool]
        switch result {
        case .noDaemon:
            throw MCPError.internalError(
                "cua-driver daemon not reachable on \(socketPath). "
                    + "Start it with `open -n -g -a CuaDriver --args serve` and retry."
            )
        case .error(let message):
            throw MCPError.internalError(
                "cua-driver daemon transport error while listing tools on \(socketPath): \(message)"
            )
        case .ok(let response):
            guard response.ok, case let .list(listed) = response.result else {
                let reason = response.error ?? "daemon returned unexpected result kind for list"
                throw MCPError.internalError(
                    "cua-driver daemon refused tool list on \(socketPath): \(reason)"
                )
            }
            tools = listed
        }
        if !claudeCodeComputerUseCompat {
            return tools
        }
        // Compat mode: swap the native `screenshot` tool descriptor for
        // the window-only shim's descriptor so MCP clients see the same
        // schema they'd see in the in-process compat registry.
        let compatHandlers = ClaudeCodeComputerUseCompatTools.all
        let compatToolsByName = Dictionary(
            uniqueKeysWithValues: compatHandlers.map { ($0.tool.name, $0.tool) }
        )
        return tools.map { tool in
            compatToolsByName[tool.name] ?? tool
        }
    }

    /// Forward a single `CallTool` invocation to the daemon and translate
    /// the `DaemonResponse` back into an MCP `CallTool.Result` (or throw
    /// `MCPError` on protocol-level failures).
    ///
    /// Tool-level errors — i.e. the tool ran but returned `isError: true`
    /// — round-trip cleanly as part of the `.call` payload, so MCP clients
    /// see exactly the same error envelope they would in the in-process
    /// path. Only daemon-level failures (socket gone, decode error, unknown
    /// tool) throw.
    private static func forwardCallToDaemon(
        name: String,
        arguments: [String: Value]?,
        socketPath: String
    ) async throws -> CallTool.Result {
        let request = DaemonRequest(method: "call", name: name, args: arguments)
        // Match the daemon's own per-call read budget. AX-heavy tools
        // (e.g. `screenshot`, `get_window_state`) regularly take a few
        // seconds; the default 120s in `DaemonClient` is plenty.
        let result = DaemonClient.sendRequest(request, socketPath: socketPath)
        switch result {
        case .noDaemon:
            throw MCPError.internalError(
                "cua-driver daemon not reachable on \(socketPath). "
                    + "Start it with `open -n -g -a CuaDriver --args serve` and retry."
            )
        case .error(let message):
            throw MCPError.internalError("daemon transport: \(message)")
        case .ok(let response):
            if !response.ok {
                let reason = response.error ?? "daemon reported failure"
                if response.exitCode == DaemonExit.usage {
                    throw MCPError.invalidParams(reason)
                }
                throw MCPError.internalError(reason)
            }
            guard case let .call(callResult) = response.result else {
                throw MCPError.internalError(
                    "daemon returned unexpected result kind for call"
                )
            }
            return callResult
        }
    }
}
