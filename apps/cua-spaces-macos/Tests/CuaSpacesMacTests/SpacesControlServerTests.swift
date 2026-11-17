// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The loopback control server the MCP `open_space_viewer` tool reaches: a
/// real server on a real port, driven the way `ControlServerDisplay` drives it.
@Suite("Spaces control server", .serialized)
struct SpacesControlServerTests {
    /// What the actions were asked to do.
    final class Opened: @unchecked Sendable {
        private let lock = NSLock()
        private var ids: [String] = []
        func add(_ id: String) { lock.withLock { ids.append(id) } }
        var all: [String] { lock.withLock { ids } }
    }

    private func makeServer(token: String = "tok") throws -> (SpacesControlServer, URL, Opened) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-control-\(UUID().uuidString)")
        let file = dir.appendingPathComponent("spaces-control.json")
        let opened = Opened()
        let server = SpacesControlServer(file: file, actions: .init(
            resolve: { $0 == "local:box" || $0 == "box" ? "local:box" : nil },
            openViewer: { opened.add($0) }), token: token)
        server.start()
        return (server, file, opened)
    }

    private func waitForFile(_ file: URL) async throws -> (port: Int, token: String) {
        for _ in 0..<100 {
            if let data = try? Data(contentsOf: file),
               let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let port = json["port"] as? Int, let token = json["token"] as? String {
                return (port, token)
            }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw CocoaError(.fileNoSuchFile)
    }

    private func post(_ path: String, _ body: String, port: Int, token: String?) async throws -> (Int, String) {
        var request = URLRequest(url: URL(string: "http://127.0.0.1:\(port)\(path)")!)
        request.httpMethod = "POST"
        request.httpBody = Data(body.utf8)
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        let (data, response) = try await URLSession(configuration: .ephemeral).data(for: request)
        return ((response as! HTTPURLResponse).statusCode, String(decoding: data, as: UTF8.self))
    }

    @Test func writesItsPortAndBearerPrivatelyAndOpensTheViewer() async throws {
        let (server, file, opened) = try makeServer()
        defer { server.stop() }
        let (port, token) = try await waitForFile(file)
        #expect(token == "tok")
        let mode = try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as? Int
        #expect(mode == 0o600)

        let (status, body) = try await post("/viewer/open", #"{"space_id":"box"}"#, port: port, token: token)
        #expect(status == 200)
        #expect(body == #"{"ok":true}"#)
        #expect(opened.all == ["local:box"])
    }

    @Test func refusesARequestWithoutTheBearer() async throws {
        let (server, file, opened) = try makeServer()
        defer { server.stop() }
        let (port, _) = try await waitForFile(file)
        let (none, _) = try await post("/viewer/open", #"{"space_id":"box"}"#, port: port, token: nil)
        let (wrong, _) = try await post("/viewer/open", #"{"space_id":"box"}"#, port: port, token: "nope")
        #expect(none == 401)
        #expect(wrong == 401)
        #expect(opened.all.isEmpty)
    }

    @Test func answersTheRoutesItCannotServe() async throws {
        let (server, file, opened) = try makeServer()
        defer { server.stop() }
        let (port, token) = try await waitForFile(file)
        let (unknown, _) = try await post("/viewer/open", #"{"space_id":"nobody"}"#, port: port, token: token)
        let (missing, _) = try await post("/viewer/open", "{}", port: port, token: token)
        let (pip, _) = try await post("/pip/pin", #"{"space_id":"box"}"#, port: port, token: token)
        let (elsewhere, _) = try await post("/nope", "{}", port: port, token: token)
        #expect(unknown == 404)
        #expect(missing == 400)
        #expect(pip == 501)
        #expect(elsewhere == 404)
        #expect(opened.all.isEmpty)
    }

    @Test func stoppingRemovesTheFileSoAClientSaysTheAppIsNotRunning() async throws {
        let (server, file, _) = try makeServer()
        _ = try await waitForFile(file)
        server.stop()
        #expect(!FileManager.default.fileExists(atPath: file.path))
    }

    @Test func theControlFileLivesInTheCuaHomeOfTheChannel() {
        let home = "/Users/ada"
        #expect(SpacesControlServer.controlFile(info: nil, environment: [:], home: home)?.path == "/Users/ada/.cua/spaces-control.json")
        #expect(SpacesControlServer.controlFile(info: ["CuaHome": "~/.cua-local"], environment: [:], home: home)?.path
            == "/Users/ada/.cua-local/spaces-control.json")
        #expect(SpacesControlServer.controlFile(info: nil, environment: ["CUA_HOME": "/tmp/h"], home: home)?.path == "/tmp/h/spaces-control.json")
        // A bundle of another channel with no usable home never writes the released app's file.
        #expect(SpacesControlServer.controlFile(info: ["CuaChannel": "local"], environment: [:], home: home) == nil)
    }
}
