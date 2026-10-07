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

@Test("A single lume ssh command argument reaches the remote shell unchanged")
func sshSingleArgumentPassthrough() throws {
    #expect(SSH.formatRemoteCommand(["ls -la"]) == "ls -la")
    #expect(SSH.formatRemoteCommand(["cd /app && npm test"]) == "cd /app && npm test")

    // The shape cua-vmm's LumeSshExec sends: one argument holding
    // `/bin/sh -c '<wrapper>'`, already quoted for the remote shell.
    let wrapper = """
        e=$(mktemp -t cua-exec) || exit 125; ( /bin/sh -c 'echo hi'\\''s' ) 2>"$e"; rc=$?; \
        printf '\\036CUA-EXEC-1\\036%d\\036' "$rc"; base64 < "$e"; rm -f "$e"; exit "$rc"
        """
    let quoted = "'" + wrapper.replacingOccurrences(of: "'", with: "'\\''") + "'"
    let cuaVmmCommand = "/bin/sh -c \(quoted)"
    #expect(SSH.formatRemoteCommand([cuaVmmCommand]) == cuaVmmCommand)

    let parsed = try SSH.parse(["guest", "--timeout", "0", cuaVmmCommand])
    #expect(SSH.formatRemoteCommand(parsed.command) == cuaVmmCommand)
}

@Test("Several lume ssh command arguments keep their boundaries (#3879)")
func sshMultipleArgumentsKeepBoundaries() throws {
    let testArgs = try SSH.parse(["guest", "--", "/usr/bin/test", "two words", "=", "two words"])
    #expect(
        SSH.formatRemoteCommand(testArgs.command)
            == "/usr/bin/test 'two words' = 'two words'")

    let bashArgs = try SSH.parse([
        "guest", "--", "/bin/bash", "-c", "test \"$1\" = \"two words\"", "argv0", "two words",
    ])
    #expect(
        SSH.formatRemoteCommand(bashArgs.command)
            == "/bin/bash -c 'test \"$1\" = \"two words\"' argv0 'two words'")

    #expect(SSH.formatRemoteCommand(["printf", "%s|", "", "a'b", "$HOME"]) == "printf '%s|' '' 'a'\\''b' '$HOME'")
}
