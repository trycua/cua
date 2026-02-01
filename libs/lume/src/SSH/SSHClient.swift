import Foundation
@preconcurrency import NIOCore
@preconcurrency import NIOConcurrencyHelpers
@preconcurrency import NIOPosix
@preconcurrency import NIOSSH

/// Result of an SSH command execution
public struct SSHResult: Sendable {
    public let exitCode: Int32
    public let output: String

    public init(exitCode: Int32, output: String) {
        self.exitCode = exitCode
        self.output = output
    }
}

/// SSH client using SwiftNIO SSH for typed, versioned API
public actor SSHClient {
    private let host: String
    private let port: Int
    private let user: String
    private let password: String
    private let eventLoopGroup: MultiThreadedEventLoopGroup

    public init(
        host: String,
        port: UInt16 = 22,
        user: String = "lume",
        password: String = "lume"
    ) {
        self.host = host
        self.port = Int(port)
        self.user = user
        self.password = password
        self.eventLoopGroup = MultiThreadedEventLoopGroup(numberOfThreads: 1)
    }

    deinit {
        try? eventLoopGroup.syncShutdownGracefully()
    }

    /// Execute a command on the remote host
    public func execute(command: String, timeout: TimeInterval = 60) async throws -> SSHResult {
        let channel = try await connect()

        // Create a promise for the result
        let resultPromise = channel.eventLoop.makePromise(of: SSHResult.self)

        // Create the SSH child channel for command execution
        // nonisolated(unsafe) is safe because NIOSSHHandler is confined to its event loop
        nonisolated(unsafe) let sshHandler = try await channel.pipeline.handler(type: NIOSSHHandler.self).get()

        let childChannelPromise = channel.eventLoop.makePromise(of: Channel.self)
        sshHandler.createChannel(childChannelPromise) { childChannel, channelType in
            guard channelType == .session else {
                return channel.eventLoop.makeFailedFuture(SSHError.connectionFailed("Invalid channel type"))
            }

            return childChannel.eventLoop.makeCompletedFuture {
                let execHandler = CommandExecHandler(command: command, resultPromise: resultPromise)
                try childChannel.pipeline.syncOperations.addHandler(execHandler)
            }
        }

        let childChannel = try await childChannelPromise.futureResult.get()

        // Set up timeout if specified
        if timeout > 0 {
            let timeoutTask = channel.eventLoop.scheduleTask(in: .seconds(Int64(timeout))) {
                resultPromise.fail(SSHError.timeout)
                childChannel.close(promise: nil)
            }

            // Cancel timeout when result is received
            resultPromise.futureResult.whenComplete { _ in
                timeoutTask.cancel()
            }
        }

        // Wait for command completion
        let result = try await resultPromise.futureResult.get()

        // Clean up
        try? await childChannel.closeFuture.get()
        try? await channel.close().get()

        return result
    }

    /// Start an interactive SSH session
    public func interactive() async throws {
        let channel = try await connect()

        // Create the SSH child channel for interactive session
        // nonisolated(unsafe) is safe because NIOSSHHandler is confined to its event loop
        nonisolated(unsafe) let sshHandler = try await channel.pipeline.handler(type: NIOSSHHandler.self).get()

        let childChannelPromise = channel.eventLoop.makePromise(of: Channel.self)
        let sessionCompletePromise = channel.eventLoop.makePromise(of: Void.self)

        sshHandler.createChannel(childChannelPromise) { childChannel, channelType in
            guard channelType == .session else {
                return channel.eventLoop.makeFailedFuture(SSHError.connectionFailed("Invalid channel type"))
            }

            return childChannel.eventLoop.makeCompletedFuture {
                let interactiveHandler = InteractiveSessionHandler(completePromise: sessionCompletePromise)
                try childChannel.pipeline.syncOperations.addHandler(interactiveHandler)
            }
        }

        let childChannel = try await childChannelPromise.futureResult.get()

        // Wait for the session to complete
        try await sessionCompletePromise.futureResult.get()

        // Clean up
        try? await childChannel.closeFuture.get()
        try? await channel.close().get()
    }

    /// Connect to the SSH server and return the channel
    private func connect() async throws -> Channel {
        let bootstrap = ClientBootstrap(group: eventLoopGroup)
            .channelInitializer { channel in
                channel.eventLoop.makeCompletedFuture {
                    let sshHandler = NIOSSHHandler(
                        role: .client(
                            .init(
                                userAuthDelegate: PasswordAuthDelegate(
                                    username: self.user,
                                    password: self.password
                                ),
                                serverAuthDelegate: AcceptAllHostKeysDelegate()
                            )
                        ),
                        allocator: channel.allocator,
                        inboundChildChannelInitializer: nil
                    )
                    try channel.pipeline.syncOperations.addHandler(sshHandler)
                    try channel.pipeline.syncOperations.addHandler(SSHErrorHandler())
                }
            }
            .channelOption(ChannelOptions.socket(SocketOptionLevel(SOL_SOCKET), SO_REUSEADDR), value: 1)
            .channelOption(ChannelOptions.socket(SocketOptionLevel(IPPROTO_TCP), TCP_NODELAY), value: 1)
            .connectTimeout(.seconds(30))

        do {
            return try await bootstrap.connect(host: host, port: port).get()
        } catch {
            throw SSHError.connectionFailed(error.localizedDescription)
        }
    }
}

// MARK: - Host Key Validation Delegate

/// Accepts all host keys (for internal VM use only - not for production)
private final class AcceptAllHostKeysDelegate: NIOSSHClientServerAuthenticationDelegate, Sendable {
    func validateHostKey(
        hostKey: NIOSSHPublicKey,
        validationCompletePromise: EventLoopPromise<Void>
    ) {
        // Accept all host keys for internal VM connections
        // In production, you should validate against known hosts
        validationCompletePromise.succeed(())
    }
}

// MARK: - Password Authentication Delegate

/// Handles password-based authentication
private final class PasswordAuthDelegate: NIOSSHClientUserAuthenticationDelegate, Sendable {
    private let username: String
    private let password: String
    private let didAttempt: NIOLockedValueBox<Bool>

    init(username: String, password: String) {
        self.username = username
        self.password = password
        self.didAttempt = NIOLockedValueBox(false)
    }

    func nextAuthenticationType(
        availableMethods: NIOSSHAvailableUserAuthenticationMethods,
        nextChallengePromise: EventLoopPromise<NIOSSHUserAuthenticationOffer?>
    ) {
        // Only attempt password auth once
        let alreadyAttempted = didAttempt.withLockedValue { attempted in
            if attempted {
                return true
            }
            attempted = true
            return false
        }

        if alreadyAttempted {
            // We already tried, authentication failed
            nextChallengePromise.fail(SSHError.authenticationFailed)
            return
        }

        guard availableMethods.contains(.password) else {
            nextChallengePromise.fail(SSHError.authenticationFailed)
            return
        }

        nextChallengePromise.succeed(
            NIOSSHUserAuthenticationOffer(
                username: username,
                serviceName: "",
                offer: .password(.init(password: password))
            )
        )
    }
}

// MARK: - Command Execution Handler

/// Handles command execution on an SSH channel
/// Note: @unchecked Sendable is safe because this handler is only used on a single event loop
private final class CommandExecHandler: ChannelDuplexHandler, @unchecked Sendable {
    typealias InboundIn = SSHChannelData
    typealias InboundOut = ByteBuffer
    typealias OutboundIn = ByteBuffer
    typealias OutboundOut = SSHChannelData

    private let command: String
    private var resultPromise: EventLoopPromise<SSHResult>?
    private var outputBuffer: ByteBuffer
    private var exitStatus: Int32?
    private var channelClosed = false

    init(command: String, resultPromise: EventLoopPromise<SSHResult>) {
        self.command = command
        self.resultPromise = resultPromise
        self.outputBuffer = ByteBuffer()
    }

    func handlerAdded(context: ChannelHandlerContext) {
        // nonisolated(unsafe) is safe here because we're on the channel's event loop
        nonisolated(unsafe) let ctx = context
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).whenFailure { error in
            ctx.fireErrorCaught(error)
        }
    }

    func channelActive(context: ChannelHandlerContext) {
        // Send exec request
        // nonisolated(unsafe) is safe here because we're on the channel's event loop
        nonisolated(unsafe) let ctx = context
        let execRequest = SSHChannelRequestEvent.ExecRequest(command: command, wantReply: true)
        context.triggerUserOutboundEvent(execRequest).whenFailure { [weak self] error in
            self?.failWithError(error, context: ctx)
        }
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let channelData = unwrapInboundIn(data)

        switch channelData.data {
        case .byteBuffer(let buffer):
            // Collect all output (stdout and stderr)
            var mutableBuffer = buffer
            outputBuffer.writeBuffer(&mutableBuffer)
        case .fileRegion:
            // Not expected, ignore
            break
        }
    }

    func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        switch event {
        case let status as SSHChannelRequestEvent.ExitStatus:
            exitStatus = Int32(status.exitStatus)
            checkCompletion(context: context)

        case let channelEvent as ChannelEvent where channelEvent == .inputClosed:
            channelClosed = true
            checkCompletion(context: context)

        default:
            context.fireUserInboundEventTriggered(event)
        }
    }

    func channelInactive(context: ChannelHandlerContext) {
        channelClosed = true
        checkCompletion(context: context)
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        failWithError(error, context: context)
    }

    func handlerRemoved(context: ChannelHandlerContext) {
        // If we haven't completed yet, complete with what we have
        checkCompletion(context: context)
    }

    private func checkCompletion(context: ChannelHandlerContext) {
        // Only complete if we have both exit status and channel is closed (or we have exit status)
        guard let promise = resultPromise else { return }

        // Complete when we have exit status (some servers close channel before sending exit status)
        if let status = exitStatus {
            resultPromise = nil
            let output = outputBuffer.readString(length: outputBuffer.readableBytes) ?? ""
            promise.succeed(SSHResult(exitCode: status, output: output))
        } else if channelClosed {
            // Channel closed without exit status - assume success with exit code 0
            resultPromise = nil
            let output = outputBuffer.readString(length: outputBuffer.readableBytes) ?? ""
            promise.succeed(SSHResult(exitCode: 0, output: output))
        }
    }

    private func failWithError(_ error: Error, context: ChannelHandlerContext) {
        if let promise = resultPromise {
            resultPromise = nil
            if let sshError = error as? SSHError {
                promise.fail(sshError)
            } else {
                promise.fail(SSHError.commandFailed(exitCode: -1, message: error.localizedDescription))
            }
        }
        context.close(promise: nil)
    }
}

// MARK: - Interactive Session Handler

/// Handles interactive SSH sessions with stdin/stdout passthrough
/// Note: @unchecked Sendable is safe because this handler is only used on a single event loop
private final class InteractiveSessionHandler: ChannelDuplexHandler, @unchecked Sendable {
    typealias InboundIn = SSHChannelData
    typealias InboundOut = ByteBuffer
    typealias OutboundIn = ByteBuffer
    typealias OutboundOut = SSHChannelData

    private var completePromise: EventLoopPromise<Void>?
    private var stdinSource: DispatchSourceRead?
    private var originalTermios: termios?
    private let stdinFD = FileHandle.standardInput.fileDescriptor

    init(completePromise: EventLoopPromise<Void>) {
        self.completePromise = completePromise
    }

    func handlerAdded(context: ChannelHandlerContext) {
        // nonisolated(unsafe) is safe here because we're on the channel's event loop
        nonisolated(unsafe) let ctx = context
        context.channel.setOption(ChannelOptions.allowRemoteHalfClosure, value: true).whenFailure { error in
            ctx.fireErrorCaught(error)
        }
    }

    func channelActive(context: ChannelHandlerContext) {
        // nonisolated(unsafe) is safe here because we're on the channel's event loop
        nonisolated(unsafe) let ctx = context

        // Request a pseudo-terminal
        let ptyRequest = SSHChannelRequestEvent.PseudoTerminalRequest(
            wantReply: true,
            term: "xterm-256color",
            terminalCharacterWidth: 80,
            terminalRowHeight: 24,
            terminalPixelWidth: 0,
            terminalPixelHeight: 0,
            terminalModes: .init([:])
        )

        context.triggerUserOutboundEvent(ptyRequest).flatMap {
            // Request a shell
            let shellRequest = SSHChannelRequestEvent.ShellRequest(wantReply: true)
            return ctx.triggerUserOutboundEvent(shellRequest)
        }.whenComplete { [weak self] result in
            switch result {
            case .success:
                self?.setupTerminalAndStdin(context: ctx)
            case .failure(let error):
                self?.failWithError(error, context: ctx)
            }
        }
    }

    private func setupTerminalAndStdin(context: ChannelHandlerContext) {
        // Save and set raw terminal mode
        var rawTermios = termios()
        if tcgetattr(stdinFD, &rawTermios) == 0 {
            originalTermios = rawTermios
            cfmakeraw(&rawTermios)
            tcsetattr(stdinFD, TCSANOW, &rawTermios)
        }

        // Set up stdin reading
        let source = DispatchSource.makeReadSource(fileDescriptor: stdinFD, queue: .global())
        stdinSource = source

        let loopBoundContext = NIOLoopBound(context, eventLoop: context.eventLoop)
        let fd = stdinFD

        source.setEventHandler { [weak self] in
            guard self != nil else { return }

            var buffer = [UInt8](repeating: 0, count: 1024)
            let bytesRead = Darwin.read(fd, &buffer, buffer.count)

            if bytesRead > 0 {
                let data = Data(buffer[0..<bytesRead])
                loopBoundContext.value.eventLoop.execute {
                    var byteBuffer = loopBoundContext.value.channel.allocator.buffer(capacity: bytesRead)
                    byteBuffer.writeBytes(data)
                    let channelData = SSHChannelData(type: .channel, data: .byteBuffer(byteBuffer))
                    loopBoundContext.value.writeAndFlush(NIOAny(channelData), promise: nil)
                }
            } else if bytesRead == 0 {
                // EOF on stdin
                loopBoundContext.value.eventLoop.execute {
                    loopBoundContext.value.close(mode: .output, promise: nil)
                }
            }
        }

        source.setCancelHandler { [weak self] in
            self?.restoreTerminal()
        }

        source.resume()
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let channelData = unwrapInboundIn(data)

        switch channelData.data {
        case .byteBuffer(let buffer):
            // Write to stdout
            let stdoutFD = FileHandle.standardOutput.fileDescriptor
            buffer.withUnsafeReadableBytes { ptr in
                _ = Darwin.write(stdoutFD, ptr.baseAddress, ptr.count)
            }
        case .fileRegion:
            break
        }
    }

    func userInboundEventTriggered(context: ChannelHandlerContext, event: Any) {
        if let channelEvent = event as? ChannelEvent, channelEvent == .inputClosed {
            complete(context: context)
        } else {
            context.fireUserInboundEventTriggered(event)
        }
    }

    func channelInactive(context: ChannelHandlerContext) {
        complete(context: context)
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        failWithError(error, context: context)
    }

    func handlerRemoved(context: ChannelHandlerContext) {
        cleanup()
    }

    private func complete(context: ChannelHandlerContext) {
        cleanup()
        if let promise = completePromise {
            completePromise = nil
            promise.succeed(())
        }
    }

    private func failWithError(_ error: Error, context: ChannelHandlerContext) {
        cleanup()
        if let promise = completePromise {
            completePromise = nil
            promise.fail(error)
        }
        context.close(promise: nil)
    }

    private func cleanup() {
        stdinSource?.cancel()
        stdinSource = nil
        restoreTerminal()
    }

    private func restoreTerminal() {
        if var original = originalTermios {
            tcsetattr(stdinFD, TCSANOW, &original)
            originalTermios = nil
        }
    }
}

// MARK: - Error Handler

/// Handles errors in the SSH pipeline
/// Note: @unchecked Sendable is safe because this handler is only used on a single event loop
private final class SSHErrorHandler: ChannelInboundHandler, @unchecked Sendable {
    typealias InboundIn = Any

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        // Log or handle the error appropriately
        context.close(promise: nil)
    }
}
