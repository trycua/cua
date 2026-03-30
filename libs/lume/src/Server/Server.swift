import Foundation
import NIOCore
import NIOPosix
import NIOHTTP1

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

// MARK: - NIO Channel Handler

/// Accumulates HTTP/1.1 request parts delivered by NIOHTTP1's decoder (which
/// already handles Content-Length / chunked-encoding reassembly), then
/// dispatches the complete request to the Server's route handlers.
private final class HTTPChannelHandler: ChannelInboundHandler, @unchecked Sendable {
    typealias InboundIn = HTTPServerRequestPart
    typealias OutboundOut = HTTPServerResponsePart

    private var requestHead: HTTPRequestHead?
    private var bodyBuffer: ByteBuffer = ByteBuffer()
    private let server: Server

    init(server: Server) {
        self.server = server
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        switch unwrapInboundIn(data) {
        case .head(let head):
            requestHead = head
            bodyBuffer.clear()
        case .body(var buf):
            bodyBuffer.writeBuffer(&buf)
        case .end:
            guard let head = requestHead else { return }
            let bodyData: Data? =
                bodyBuffer.readableBytes > 0 ? Data(bodyBuffer.readableBytesView) : nil
            var headers: [String: String] = [:]
            for (name, value) in head.headers {
                headers[name.description] = value
            }
            let request = HTTPRequest(
                method: head.method.rawValue,
                path: head.uri,
                headers: headers,
                body: bodyData
            )
            Logger.info(
                "Received request",
                metadata: [
                    "method": request.method,
                    "path": request.path,
                    "body": String(data: bodyData ?? Data(), encoding: .utf8) ?? "",
                ])

            // Bridge to Swift concurrency via an EventLoopPromise so that
            // ChannelHandlerContext (non-Sendable) is only ever accessed on
            // the event loop — never sent across actor boundaries.
            let promise = context.eventLoop.makePromise(of: HTTPResponse.self)
            let srv = server
            Task {
                do {
                    let response = try await srv.handleRequest(request)
                    promise.succeed(response)
                } catch {
                    promise.succeed(srv.errorResponse(error))
                }
            }
            promise.futureResult.whenComplete { result in
                let response: HTTPResponse
                switch result {
                case .success(let r): response = r
                case .failure(let e): response = srv.errorResponse(e)
                }
                HTTPChannelHandler.writeResponse(response, to: context)
            }
        }
    }

    private static func writeResponse(_ response: HTTPResponse, to context: ChannelHandlerContext) {
        var nioHeaders = HTTPHeaders()
        for (k, v) in response.headers {
            nioHeaders.add(name: k, value: v)
        }
        if let body = response.body {
            nioHeaders.replaceOrAdd(name: "Content-Length", value: "\(body.count)")
        }
        let status = HTTPResponseStatus(statusCode: response.statusCode.rawValue)
        let head = HTTPResponseHead(version: .http1_1, status: status, headers: nioHeaders)

        Logger.info(
            "Sending response",
            metadata: [
                "statusCode": "\(response.statusCode.rawValue)",
                "body": String(data: response.body ?? Data(), encoding: .utf8) ?? "",
            ])

        context.eventLoop.execute {
            context.write(NIOAny(HTTPServerResponsePart.head(head)), promise: nil)
            if let body = response.body {
                var buf = context.channel.allocator.buffer(capacity: body.count)
                buf.writeBytes(body)
                context.write(NIOAny(HTTPServerResponsePart.body(.byteBuffer(buf))), promise: nil)
            }
            context.writeAndFlush(NIOAny(HTTPServerResponsePart.end(nil)), promise: nil)
        }
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        Logger.error("Channel error", metadata: ["error": error.localizedDescription])
        context.close(promise: nil)
    }
}

// MARK: - Server Class

final class Server: @unchecked Sendable {

    // MARK: - Route Type

    private struct Route {
        let method: String
        let path: String
        let handler: (HTTPRequest) async throws -> HTTPResponse

        func matches(_ request: HTTPRequest) -> Bool {
            if method != request.method { return false }

            let routeParts = path.split(separator: "/")
            let requestParts = request.path.split(separator: "/")

            if routeParts.count != requestParts.count { return false }

            for (routePart, requestPart) in zip(routeParts, requestParts) {
                if routePart.hasPrefix(":") { continue }
                if routePart != requestPart { return false }
            }

            return true
        }

        func extractParams(_ request: HTTPRequest) -> [String: String] {
            var params: [String: String] = [:]
            let routeParts = path.split(separator: "/")
            let requestPathOnly = request.path.split(separator: "?", maxSplits: 1)[0]
            let requestParts = requestPathOnly.split(separator: "/")

            for (routePart, requestPart) in zip(routeParts, requestParts) {
                if routePart.hasPrefix(":") {
                    params[String(routePart.dropFirst())] = String(requestPart)
                }
            }
            return params
        }
    }

    // MARK: - Properties

    private let portNumber: UInt16
    private let controller: LumeController
    private var routes: [Route]
    // _channelLock guards both _serverChannel and _eventLoopGroup, which are
    // written in start() and read in stop() — potentially from different tasks.
    private let _channelLock = NSLock()
    private var _serverChannel: (any Channel)?
    private var _eventLoopGroup: (any EventLoopGroup)?

    private var serverChannel: (any Channel)? {
        get { _channelLock.withLock { _serverChannel } }
        set { _channelLock.withLock { _serverChannel = newValue } }
    }
    private var eventLoopGroup: (any EventLoopGroup)? {
        get { _channelLock.withLock { _eventLoopGroup } }
        set { _channelLock.withLock { _eventLoopGroup = newValue } }
    }

    // MARK: - Initialization

    init(port: UInt16 = 7777) {
        self.portNumber = port
        self.controller = LumeController()
        self.routes = []
        self.setupRoutes()
    }

    // MARK: - Route Setup

    private func setupRoutes() {
        routes = [
            Route(
                method: "GET", path: "/lume/vms",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let storage = self.extractQueryParam(request: request, name: "storage")
                    return try await self.handleListVMs(storage: storage)
                }),
            Route(
                method: "GET", path: "/lume/vms/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = self.extractPathParams(pattern: "/lume/vms/:name", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    let storage = self.extractQueryParam(request: request, name: "storage")
                    return try await self.handleGetVM(name: name, storage: storage)
                }),
            Route(
                method: "DELETE", path: "/lume/vms/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = self.extractPathParams(pattern: "/lume/vms/:name", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
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
                    let params = self.extractPathParams(pattern: "/lume/vms/:name", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    return try await self.handleSetVM(name: name, body: request.body)
                }),
            Route(
                method: "POST", path: "/lume/vms/:name/run",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = self.extractPathParams(pattern: "/lume/vms/:name/run", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    return try await self.handleRunVM(name: name, body: request.body)
                }),
            Route(
                method: "POST", path: "/lume/vms/:name/stop",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = self.extractPathParams(
                        pattern: "/lume/vms/:name/stop", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    Logger.info(
                        "Processing stop VM request",
                        metadata: ["method": request.method, "path": request.path])
                    var storage: String? = nil
                    if let bodyData = request.body, !bodyData.isEmpty {
                        do {
                            if let json = try JSONSerialization.jsonObject(with: bodyData)
                                as? [String: Any],
                                let bodyStorage = json["storage"] as? String
                            {
                                storage = bodyStorage
                            }
                        } catch {}
                    }
                    return try await self.handleStopVM(name: name, storage: storage)
                }),
            Route(
                method: "POST", path: "/lume/vms/:name/setup",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = self.extractPathParams(
                        pattern: "/lume/vms/:name/setup", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing VM name")
                    }
                    return try await self.handleSetupVM(name: name, body: request.body)
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
                method: "POST", path: "/lume/pull/start",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handlePullStart(request.body)
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
                    let params = self.extractPathParams(
                        pattern: "/lume/config/locations/:name", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing location name")
                    }
                    return try await self.handleRemoveLocation(name)
                }),
            Route(
                method: "GET", path: "/lume/logs",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let type = self.extractQueryParam(request: request, name: "type")
                    let linesParam = self.extractQueryParam(request: request, name: "lines")
                    let lines = linesParam.flatMap { Int($0) }
                    return try await self.handleGetLogs(type: type, lines: lines)
                }),
            Route(
                method: "POST", path: "/lume/config/locations/default/:name",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    let params = self.extractPathParams(
                        pattern: "/lume/config/locations/default/:name", from: request)
                    guard let name = params["name"] else {
                        return HTTPResponse(statusCode: .badRequest, body: "Missing location name")
                    }
                    return try await self.handleSetDefaultLocation(name)
                }),
            Route(
                method: "POST", path: "/lume/vms/push",
                handler: { [weak self] request in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handlePush(request.body)
                }),
            Route(
                method: "GET", path: "/lume/host/status",
                handler: { [weak self] _ in
                    guard let self else { throw HTTPError.internalError }
                    return try await self.handleGetHostStatus()
                }),
        ]
    }

    // MARK: - Helpers

    private func extractQueryParam(request: HTTPRequest, name: String) -> String? {
        let parts = request.path.split(separator: "?", maxSplits: 1)
        guard parts.count > 1 else { return nil }
        let queryString = String(parts[1])
        if let urlComponents = URLComponents(string: "http://placeholder.com?" + queryString),
            let queryItems = urlComponents.queryItems
        {
            return queryItems.first(where: { $0.name == name })?.value?.removingPercentEncoding
        }
        return nil
    }

    private func extractPathParams(pattern: String, from request: HTTPRequest) -> [String: String] {
        var params: [String: String] = [:]
        let routeParts = pattern.split(separator: "/")
        let requestPathOnly = request.path.split(separator: "?", maxSplits: 1)[0]
        let requestParts = requestPathOnly.split(separator: "/")
        for (routePart, requestPart) in zip(routeParts, requestParts) {
            if routePart.hasPrefix(":") {
                params[String(routePart.dropFirst())] = String(requestPart)
            }
        }
        return params
    }

    // MARK: - Server Lifecycle

    func start() async throws {
        let group = MultiThreadedEventLoopGroup(numberOfThreads: System.coreCount)
        eventLoopGroup = group
        let srv = self

        let bootstrap = ServerBootstrap(group: group)
            .serverChannelOption(ChannelOptions.backlog, value: 256)
            .serverChannelOption(ChannelOptions.socketOption(.so_reuseaddr), value: 1)
            .childChannelInitializer { channel in
                channel.pipeline.configureHTTPServerPipeline().flatMap {
                    channel.pipeline.addHandler(HTTPChannelHandler(server: srv))
                }
            }

        do {
            let channel = try await bootstrap.bind(host: "127.0.0.1", port: Int(portNumber)).get()
            serverChannel = channel
            Logger.info("Server started", metadata: ["port": "\(portNumber)"])
            try await channel.closeFuture.get()
        } catch let ioError as IOError where ioError.errnoCode == EADDRINUSE {
            try? await group.shutdownGracefully()
            throw PortError.alreadyInUse(port: portNumber)
        } catch {
            try? await group.shutdownGracefully()
            throw error
        }
        try? await group.shutdownGracefully()
    }

    func stop() {
        serverChannel?.close(promise: nil)
        if let group = eventLoopGroup {
            Task { try? await group.shutdownGracefully() }
        }
    }

    // MARK: - Request Handling

    func handleRequest(_ request: HTTPRequest) async throws -> HTTPResponse {
        Logger.info(
            "Parsed request",
            metadata: [
                "method": request.method,
                "path": request.path,
                "body": String(data: request.body ?? Data(), encoding: .utf8) ?? "",
            ])

        guard let route = routes.first(where: { $0.matches(request) }) else {
            return HTTPResponse(statusCode: .notFound, body: "Not found")
        }

        let response = try await route.handler(request)

        Logger.info(
            "Sending response",
            metadata: [
                "statusCode": "\(response.statusCode.rawValue)",
                "body": String(data: response.body ?? Data(), encoding: .utf8) ?? "",
            ])

        return response
    }

    func errorResponse(_ error: Error) -> HTTPResponse {
        HTTPResponse(
            statusCode: .internalServerError,
            headers: ["Content-Type": "application/json"],
            body: try! JSONEncoder().encode(APIError(message: error.localizedDescription))
        )
    }
}
