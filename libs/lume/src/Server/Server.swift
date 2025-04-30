import Darwin
import Foundation
import Network

// MARK: - Error Types
enum PortError: Error, LocalizedError {
    case alreadyInUse(port: UInt16)

    var errorDescription: String? {
        switch self {
        case .alreadyInUse(let port):
            return "Port \(port) is already in use by another process"
        }
    }
}

// MARK: - Server Class
@MainActor
final class Server {

    // MARK: - Route Type
    private struct Route {
        let method: String
        let path: String
        let handler: (HTTPRequest) async throws -> HTTPResponse

        func matches(_ request: HTTPRequest) -> Bool {
            if method != request.method { return false }

            // Handle path parameters
            let routeParts = path.split(separator: "/")
            let requestParts = request.path.split(separator: "/")

            if routeParts.count != requestParts.count { return false }

            for (routePart, requestPart) in zip(routeParts, requestParts) {
                if routePart.hasPrefix(":") { continue }  // Path parameter
                if routePart != requestPart { return false }
            }

            return true
        }

        func extractParams(_ request: HTTPRequest) -> [String: String] {
            var params: [String: String] = [:]
            let routeParts = path.split(separator: "/")
            let requestParts = request.path.split(separator: "/")

            for (routePart, requestPart) in zip(routeParts, requestParts) {
                if routePart.hasPrefix(":") {
                    let paramName = String(routePart.dropFirst())
                    params[paramName] = String(requestPart)
                }
            }

            return params
        }
    }

    // MARK: - Properties
    private let port: NWEndpoint.Port
    private let controller: LumeController
    private var isRunning = false
    private var listener: NWListener?
    private var routes: [Route]

    // MARK: - Initialization
    init(port: UInt16 = 3000) {
        self.port = NWEndpoint.Port(rawValue: port)!
        self.controller = LumeController()
        self.routes = []

        // Define API routes after self is fully initialized
        self.setupRoutes()
    }

    // MARK: - Route Setup
    private func setupRoutes() {
        routes = [
            Route(
                method: "GET", path: "/lume/vms",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    // Extract storage from query params if present
                    let storage = self.extractQueryParam(request: request, name: "storage")
                    return try await self.handleListVMs(storage: storage)
                }),
            Route(
                method: "GET", path: "/lume/vms/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "GET", path: "/lume/vms/:name",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }

                    // Extract storage from query params if present
                    let storage = self.extractQueryParam(request: request, name: "storage")

                    return try await self.handleGetVM(name: name, storage: storage)
                }),
            Route(
                method: "DELETE", path: "/lume/vms/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "DELETE", path: "/lume/vms/:name",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }

                    // Extract storage from query params if present
                    let storage = self.extractQueryParam(request: request, name: "storage")

                    return try await self.handleDeleteVM(name: name, storage: storage)
                }),
            Route(
                method: "POST", path: "/lume/vms",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleCreateVM(request.body)
                }),
            Route(
                method: "POST", path: "/lume/vms/clone",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleCloneVM(request.body)
                }),
            Route(
                method: "PATCH", path: "/lume/vms/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "PATCH", path: "/lume/vms/:name",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    return try await self.handleSetVM(name: name, body: request.body)
                }),
            Route(
                method: "POST", path: "/lume/vms/:name/run",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "POST", path: "/lume/vms/:name/run",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    return try await self.handleRunVM(name: name, body: request.body)
                }),
            Route(
                method: "POST", path: "/lume/vms/:name/stop",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "POST", path: "/lume/vms/:name/stop",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }

                    // Extract storage from query params if present
                    let storage = self.extractQueryParam(request: request, name: "storage")

                    return try await self.handleStopVM(name: name, storage: storage)
                }),
            Route(
                method: "GET", path: "/lume/ipsw",
                handler: { [weak self] _ in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleIPSW()
                }),
            Route(
                method: "POST", path: "/lume/pull",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handlePull(request.body)
                }),
            Route(
                method: "POST", path: "/lume/prune",
                handler: { [weak self] _ in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handlePruneImages()
                }),
            Route(
                method: "GET", path: "/lume/images",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleGetImages(request)
                }),
            // New config endpoint
            Route(
                method: "GET", path: "/lume/config",
                handler: { [weak self] _ in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleGetConfig()
                }),
            Route(
                method: "POST", path: "/lume/config",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleUpdateConfig(request.body)
                }),
            Route(
                method: "GET", path: "/lume/config/locations",
                handler: { [weak self] _ in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleGetLocations()
                }),
            Route(
                method: "POST", path: "/lume/config/locations",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleAddLocation(request.body)
                }),
            Route(
                method: "DELETE", path: "/lume/config/locations/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "DELETE", path: "/lume/config/locations/:name",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing location name")
                    }
                    return try await self.handleRemoveLocation(name)
                }),
            Route(
                method: "POST", path: "/lume/config/locations/default/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = Route(
                        method: "POST", path: "/lume/config/locations/default/:name",
                        handler: { _ in
                            HTTPResponse(statusCode: .ok, body: "")
                        }
                    ).extractParams(request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing location name")
                    }
                    return try await self.handleSetDefaultLocation(name)
                }),
            Route(
                method: "POST", path: "/vms/push",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handlePush(request.body)
                }),
        ]
    }

    // Helper to extract query parameters from the URL
    private func extractQueryParam(request: HTTPRequest, name: String) -> String? {
        if let urlComponents = URLComponents(string: request.path),
            let queryItems = urlComponents.queryItems
        {
            return queryItems.first(where: { $0.name == name })?.value
        }
        return nil
    }

    // MARK: - Port Utilities
    private func isPortAvailable(port: Int) async -> Bool {
        // Create a socket
        let socketFD = socket(AF_INET, SOCK_STREAM, 0)
        if socketFD == -1 {
            return false
        }

        // Set socket options to allow reuse
        var value: Int32 = 1
        if setsockopt(
            socketFD, SOL_SOCKET, SO_REUSEADDR, &value, socklen_t(MemoryLayout<Int32>.size)) == -1
        {
            close(socketFD)
            return false
        }

        // Set up the address structure
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = UInt16(port).bigEndian
        addr.sin_addr.s_addr = INADDR_ANY.bigEndian

        // Bind to the port
        let bindResult = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { addrPtr in
                Darwin.bind(socketFD, addrPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }

        // Clean up
        close(socketFD)

        // If bind failed, the port is in use
        return bindResult == 0
    }

    // MARK: - Server Lifecycle
    func start() async throws {
        // First check if the port is already in use
        if !(await isPortAvailable(port: Int(port.rawValue))) {
            // Don't log anything here, just throw the error
            throw PortError.alreadyInUse(port: port.rawValue)
        }

        let parameters = NWParameters.tcp
        listener = try NWListener(using: parameters, on: port)

        // Create an actor to safely manage state transitions
        actor StartupState {
            var error: Error?
            var isComplete = false

            func setError(_ error: Error) {
                self.error = error
                self.isComplete = true
            }

            func setComplete() {
                self.isComplete = true
            }

            func checkStatus() -> (isComplete: Bool, error: Error?) {
                return (isComplete, error)
            }
        }

        let startupState = StartupState()

        // Set up a state update handler to detect port binding errors
        listener?.stateUpdateHandler = { state in
            Task {
                switch state {
                case .setup:
                    // Initial state, no action needed
                    Logger.info("Listener setup", metadata: ["port": "\(self.port.rawValue)"])
                    break
                case .waiting(let error):
                    // Log the full error details to see what we're getting
                    Logger.error(
                        "Listener waiting",
                        metadata: [
                            "error": error.localizedDescription,
                            "debugDescription": error.debugDescription,
                            "localizedDescription": error.localizedDescription,
                            "port": "\(self.port.rawValue)",
                        ])

                    // Check for different port in use error messages
                    if error.debugDescription.contains("Address already in use")
                        || error.localizedDescription.contains("in use")
                        || error.localizedDescription.contains("address already in use")
                    {
                        Logger.error(
                            "Port conflict detected", metadata: ["port": "\(self.port.rawValue)"])
                        await startupState.setError(
                            PortError.alreadyInUse(port: self.port.rawValue))
                    } else {
                        // Wait for a short period to see if the listener recovers
                        // Some network errors are transient
                        try? await Task.sleep(nanoseconds: 1_000_000_000)  // 1 second

                        // If we're still waiting after delay, consider it an error
                        if case .waiting = await self.listener?.state {
                            await startupState.setError(error)
                        }
                    }
                case .failed(let error):
                    // Log the full error details
                    Logger.error(
                        "Listener failed",
                        metadata: [
                            "error": error.localizedDescription,
                            "debugDescription": error.debugDescription,
                            "port": "\(self.port.rawValue)",
                        ])
                    await startupState.setError(error)
                case .ready:
                    // Listener successfully bound to port
                    Logger.info("Listener ready", metadata: ["port": "\(self.port.rawValue)"])
                    await startupState.setComplete()
                case .cancelled:
                    // Listener was cancelled
                    Logger.info("Listener cancelled", metadata: ["port": "\(self.port.rawValue)"])
                    break
                @unknown default:
                    Logger.info(
                        "Unknown listener state",
                        metadata: ["state": "\(state)", "port": "\(self.port.rawValue)"])
                    break
                }
            }
        }

        listener?.newConnectionHandler = { [weak self] connection in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.handleConnection(connection)
            }
        }

        listener?.start(queue: .main)

        // Wait for either successful startup or an error
        var status: (isComplete: Bool, error: Error?) = (false, nil)
        repeat {
            try await Task.sleep(nanoseconds: 100_000_000)  // 100ms
            status = await startupState.checkStatus()
        } while !status.isComplete

        // If there was a startup error, throw it
        if let error = status.error {
            self.stop()
            throw error
        }

        isRunning = true

        Logger.info("Server started", metadata: ["port": "\(port.rawValue)"])

        // Keep the server running
        while isRunning {
            try await Task.sleep(nanoseconds: 1_000_000_000)
        }
    }

    func stop() {
        isRunning = false
        listener?.cancel()
    }

    // MARK: - Connection Handling
    private func handleConnection(_ connection: NWConnection) {
        connection.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.receiveData(connection)
                }
            case .failed(let error):
                Logger.error("Connection failed", metadata: ["error": error.localizedDescription])
                connection.cancel()
            case .cancelled:
                // Connection is already cancelled, no need to cancel again
                break
            default:
                break
            }
        }
        connection.start(queue: .main)
    }

    private func receiveData(_ connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) {
            [weak self] content, _, isComplete, error in
            if let error = error {
                Logger.error("Receive error", metadata: ["error": error.localizedDescription])
                connection.cancel()
                return
            }

            guard let data = content, !data.isEmpty else {
                if isComplete {
                    connection.cancel()
                }
                return
            }

            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let response = try await self.handleRequest(data)
                    self.send(response, on: connection)
                } catch {
                    let errorResponse = self.errorResponse(error)
                    self.send(errorResponse, on: connection)
                }
            }
        }
    }

    private func send(_ response: HTTPResponse, on connection: NWConnection) {
        let data = response.serialize()
        Logger.info(
            "Serialized response", metadata: ["data": String(data: data, encoding: .utf8) ?? ""])
        connection.send(
            content: data,
            completion: .contentProcessed { [weak connection] error in
                if let error = error {
                    Logger.error(
                        "Failed to send response", metadata: ["error": error.localizedDescription])
                } else {
                    Logger.info("Response sent successfully")
                }
                if connection?.state != .cancelled {
                    connection?.cancel()
                }
            })
    }

    // MARK: - Request Handling
    private func handleRequest(_ data: Data) async throws -> HTTPResponse {
        Logger.info(
            "Received request data", metadata: ["data": String(data: data, encoding: .utf8) ?? ""])

        guard let request = HTTPRequest(data: data) else {
            Logger.error("Failed to parse request")
            return HTTPResponse(statusCode: .badRequest, body: "Invalid request")
        }

        Logger.info(
            "Parsed request",
            metadata: [
                "method": request.method,
                "path": request.path,
                "headers": "\(request.headers)",
                "body": String(data: request.body ?? Data(), encoding: .utf8) ?? "",
            ])

        // Find matching route
        guard let route = routes.first(where: { $0.matches(request) }) else {
            return HTTPResponse(statusCode: .notFound, body: "Not found")
        }

        // Handle the request
        let response = try await route.handler(request)

        Logger.info(
            "Sending response",
            metadata: [
                "statusCode": "\(response.statusCode.rawValue)",
                "headers": "\(response.headers)",
                "body": String(data: response.body ?? Data(), encoding: .utf8) ?? "",
            ])

        return response
    }

    private func errorResponse(_ error: Error) -> HTTPResponse {
        HTTPResponse(
            statusCode: .internalServerError,
            headers: ["Content-Type": "application/json"],
            body: try! JSONEncoder().encode(APIError(message: error.localizedDescription))
        )
    }
}
