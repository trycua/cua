// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Network
import Security

/// The app's loopback control server: how `cua daemon mcp` (and any other
/// Spaces host using `cua_spaces::operator::ControlServerDisplay`) asks this
/// app to present a Space on the operator's desktop, such as the MCP
/// `open_space_viewer` tool.
///
/// It binds `127.0.0.1:<ephemeral>`, writes `{port, token}` to
/// `<cua home>/spaces-control.json` (mode 0600) and requires
/// `Authorization: Bearer <token>` on every request. The Tauri app serves the
/// same protocol (`apps/cua-spaces/src-tauri/src/control.rs`), and the client
/// reads the file on every call, so a restart that rotates the port and the
/// token is picked up.
///
/// Routes (every body is JSON, every reply is JSON):
///
///     POST /viewer/open  {space_id}  opens the Space's page in the main window
///
/// `/pip/pin`, `/pip/unpin` and `/window/stream` answer 501 until this app
/// presents them natively; the client reports that status instead of the
/// misleading "app isn't running".
public final class SpacesControlServer: @unchecked Sendable {
    /// What the server does for a request. Both run on the main actor.
    public struct Actions: Sendable {
        /// The canonical id of the Space a request names (any accepted
        /// spelling: the id, or the Space's name), or nil when there is none.
        public var resolve: @MainActor @Sendable (String) -> String?
        /// Shows the Space's page in the main window.
        public var openViewer: @MainActor @Sendable (String) -> Void

        public init(resolve: @escaping @MainActor @Sendable (String) -> String?,
                    openViewer: @escaping @MainActor @Sendable (String) -> Void) {
            self.resolve = resolve
            self.openViewer = openViewer
        }
    }

    /// The reply to a request: status code and JSON body.
    struct Reply: Equatable {
        var status: Int
        var body: String
    }

    /// Headers and body sizes past which a request is dropped.
    static let maxHeader = 64 * 1024
    static let maxBody = 1024 * 1024

    private let file: URL
    private let actions: Actions
    private let token: String
    private let queue = DispatchQueue(label: "com.trycua.spaces.control")
    private var listener: NWListener?

    /// `file` is `<cua home>/spaces-control.json`; `token` is the bearer
    /// (random per launch unless a test passes one).
    public init(file: URL, actions: Actions, token: String = SpacesControlServer.randomToken()) {
        self.file = file
        self.actions = actions
        self.token = token
    }

    // MARK: - Lifecycle

    /// Starts listening and writes the control file once the port is known.
    /// A failure is logged and leaves the app running without the server,
    /// as the Tauri app does: only agent-driven presentation is lost.
    public func start() {
        queue.async { [self] in
            guard listener == nil else { return }
            do {
                let parameters = NWParameters.tcp
                parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
                let listener = try NWListener(using: parameters)
                listener.stateUpdateHandler = { [weak self] state in
                    self?.queue.async { self?.listenerState(state) }
                }
                listener.newConnectionHandler = { [weak self] connection in
                    self?.accept(connection)
                }
                self.listener = listener
                listener.start(queue: queue)
            } catch {
                Self.log("control server disabled: \(error)")
            }
        }
    }

    /// Stops listening and removes the control file, so a client says the
    /// app isn't running instead of connecting to a dead port.
    public func stop() {
        queue.sync {
            listener?.cancel()
            listener = nil
            try? FileManager.default.removeItem(at: file)
        }
    }

    private func listenerState(_ state: NWListener.State) {
        switch state {
        case .ready:
            guard let port = listener?.port?.rawValue else { return }
            do {
                try Self.writeControlFile(at: file, port: port, token: token)
            } catch {
                Self.log("control server disabled: could not write \(file.path): \(error)")
                listener?.cancel()
                listener = nil
            }
        case let .failed(error):
            Self.log("control server disabled: \(error)")
            listener?.cancel()
            listener = nil
        default:
            break
        }
    }

    // MARK: - Connections

    private func accept(_ connection: NWConnection) {
        connection.start(queue: queue)
        receive(on: connection, buffered: Data())
    }

    private func receive(on connection: NWConnection, buffered: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16 * 1024) { [weak self] data, _, isComplete, error in
            guard let self else { return connection.cancel() }
            var buffer = buffered
            if let data { buffer.append(data) }
            switch Self.parse(buffer) {
            case let .complete(request):
                Task {
                    let reply = await self.route(request)
                    self.send(reply, on: connection)
                }
            case .incomplete:
                if error != nil || isComplete {
                    connection.cancel()
                } else {
                    self.receive(on: connection, buffered: buffer)
                }
            case let .invalid(reason):
                self.send(Reply(status: 400, body: Self.errorJSON(reason)), on: connection)
            }
        }
    }

    private func send(_ reply: Reply, on connection: NWConnection) {
        let body = Data(reply.body.utf8)
        var response = Data("HTTP/1.1 \(reply.status) \(Self.reason(reply.status))\r\nContent-Type: application/json\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n".utf8)
        response.append(body)
        connection.send(content: response, completion: .contentProcessed { _ in connection.cancel() })
    }

    // MARK: - Routing

    func route(_ request: Request) async -> Reply {
        guard Self.bearerMatches(request.authorization, token: token) else {
            return Reply(status: 401, body: Self.errorJSON("unauthorized"))
        }
        switch (request.method, request.path) {
        case ("POST", "/viewer/open"):
            guard let requested = Self.field(request.body, "space_id") else {
                return Reply(status: 400, body: Self.errorJSON("space_id required"))
            }
            let actions = self.actions
            return await MainActor.run {
                guard let id = actions.resolve(requested) else {
                    return Reply(status: 404, body: Self.errorJSON("no Space \(requested)"))
                }
                actions.openViewer(id)
                return Reply(status: 200, body: "{\"ok\":true}")
            }
        case ("POST", "/pip/pin"), ("POST", "/pip/unpin"), ("POST", "/window/stream"):
            return Reply(status: 501, body: Self.errorJSON("\(request.path) is not available in the macOS app yet"))
        default:
            return Reply(status: 404, body: Self.errorJSON("not found"))
        }
    }

    // MARK: - HTTP

    /// A parsed HTTP/1.1 request.
    struct Request: Equatable {
        var method: String
        var path: String
        var authorization: String?
        var body: String
    }

    enum Parsed: Equatable {
        case complete(Request)
        case incomplete
        case invalid(String)
    }

    /// Reads one request from what has arrived so far.
    static func parse(_ data: Data) -> Parsed {
        guard let end = data.range(of: Data("\r\n\r\n".utf8)) else {
            return data.count > maxHeader ? .invalid("request headers too large") : .incomplete
        }
        let head = String(decoding: data[data.startIndex..<end.lowerBound], as: UTF8.self)
        var lines = head.components(separatedBy: "\r\n")
        let requestLine = lines.removeFirst().split(separator: " ", omittingEmptySubsequences: true)
        guard requestLine.count >= 2 else { return .invalid("bad request line") }
        var contentLength = 0
        var authorization: String?
        for line in lines {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = line[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            if name == "content-length" {
                contentLength = Int(value) ?? 0
            } else if name == "authorization" {
                authorization = value
            }
        }
        guard contentLength >= 0, contentLength <= maxBody else { return .invalid("request body too large") }
        let body = data[end.upperBound...]
        guard body.count >= contentLength else { return .incomplete }
        return .complete(Request(
            method: String(requestLine[0]),
            path: String(requestLine[1]),
            authorization: authorization,
            body: String(decoding: body.prefix(contentLength), as: UTF8.self)))
    }

    static func reason(_ status: Int) -> String {
        switch status {
        case 200: "OK"
        case 400: "Bad Request"
        case 401: "Unauthorized"
        case 404: "Not Found"
        case 501: "Not Implemented"
        default: "Internal Server Error"
        }
    }

    static func errorJSON(_ message: String) -> String {
        let data = try? JSONSerialization.data(withJSONObject: ["error": message])
        return data.map { String(decoding: $0, as: UTF8.self) } ?? "{\"error\":\"failed\"}"
    }

    /// A string field of a JSON object body.
    static func field(_ body: String, _ key: String) -> String? {
        guard let object = try? JSONSerialization.jsonObject(with: Data(body.utf8)) as? [String: Any] else { return nil }
        return object[key] as? String
    }

    /// Whether an `Authorization` value is `Bearer <token>`, compared in
    /// constant time.
    static func bearerMatches(_ value: String?, token: String) -> Bool {
        let expected = Array("Bearer \(token)".utf8)
        let given = Array((value ?? "").utf8)
        guard given.count == expected.count else { return false }
        return zip(given, expected).reduce(UInt8(0)) { $0 | ($1.0 ^ $1.1) } == 0
    }

    // MARK: - Control file

    /// Writes `{port, token}` to `url` with owner-only permissions from
    /// creation (the token is never readable by others), by renaming a
    /// private temporary file over any earlier one.
    static func writeControlFile(at url: URL, port: UInt16, token: String) throws {
        let fileManager = FileManager.default
        try fileManager.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let json = try JSONSerialization.data(withJSONObject: ["port": Int(port), "token": token])
        let temporary = url.deletingLastPathComponent().appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString)")
        guard fileManager.createFile(atPath: temporary.path, contents: json + Data("\n".utf8),
                                     attributes: [.posixPermissions: 0o600]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        do {
            _ = try fileManager.replaceItemAt(url, withItemAt: temporary)
        } catch {
            try? fileManager.removeItem(at: temporary)
            throw error
        }
    }

    /// The control file of the cua home this app runs in: `CUA_HOME`, else
    /// the bundle's `CuaHome` (Cua Spaces Local), else `~/.cua`. `nil` for a
    /// bundle of another channel with no usable home, which must never write
    /// the released app's file.
    public static func controlFile(info: [String: Any]?, environment: [String: String], home: String) -> URL? {
        let base: String
        if let override = environment["CUA_HOME"], !override.isEmpty {
            base = override
        } else if let stamped = HelperService.homeDirectory(info: info, home: home) {
            base = stamped
        } else {
            return nil
        }
        return URL(fileURLWithPath: base, isDirectory: true).appendingPathComponent("spaces-control.json")
    }

    /// 32 random bytes, hex.
    public static func randomToken() -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
            return UUID().uuidString + UUID().uuidString
        }
        return bytes.map { String(format: "%02x", $0) }.joined()
    }

    private static func log(_ message: String) {
        FileHandle.standardError.write(Data("[cua-spaces] \(message)\n".utf8))
    }
}
