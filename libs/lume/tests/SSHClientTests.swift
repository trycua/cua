@preconcurrency import NIOCore
@preconcurrency import NIOConcurrencyHelpers
@preconcurrency import NIOEmbedded
@preconcurrency import NIOSSH
import Testing

@testable import lume

private enum SSHClientFutureTestError: Error {
    case handlerLookupFailed
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
