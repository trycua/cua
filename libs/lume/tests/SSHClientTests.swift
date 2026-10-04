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

@Test("SSH command escaping preserves spaces, empty strings, and special characters")
func sshCommandEscaping() {
    // Basic tokens that require no escaping
    #expect(SSH.shellEscape("echo") == "echo")
    #expect(SSH.shellEscape("arg_123-45.txt") == "arg_123-45.txt")

    // Empty string
    #expect(SSH.shellEscape("") == "''")

    // Arguments with spaces
    #expect(SSH.shellEscape("two words") == "'two words'")

    // Arguments with single quotes
    #expect(SSH.shellEscape("don't") == "'don'\\''t'")

    // Multi-token commands formatting
    let command = ["/usr/bin/test", "two words", "=", "two words"]
    let formatted = SSH.formatRemoteCommand(command)
    #expect(formatted == "/usr/bin/test 'two words' = 'two words'")

    // bash -c script and positional args
    let bashCmd = ["/bin/bash", "-c", "test \"$1\" = \"two words\"", "argv0", "two words"]
    let formattedBash = SSH.formatRemoteCommand(bashCmd)
    #expect(formattedBash == "/bin/bash -c 'test \"$1\" = \"two words\"' argv0 'two words'")
}

