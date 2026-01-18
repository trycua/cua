import ArgumentParser
import Foundation

struct Serve: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Start the VM management server"
    )

    @Option(help: "Port to listen on (HTTP mode only)")
    var port: UInt16 = 7777

    @Flag(name: .long, help: "Run as MCP server (stdio transport) for AI agent integration")
    var mcp: Bool = false

    func run() async throws {
        if mcp {
            // MCP mode - run as MCP server over stdio
            TelemetryClient.shared.record(event: TelemetryEvent.serve, properties: [
                "mode": "mcp"
            ])

            let controller = LumeController()
            let mcpServer = await LumeMCPServer(controller: controller)

            // Note: Don't log to stdout in MCP mode as it interferes with the protocol
            try await mcpServer.start()
        } else {
            // HTTP mode - run as HTTP server
            TelemetryClient.shared.record(event: TelemetryEvent.serve, properties: [
                "port": port,
                "mode": "http"
            ])

            let server = await Server(port: port)

            Logger.info("Starting server", metadata: ["port": "\(port)"])

            // Using custom error handling to prevent ArgumentParser from printing additional error messages
            do {
                try await server.start()
            } catch let error as PortError {
                // For port errors, just log once with the suggestion
                let suggestedPort = port + 1

                // Create a user-friendly error message that includes the suggestion
                let message = """
                \(error.localizedDescription)
                Try using a different port: lume serve --port \(suggestedPort)
                """

                // Log the message (without the "ERROR:" prefix that ArgumentParser will add)
                Logger.error(message)

                // Exit with a custom code to prevent ArgumentParser from printing the error again
                Foundation.exit(1)
            } catch {
                // For other errors, log once
                Logger.error("Failed to start server", metadata: ["error": error.localizedDescription])
                throw error
            }
        }
    }
}