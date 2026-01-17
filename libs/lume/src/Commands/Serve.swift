import ArgumentParser
import Foundation

struct Serve: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Start the VM management server"
    )
    
    @Option(help: "Port to listen on")
    var port: UInt16 = 7777
    
    func run() async throws {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.serve, properties: [
            "port": port
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