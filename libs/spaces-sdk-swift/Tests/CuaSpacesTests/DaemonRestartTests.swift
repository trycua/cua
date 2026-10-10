import Foundation
import Testing
@testable import CuaSpaces

/// A `cua daemon` started again listens on a new loopback port with a new
/// token, so what a connection cached from the old one (Space handles with
/// their env passthrough, stream endpoints) must not outlive it: a viewer's
/// Try again and a reopened viewer reach the new daemon.
@Suite(.serialized) final class DaemonRestartTests {

    /// The daemon behind the connection, as the test sets it.
    final class Daemon: @unchecked Sendable {
        private let lock = NSLock()
        private var current: String?
        init(_ identity: String?) { current = identity }
        var identity: String? {
            get { lock.withLock { current } }
            set { lock.withLock { current = newValue } }
        }
    }

    private let backend: FakeSpacesBackend
    private let daemon: Daemon
    private let connection: SpacesConnection

    init() {
        let backend = FakeSpacesBackend(), daemon = Daemon("100 http://127.0.0.1:50001")
        self.backend = backend
        self.daemon = daemon
        connection = SpacesConnection(transport: backend, daemonIdentity: { daemon.identity })
    }

    @Test func testEndpointCacheOutlivesNothingButItsDaemon() async throws {
        let space = try await connection.attach(to: "local:cua-space-test")
        _ = try await space.streamEndpoint()
        _ = try await space.streamEndpoint()
        #expect(await backend.callCount("stream_endpoint") == 1, "same daemon: cached")

        // The supervisor started it again: new pid, new port.
        daemon.identity = "200 http://127.0.0.1:50002"
        _ = try await space.streamEndpoint()
        #expect(await backend.callCount("stream_endpoint") == 2, "a new daemon: minted again")
        _ = try await space.streamEndpoint()
        #expect(await backend.callCount("stream_endpoint") == 2, "and cached for that daemon")
    }

    @Test func testADaemonThatDoesNotAnswerKeepsNothing() async throws {
        let space = try await connection.attach(to: "local:cua-space-test")
        _ = try await space.streamEndpoint()
        daemon.identity = nil
        _ = try await space.streamEndpoint()
        _ = try await space.streamEndpoint()
        #expect(await backend.callCount("stream_endpoint") == 3,
                "nothing cached while the daemon is down")
        daemon.identity = "300 http://127.0.0.1:50003"
        _ = try await space.streamEndpoint()
        _ = try await space.streamEndpoint()
        #expect(await backend.callCount("stream_endpoint") == 4)
    }

    @Test func testWithoutADaemonTheCacheIsKept() async throws {
        let connection = SpacesConnection(transport: backend)
        let space = try await connection.attach(to: "local:cua-space-test")
        _ = try await space.streamEndpoint()
        _ = try await space.streamEndpoint()
        #expect(await backend.callCount("stream_endpoint") == 1)
    }
}
