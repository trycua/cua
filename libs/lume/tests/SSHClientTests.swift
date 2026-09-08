@preconcurrency import NIOCore
@preconcurrency import NIOConcurrencyHelpers
@preconcurrency import NIOEmbedded
@preconcurrency import NIOPosix
@preconcurrency import NIOSSH
import Testing

@testable import lume

private enum SSHClientFutureTestError: Error {
    case handlerLookupFailed
}

@Test("SSH client teardown is safe on a NIO event loop")
func clientTeardownIsSafeOnEventLoop() async throws {
    let client = NIOLockedValueBox<SSHClient?>(SSHClient(host: "127.0.0.1"))

    try await MultiThreadedEventLoopGroup.singleton.next().submit {
        client.withLockedValue { $0 = nil }
    }.get()

    #expect(client.withLockedValue { $0 == nil })
}

@Test("SSH child promise is not created when handler lookup fails")
func childPromiseIsCreatedAfterHandlerLookup() throws {
    let eventLoop = EmbeddedEventLoop()
    let handlerFuture = eventLoop.makeFailedFuture(
        SSHClientFutureTestError.handlerLookupFailed
    ) as EventLoopFuture<NIOSSHHandler>
    let initializerCalled = NIOLockedValueBox(false)

    let childFuture = SSHClient.makeChildChannelFuture(
        handlerFuture: handlerFuture,
        eventLoop: eventLoop
    ) { _, promise in
        initializerCalled.withLockedValue { $0 = true }
        promise.fail(SSHClientFutureTestError.handlerLookupFailed)
    }

    #expect(throws: SSHClientFutureTestError.self) {
        try childFuture.wait()
    }
    #expect(!initializerCalled.withLockedValue { $0 })
}
