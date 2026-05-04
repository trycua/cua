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
}
