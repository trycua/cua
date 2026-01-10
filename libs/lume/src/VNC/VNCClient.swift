import Foundation
import Network
import CoreGraphics

/// A simple VNC client that connects to a VNC server and sends input events
/// This implements the RFB (Remote Frame Buffer) protocol for input
actor VNCClient {
    private var connection: NWConnection?
    private let host: String
    private let port: UInt16
    private let password: String
    private var isConnected = false
    private var frameBufferWidth: UInt16 = 1024
    private var frameBufferHeight: UInt16 = 768

    // Pixel format from server
    private var bitsPerPixel: UInt8 = 32
    private var depth: UInt8 = 24
    private var bigEndianFlag: UInt8 = 0
    private var redShift: UInt8 = 16
    private var greenShift: UInt8 = 8
    private var blueShift: UInt8 = 0

    init(host: String, port: UInt16, password: String) {
        self.host = host
        self.port = port
        self.password = password
    }

    /// Connect to the VNC server
    func connect() async throws {
        let endpoint = NWEndpoint.hostPort(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port)!)
        connection = NWConnection(to: endpoint, using: .tcp)

        return try await withCheckedThrowingContinuation { continuation in
            connection?.stateUpdateHandler = { [weak self] state in
                Task { @MainActor in
                    switch state {
                    case .ready:
                        Logger.debug("VNC client connected to server")
                        continuation.resume()
                    case .failed(let error):
                        Logger.error("VNC client connection failed", metadata: ["error": "\(error)"])
                        continuation.resume(throwing: VNCClientError.connectionFailed("\(error)"))
                    case .cancelled:
                        Logger.debug("VNC client connection cancelled")
                    default:
                        break
                    }
                }
            }
            connection?.start(queue: .global())
        }
    }

    /// Perform the VNC handshake (RFB protocol)
    func handshake() async throws {
        guard let conn = connection else {
            throw VNCClientError.notConnected
        }

        // Read server protocol version (12 bytes: "RFB 003.008\n")
        let versionData = try await receive(length: 12)
        let versionString = String(data: versionData, encoding: .ascii) ?? ""
        Logger.debug("VNC server version", metadata: ["version": versionString.trimmingCharacters(in: .whitespacesAndNewlines)])

        // Send client protocol version
        let clientVersion = "RFB 003.008\n"
        try await send(data: clientVersion.data(using: .ascii)!)

        // Read security types
        let numSecTypes = try await receive(length: 1)
        let securityTypeCount = numSecTypes[0]

        if securityTypeCount == 0 {
            // Read error reason
            let reasonLenData = try await receive(length: 4)
            let reasonLen = UInt32(bigEndian: reasonLenData.withUnsafeBytes { $0.load(as: UInt32.self) })
            let reasonData = try await receive(length: Int(reasonLen))
            let reason = String(data: reasonData, encoding: .utf8) ?? "Unknown error"
            throw VNCClientError.securityError(reason)
        }

        let securityTypes = try await receive(length: Int(securityTypeCount))
        Logger.debug("VNC security types offered", metadata: ["types": securityTypes.map { String($0) }.joined(separator: ", ")])

        // Check for VNC Authentication (type 2) or None (type 1)
        if securityTypes.contains(2) {
            // Use VNC authentication
            try await send(data: Data([2]))
            try await performVNCAuth()
        } else if securityTypes.contains(1) {
            // Use no authentication
            try await send(data: Data([1]))
        } else {
            throw VNCClientError.securityError("No supported security type")
        }

        // Read security result (4 bytes)
        let resultData = try await receive(length: 4)
        let result = UInt32(bigEndian: resultData.withUnsafeBytes { $0.load(as: UInt32.self) })

        if result != 0 {
            // Authentication failed, try to read error message
            if let reasonLenData = try? await receive(length: 4) {
                let reasonLen = UInt32(bigEndian: reasonLenData.withUnsafeBytes { $0.load(as: UInt32.self) })
                if let reasonData = try? await receive(length: Int(reasonLen)) {
                    let reason = String(data: reasonData, encoding: .utf8) ?? "Unknown error"
                    throw VNCClientError.authenticationFailed(reason)
                }
            }
            throw VNCClientError.authenticationFailed("Security handshake failed")
        }

        Logger.debug("VNC authentication successful")

        // Send ClientInit (shared flag = 1 to allow other clients)
        try await send(data: Data([1]))

        // Read ServerInit header (24 bytes):
        // - 2 bytes: framebuffer-width
        // - 2 bytes: framebuffer-height
        // - 16 bytes: pixel-format
        // - 4 bytes: name-length
        let serverInit = try await receive(length: 24)
        frameBufferWidth = UInt16(bigEndian: serverInit.withUnsafeBytes { $0.load(fromByteOffset: 0, as: UInt16.self) })
        frameBufferHeight = UInt16(bigEndian: serverInit.withUnsafeBytes { $0.load(fromByteOffset: 2, as: UInt16.self) })

        // Parse pixel format (16 bytes at offset 4)
        // bits-per-pixel, depth, big-endian-flag, true-colour-flag,
        // red-max, green-max, blue-max, red-shift, green-shift, blue-shift
        self.bitsPerPixel = serverInit[4]
        self.depth = serverInit[5]
        self.bigEndianFlag = serverInit[6]
        let trueColourFlag = serverInit[7]
        let redMax = UInt16(bigEndian: serverInit.withUnsafeBytes { $0.load(fromByteOffset: 8, as: UInt16.self) })
        let greenMax = UInt16(bigEndian: serverInit.withUnsafeBytes { $0.load(fromByteOffset: 10, as: UInt16.self) })
        let blueMax = UInt16(bigEndian: serverInit.withUnsafeBytes { $0.load(fromByteOffset: 12, as: UInt16.self) })
        self.redShift = serverInit[14]
        self.greenShift = serverInit[15]
        self.blueShift = serverInit[16]

        // Name length is at offset 20 in the ServerInit message
        let nameLen = UInt32(bigEndian: serverInit.withUnsafeBytes { $0.load(fromByteOffset: 20, as: UInt32.self) })
        Logger.debug("ServerInit received", metadata: [
            "width": "\(frameBufferWidth)",
            "height": "\(frameBufferHeight)",
            "bitsPerPixel": "\(bitsPerPixel)",
            "depth": "\(depth)",
            "bigEndian": "\(bigEndianFlag)",
            "trueColour": "\(trueColourFlag)",
            "redMax": "\(redMax)",
            "greenMax": "\(greenMax)",
            "blueMax": "\(blueMax)",
            "redShift": "\(redShift)",
            "greenShift": "\(greenShift)",
            "blueShift": "\(blueShift)",
            "nameLen": "\(nameLen)"
        ])

        // Read server name if present
        if nameLen > 0 && nameLen < 1024 { // Sanity check on name length
            _ = try await receive(length: Int(nameLen)) // Read and discard server name
        }

        // Send SetEncodings message to request supported encodings
        // Message type 2: SetEncodings
        // Include pseudo-encodings that Apple's VNC server expects
        var encodingsMsg = Data(capacity: 20)
        encodingsMsg.append(2)  // Message type 2 = SetEncodings
        encodingsMsg.append(0)  // Padding
        encodingsMsg.append(contentsOf: withUnsafeBytes(of: UInt16(4).bigEndian) { Array($0) })  // Number of encodings
        // Raw encoding = 0 (must support)
        encodingsMsg.append(contentsOf: withUnsafeBytes(of: Int32(0).bigEndian) { Array($0) })
        // DesktopSize pseudo-encoding = -223 (Apple requires this)
        encodingsMsg.append(contentsOf: withUnsafeBytes(of: Int32(-223).bigEndian) { Array($0) })
        // Cursor pseudo-encoding = -239
        encodingsMsg.append(contentsOf: withUnsafeBytes(of: Int32(-239).bigEndian) { Array($0) })
        // LastRect pseudo-encoding = -224
        encodingsMsg.append(contentsOf: withUnsafeBytes(of: Int32(-224).bigEndian) { Array($0) })
        try await send(data: encodingsMsg)
        Logger.debug("Sent SetEncodings request")

        Logger.info("VNC handshake complete", metadata: [
            "width": "\(frameBufferWidth)",
            "height": "\(frameBufferHeight)"
        ])

        isConnected = true
    }

    /// Perform VNC authentication using DES challenge-response
    private func performVNCAuth() async throws {
        // Read 16-byte challenge
        let challenge = try await receive(length: 16)

        // Prepare the password (must be exactly 8 bytes, padded with zeros)
        var keyBytes = [UInt8](repeating: 0, count: 8)
        let passwordBytes = Array(password.utf8)
        for i in 0..<min(8, passwordBytes.count) {
            keyBytes[i] = passwordBytes[i]
        }

        // VNC uses a modified DES where each byte of the key is bit-reversed
        let reversedKey = keyBytes.map { reverseBits($0) }

        // Encrypt the challenge using DES-ECB
        let response = try desEncrypt(challenge: Array(challenge), key: reversedKey)

        try await send(data: Data(response))
    }

    /// Reverse bits in a byte (VNC's quirky DES key format)
    private func reverseBits(_ byte: UInt8) -> UInt8 {
        var result: UInt8 = 0
        var b = byte
        for _ in 0..<8 {
            result = (result << 1) | (b & 1)
            b >>= 1
        }
        return result
    }

    /// Simple DES encryption for VNC authentication
    /// Uses CommonCrypto under the hood
    private func desEncrypt(challenge: [UInt8], key: [UInt8]) throws -> [UInt8] {
        var outBuffer = [UInt8](repeating: 0, count: 16)
        var outLength = 0

        // Use CCCrypt for DES-ECB encryption
        let status = key.withUnsafeBytes { keyPtr in
            challenge.withUnsafeBytes { dataPtr in
                outBuffer.withUnsafeMutableBytes { outPtr in
                    CCCrypt(
                        CCOperation(kCCEncrypt),
                        CCAlgorithm(kCCAlgorithmDES),
                        CCOptions(kCCOptionECBMode),
                        keyPtr.baseAddress, 8,
                        nil,
                        dataPtr.baseAddress, 16,
                        outPtr.baseAddress, 16,
                        &outLength
                    )
                }
            }
        }

        if status != kCCSuccess {
            throw VNCClientError.encryptionFailed
        }

        return outBuffer
    }

    /// Send a pointer (mouse) event
    /// Button mask: bit 0 = left, bit 1 = middle, bit 2 = right
    func sendPointerEvent(x: UInt16, y: UInt16, buttonMask: UInt8) async throws {
        guard isConnected else {
            throw VNCClientError.notConnected
        }

        // RFB PointerEvent: message-type (1) + button-mask (1) + x-position (2) + y-position (2)
        var data = Data(capacity: 6)
        data.append(5) // Message type 5 = PointerEvent
        data.append(buttonMask)
        data.append(contentsOf: withUnsafeBytes(of: x.bigEndian) { Array($0) })
        data.append(contentsOf: withUnsafeBytes(of: y.bigEndian) { Array($0) })

        try await send(data: data)
        Logger.debug("Sent VNC pointer event", metadata: [
            "x": "\(x)",
            "y": "\(y)",
            "buttonMask": "\(buttonMask)"
        ])
    }

    /// Send a key event
    /// Key is an X11 keysym
    func sendKeyEvent(key: UInt32, down: Bool) async throws {
        guard isConnected else {
            throw VNCClientError.notConnected
        }

        // RFB KeyEvent: message-type (1) + down-flag (1) + padding (2) + key (4)
        var data = Data(capacity: 8)
        data.append(4) // Message type 4 = KeyEvent
        data.append(down ? 1 : 0) // Down flag
        data.append(contentsOf: [0, 0]) // Padding
        data.append(contentsOf: withUnsafeBytes(of: key.bigEndian) { Array($0) })

        try await send(data: data)
        Logger.debug("Sent VNC key event", metadata: [
            "key": "0x\(String(key, radix: 16))",
            "down": "\(down)"
        ])
    }

    /// Get framebuffer dimensions
    func getFrameBufferSize() -> (width: UInt16, height: UInt16) {
        return (frameBufferWidth, frameBufferHeight)
    }

    /// Request and receive a framebuffer update from the VNC server
    /// Returns the framebuffer as a CGImage
    func captureFramebuffer() async throws -> CGImage {
        guard isConnected else {
            Logger.debug("VNCClient.captureFramebuffer: not connected")
            throw VNCClientError.notConnected
        }

        Logger.debug("VNCClient.captureFramebuffer: requesting framebuffer", metadata: [
            "width": "\(frameBufferWidth)",
            "height": "\(frameBufferHeight)"
        ])

        // Send FramebufferUpdateRequest
        // Message type 3: request full framebuffer
        var request = Data(capacity: 10)
        request.append(3)  // Message type 3 = FramebufferUpdateRequest
        request.append(0)  // Incremental = 0 (request full update)
        request.append(contentsOf: withUnsafeBytes(of: UInt16(0).bigEndian) { Array($0) })  // x-position
        request.append(contentsOf: withUnsafeBytes(of: UInt16(0).bigEndian) { Array($0) })  // y-position
        request.append(contentsOf: withUnsafeBytes(of: frameBufferWidth.bigEndian) { Array($0) })  // width
        request.append(contentsOf: withUnsafeBytes(of: frameBufferHeight.bigEndian) { Array($0) })  // height

        try await send(data: request)

        // Read FramebufferUpdate response
        let header = try await receive(length: 4)
        let messageType = header[0]

        guard messageType == 0 else {
            throw VNCClientError.receiveFailed("Unexpected message type: \(messageType)")
        }

        // header[1] is padding
        let numRectangles = UInt16(bigEndian: header.withUnsafeBytes { $0.load(fromByteOffset: 2, as: UInt16.self) })

        // Allocate pixel buffer (BGRA format, 4 bytes per pixel)
        let bytesPerPixel = 4
        let stride = Int(frameBufferWidth) * bytesPerPixel
        var pixelData = [UInt8](repeating: 0, count: Int(frameBufferWidth) * Int(frameBufferHeight) * bytesPerPixel)

        // Process each rectangle
        for _ in 0..<numRectangles {
            // Rectangle header: x (2) + y (2) + width (2) + height (2) + encoding (4) = 12 bytes
            let rectHeader = try await receive(length: 12)

            let x = UInt16(bigEndian: rectHeader.withUnsafeBytes { $0.load(fromByteOffset: 0, as: UInt16.self) })
            let y = UInt16(bigEndian: rectHeader.withUnsafeBytes { $0.load(fromByteOffset: 2, as: UInt16.self) })
            let width = UInt16(bigEndian: rectHeader.withUnsafeBytes { $0.load(fromByteOffset: 4, as: UInt16.self) })
            let height = UInt16(bigEndian: rectHeader.withUnsafeBytes { $0.load(fromByteOffset: 6, as: UInt16.self) })
            let encoding = Int32(bigEndian: rectHeader.withUnsafeBytes { $0.load(fromByteOffset: 8, as: Int32.self) })

            // Handle different encodings
            switch encoding {
            case 0:  // Raw encoding
                let rectPixelCount = Int(width) * Int(height) * bytesPerPixel
                let rectData = try await receive(length: rectPixelCount)

                // Copy rectangle data into the pixel buffer
                for row in 0..<Int(height) {
                    let srcOffset = row * Int(width) * bytesPerPixel
                    let dstY = Int(y) + row
                    let dstOffset = dstY * stride + Int(x) * bytesPerPixel

                    for col in 0..<Int(width) * bytesPerPixel {
                        if dstOffset + col < pixelData.count && srcOffset + col < rectData.count {
                            pixelData[dstOffset + col] = rectData[srcOffset + col]
                        }
                    }
                }

            case -239:  // Cursor pseudo-encoding
                // Cursor data: pixels (width * height * bytesPerPixel) + bitmask ((width+7)/8 * height)
                let cursorPixels = Int(width) * Int(height) * bytesPerPixel
                let bitmaskRowBytes = (Int(width) + 7) / 8
                let cursorBitmask = bitmaskRowBytes * Int(height)
                _ = try await receive(length: cursorPixels + cursorBitmask)
                Logger.debug("Skipped cursor encoding", metadata: ["width": "\(width)", "height": "\(height)"])

            case -223:  // DesktopSize pseudo-encoding
                // No additional data, just updates dimensions
                frameBufferWidth = width
                frameBufferHeight = height
                Logger.debug("Desktop size changed", metadata: ["width": "\(width)", "height": "\(height)"])

            case -224:  // LastRect pseudo-encoding
                // No additional data, signals end of rectangles
                Logger.debug("LastRect encoding received")
                break

            default:
                // For unsupported encodings, log and skip
                Logger.debug("Unsupported VNC encoding (skipping)", metadata: ["encoding": "\(encoding)"])
                // Try to continue instead of throwing - some encodings have no extra data
            }
        }

        // Create CGImage from pixel data
        // Determine the correct pixel format based on server's pixel format
        // VNC typically sends pixels as: [padding/alpha][R][G][B] or [B][G][R][padding/alpha]
        // depending on the shift values
        let colorSpace = CGColorSpaceCreateDeviceRGB()

        // Determine bitmap info based on server pixel format
        // Common formats from Apple VNC:
        // - redShift=16, greenShift=8, blueShift=0 = ARGB/XRGB (alpha/skip first, big endian within pixel)
        // - redShift=0, greenShift=8, blueShift=16 = BGRA/BGRX (alpha/skip last)
        let bitmapInfo: CGBitmapInfo
        if redShift == 16 && greenShift == 8 && blueShift == 0 {
            // ARGB format - standard macOS format
            // In memory: [A/X][R][G][B] for big endian, [B][G][R][A/X] for little endian
            if bigEndianFlag != 0 {
                bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
            } else {
                // Little endian: bytes are [B][G][R][X] which CGImage interprets as BGRX
                bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
            }
        } else if redShift == 0 && greenShift == 8 && blueShift == 16 {
            // RGBA/RGBX format
            bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
        } else {
            // Default fallback - assume little endian BGRA
            Logger.debug("Unknown pixel format, using default", metadata: [
                "redShift": "\(redShift)",
                "greenShift": "\(greenShift)",
                "blueShift": "\(blueShift)"
            ])
            bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        }

        guard let provider = CGDataProvider(data: Data(pixelData) as CFData),
              let cgImage = CGImage(
                  width: Int(frameBufferWidth),
                  height: Int(frameBufferHeight),
                  bitsPerComponent: 8,
                  bitsPerPixel: 32,
                  bytesPerRow: stride,
                  space: colorSpace,
                  bitmapInfo: bitmapInfo,
                  provider: provider,
                  decode: nil,
                  shouldInterpolate: false,
                  intent: .defaultIntent
              ) else {
            throw VNCClientError.receiveFailed("Failed to create CGImage from framebuffer")
        }

        return cgImage
    }

    /// Disconnect from the server
    func disconnect() {
        connection?.cancel()
        connection = nil
        isConnected = false
        Logger.debug("VNC client disconnected")
    }

    // MARK: - Private helpers

    private func send(data: Data) async throws {
        guard let conn = connection else {
            throw VNCClientError.notConnected
        }

        return try await withCheckedThrowingContinuation { continuation in
            conn.send(content: data, completion: .contentProcessed { error in
                if let error = error {
                    continuation.resume(throwing: VNCClientError.sendFailed("\(error)"))
                } else {
                    continuation.resume()
                }
            })
        }
    }

    private func receive(length: Int) async throws -> Data {
        guard let conn = connection else {
            throw VNCClientError.notConnected
        }

        return try await withCheckedThrowingContinuation { continuation in
            conn.receive(minimumIncompleteLength: length, maximumLength: length) { data, _, _, error in
                if let error = error {
                    continuation.resume(throwing: VNCClientError.receiveFailed("\(error)"))
                } else if let data = data {
                    continuation.resume(returning: data)
                } else {
                    continuation.resume(throwing: VNCClientError.receiveFailed("No data received"))
                }
            }
        }
    }
}

/// Errors from VNC client operations
enum VNCClientError: Error, LocalizedError {
    case connectionFailed(String)
    case notConnected
    case securityError(String)
    case authenticationFailed(String)
    case encryptionFailed
    case sendFailed(String)
    case receiveFailed(String)

    var errorDescription: String? {
        switch self {
        case .connectionFailed(let msg):
            return "VNC connection failed: \(msg)"
        case .notConnected:
            return "VNC client not connected"
        case .securityError(let msg):
            return "VNC security error: \(msg)"
        case .authenticationFailed(let msg):
            return "VNC authentication failed: \(msg)"
        case .encryptionFailed:
            return "VNC encryption failed"
        case .sendFailed(let msg):
            return "VNC send failed: \(msg)"
        case .receiveFailed(let msg):
            return "VNC receive failed: \(msg)"
        }
    }
}

// CommonCrypto imports for DES encryption
import CommonCrypto
