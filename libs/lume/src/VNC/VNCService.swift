import Foundation
import Dynamic
import Virtualization
import CoreGraphics
import Network

/// Protocol defining the interface for VNC server operations
@MainActor
protocol VNCService {
    var url: String? { get }
    func start(port: Int, virtualMachine: Any?) async throws
    func stop()
    func openClient(url: String) async throws

    // Automation support
    var virtualMachine: VZVirtualMachine? { get }
    func captureFramebuffer() async throws -> CGImage
    func sendMouseClick(at point: CGPoint, button: VNCMouseButton) async throws
    func sendKeyPress(_ keyCode: UInt16, modifiers: VNCKeyModifiers) async throws
    func sendText(_ text: String) async throws
    func sendCharWithModifiers(_ char: Character, modifiers: VNCKeyModifiers) async throws

    // VNC client for input
    func connectInputClient() async throws
    func disconnectInputClient()
    func getFrameBufferSize() async -> (width: UInt16, height: UInt16)?
}

/// Mouse button for VNC input
enum VNCMouseButton: UInt8 {
    case left = 1
    case middle = 2
    case right = 4
}

/// Key modifiers for VNC input
struct VNCKeyModifiers: OptionSet, Sendable {
    let rawValue: UInt32

    static let shift = VNCKeyModifiers(rawValue: 1 << 0)
    static let control = VNCKeyModifiers(rawValue: 1 << 1)
    static let option = VNCKeyModifiers(rawValue: 1 << 2)
    static let command = VNCKeyModifiers(rawValue: 1 << 3)

    static let none: VNCKeyModifiers = []
}

/// Default implementation of VNCService
@MainActor
final class DefaultVNCService: VNCService {
    private var vncServer: Any?
    private let vmDirectory: VMDirectory
    private var _virtualMachine: VZVirtualMachine?
    private var vncClient: VNCClient?
    private var vncPort: UInt16 = 0
    private var vncPassword: String = ""

    var virtualMachine: VZVirtualMachine? {
        return _virtualMachine
    }

    init(vmDirectory: VMDirectory) {
        self.vmDirectory = vmDirectory
    }
    
    var url: String? {
        get {
            return try? vmDirectory.loadSession().url
        }
    }
    
    func start(port: Int, virtualMachine: Any?) async throws {
        let password = Array(PassphraseGenerator().prefix(4)).joined(separator: "-")
        let securityConfiguration = Dynamic._VZVNCAuthenticationSecurityConfiguration(password: password)

        // Store password for later VNC client connection
        self.vncPassword = password

        // Create VNC server with specified port
        let server = Dynamic._VZVNCServer(port: port, queue: DispatchQueue.main,
                                      securityConfiguration: securityConfiguration)

        if let vm = virtualMachine as? VZVirtualMachine {
            server.virtualMachine = vm
            _virtualMachine = vm
        }
        server.start()

        vncServer = server

        // Wait for port to be assigned (both for auto-assign and specific port)
        var attempts = 0
        let maxAttempts = 20  // 1 second total wait time
        while true {
            if let assignedPort: UInt16 = server.port.asUInt16 {
                // If we got a non-zero port, check if it matches our request
                if assignedPort != 0 {
                    // For specific port requests, verify we got the requested port
                    if port != 0 && Int(assignedPort) != port {
                        throw VMError.vncPortBindingFailed(requested: port, actual: Int(assignedPort))
                    }

                    // Store port for later VNC client connection
                    self.vncPort = assignedPort

                    // Get the local IP address for the URL - prefer IPv4
                    let hostIP = try getLocalIPAddress() ?? "127.0.0.1"
                    let url = "vnc://:\(password)@127.0.0.1:\(assignedPort)"  // Use localhost for local connections
                    let externalUrl = "vnc://:\(password)@\(hostIP):\(assignedPort)"  // External URL for remote connections

                    Logger.info("VNC server started", metadata: [
                        "local": url,
                        "external": externalUrl
                    ])

                    // Save session information with local URL for the client
                    let session = VNCSession(url: url)
                    try vmDirectory.saveSession(session)
                    break
                }
            }

            attempts += 1
            if attempts >= maxAttempts {
                // If we've timed out and we requested a specific port, it likely means binding failed
                vncServer = nil
                if port != 0 {
                    throw VMError.vncPortBindingFailed(requested: port, actual: -1)
                }
                throw VMError.internalError("Timeout waiting for VNC server to start")
            }
            try await Task.sleep(nanoseconds: 50_000_000)  // 50ms delay between checks
        }
    }
    
    // Modified to prefer IPv4 addresses
    private func getLocalIPAddress() throws -> String? {
        var address: String?
        
        var ifaddr: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&ifaddr) == 0 else {
            return nil
        }
        defer { freeifaddrs(ifaddr) }
        
        var ptr = ifaddr
        while ptr != nil {
            defer { ptr = ptr?.pointee.ifa_next }
            
            let interface = ptr?.pointee
            let family = interface?.ifa_addr.pointee.sa_family
            
            // Only look for IPv4 addresses
            if family == UInt8(AF_INET) {
                let name = String(cString: (interface?.ifa_name)!)
                if name == "en0" { // Primary interface
                    var hostname = [CChar](repeating: 0, count: Int(NI_MAXHOST))
                    getnameinfo(interface?.ifa_addr,
                              socklen_t((interface?.ifa_addr.pointee.sa_len)!),
                              &hostname,
                              socklen_t(hostname.count),
                              nil,
                              0,
                              NI_NUMERICHOST)
                    address = String(cString: hostname, encoding: .utf8)
                    break
                }
            }
        }
        
        return address
    }
    
    func stop() {
        // Disconnect VNC client first
        disconnectInputClient()

        if let server = vncServer as? Dynamic {
            server.stop()
        }
        vncServer = nil
        vmDirectory.clearSession()
    }

    func openClient(url: String) async throws {
        let processRunner = DefaultProcessRunner()
        try processRunner.run(executable: "/usr/bin/open", arguments: [url])
    }

    /// Connect a VNC client to the server for sending input events
    func connectInputClient() async throws {
        guard vncPort != 0 else {
            throw VNCClientError.notConnected
        }

        // Disconnect existing client if any
        disconnectInputClient()

        let client = VNCClient(host: "127.0.0.1", port: vncPort, password: vncPassword)
        try await client.connect()
        try await client.handshake()

        self.vncClient = client
        Logger.info("VNC input client connected", metadata: ["port": "\(vncPort)"])
    }

    /// Disconnect the VNC input client
    func disconnectInputClient() {
        if let client = vncClient {
            Task {
                await client.disconnect()
            }
            vncClient = nil
        }
    }

    /// Get the VNC framebuffer dimensions from the connected client
    func getFrameBufferSize() async -> (width: UInt16, height: UInt16)? {
        guard let client = vncClient else {
            return nil
        }
        return await client.getFrameBufferSize()
    }

    // MARK: - Automation Support

    func captureFramebuffer() async throws -> CGImage {
        guard let client = vncClient else {
            throw UnattendedError.framebufferCaptureFailed("VNC client not connected")
        }

        Logger.debug("Capturing framebuffer via VNC protocol")
        let image = try await client.captureFramebuffer()
        Logger.debug("VNC framebuffer capture succeeded", metadata: [
            "width": "\(image.width)",
            "height": "\(image.height)"
        ])
        return image
    }

    func sendMouseClick(at point: CGPoint, button: VNCMouseButton = .left) async throws {
        guard let client = vncClient else {
            throw UnattendedError.inputSimulationFailed("VNC input client not connected")
        }

        Logger.debug("Sending mouse click via VNC client", metadata: [
            "x": "\(Int(point.x))",
            "y": "\(Int(point.y))",
            "button": "\(button.rawValue)"
        ])

        let x = UInt16(max(0, Int(point.x)))
        let y = UInt16(max(0, Int(point.y)))

        // Move to position
        try await client.sendPointerEvent(x: x, y: y, buttonMask: 0)
        try await Task.sleep(nanoseconds: 50_000_000) // 50ms

        // Press button
        try await client.sendPointerEvent(x: x, y: y, buttonMask: button.rawValue)
        try await Task.sleep(nanoseconds: 100_000_000) // 100ms

        // Release button
        try await client.sendPointerEvent(x: x, y: y, buttonMask: 0)

        Logger.debug("Mouse click sent via VNC client")
    }

    func sendKeyPress(_ keyCode: UInt16, modifiers: VNCKeyModifiers = .none) async throws {
        guard let client = vncClient else {
            throw UnattendedError.inputSimulationFailed("VNC input client not connected")
        }

        // Convert macOS key code to X11 keysym
        let keysym = macKeyCodeToKeysym(keyCode)

        Logger.debug("Sending key press via VNC client", metadata: [
            "keyCode": "\(keyCode)",
            "keysym": "0x\(String(keysym, radix: 16))",
            "modifiers": "\(modifiers.rawValue)"
        ])

        // Strategy: Send all key-down events as fast as possible (simulating simultaneous press)
        // Then hold, then release all at once
        // This is how real keyboards work - keys are pressed "together"

        // Press ALL keys down as fast as possible (no delays between them)
        if modifiers.contains(.shift) {
            try await client.sendKeyEvent(key: X11Keysym.shiftL.rawValue, down: true)
        }
        if modifiers.contains(.control) {
            try await client.sendKeyEvent(key: X11Keysym.controlL.rawValue, down: true)
        }
        // OSXvnc-server mapping (from source code analysis):
        // XK_Alt_L/R -> kCGEventFlagMaskCommand (Command key)
        // XK_Meta_L/R -> kCGEventFlagMaskAlternate (Option key)
        // XK_Super_L/R -> NOT mapped to Command (contrary to RFB spec)
        if modifiers.contains(.option) {
            // Option uses Meta on OSXvnc
            try await client.sendKeyEvent(key: X11Keysym.metaL.rawValue, down: true)
        }
        if modifiers.contains(.command) {
            // Command uses Alt on OSXvnc
            try await client.sendKeyEvent(key: X11Keysym.altL.rawValue, down: true)
        }
        // Press main key immediately after modifiers (no delay!)
        try await client.sendKeyEvent(key: keysym, down: true)

        // NOW hold all keys together for a longer period
        try await Task.sleep(nanoseconds: 300_000_000) // 300ms - hold the combination

        // Release ALL keys as fast as possible (simulating simultaneous release)
        try await client.sendKeyEvent(key: keysym, down: false)
        if modifiers.contains(.command) {
            try await client.sendKeyEvent(key: X11Keysym.altL.rawValue, down: false)
        }
        if modifiers.contains(.option) {
            try await client.sendKeyEvent(key: X11Keysym.metaL.rawValue, down: false)
        }
        if modifiers.contains(.control) {
            try await client.sendKeyEvent(key: X11Keysym.controlL.rawValue, down: false)
        }
        if modifiers.contains(.shift) {
            try await client.sendKeyEvent(key: X11Keysym.shiftL.rawValue, down: false)
        }

        // Small delay after hotkey
        try await Task.sleep(nanoseconds: 50_000_000)

        Logger.debug("Key press sent via VNC client")
    }

    func sendText(_ text: String) async throws {
        guard let client = vncClient else {
            throw UnattendedError.inputSimulationFailed("VNC input client not connected")
        }

        // Type each character using X11 keysyms
        for char in text {
            let (keysym, needsShift) = charToKeysym(char)

            if needsShift {
                try await client.sendKeyEvent(key: X11Keysym.shiftL.rawValue, down: true)
            }

            try await client.sendKeyEvent(key: keysym, down: true)
            try await Task.sleep(nanoseconds: 80_000_000) // 80ms key hold
            try await client.sendKeyEvent(key: keysym, down: false)

            if needsShift {
                try await client.sendKeyEvent(key: X11Keysym.shiftL.rawValue, down: false)
            }

            try await Task.sleep(nanoseconds: 120_000_000) // 120ms between characters
        }
    }

    func sendCharWithModifiers(_ char: Character, modifiers: VNCKeyModifiers) async throws {
        guard let client = vncClient else {
            throw UnattendedError.inputSimulationFailed("VNC input client not connected")
        }

        // Get keysym for the character
        let (keysym, _) = charToKeysym(char)

        Logger.debug("Sending char with modifiers via VNC client", metadata: [
            "char": "\(char)",
            "keysym": "0x\(String(keysym, radix: 16))",
            "modifiers": "\(modifiers.rawValue)"
        ])

        // Strategy: Send all key-down events as fast as possible (simulating simultaneous press)
        // Then hold, then release all at once
        // This is how real keyboards work - keys are pressed "together"

        // Press ALL keys down as fast as possible (no delays between them)
        if modifiers.contains(.shift) {
            try await client.sendKeyEvent(key: X11Keysym.shiftL.rawValue, down: true)
        }
        if modifiers.contains(.control) {
            try await client.sendKeyEvent(key: X11Keysym.controlL.rawValue, down: true)
        }
        // OSXvnc-server mapping (from source code analysis):
        // XK_Alt_L/R -> kCGEventFlagMaskCommand (Command key)
        // XK_Meta_L/R -> kCGEventFlagMaskAlternate (Option key)
        if modifiers.contains(.option) {
            // Option uses Meta on OSXvnc
            try await client.sendKeyEvent(key: X11Keysym.metaL.rawValue, down: true)
        }
        if modifiers.contains(.command) {
            // Command uses Alt on OSXvnc
            try await client.sendKeyEvent(key: X11Keysym.altL.rawValue, down: true)
        }
        // Press main key immediately after modifiers (no delay!)
        try await client.sendKeyEvent(key: keysym, down: true)

        // NOW hold all keys together for a longer period
        try await Task.sleep(nanoseconds: 300_000_000) // 300ms - hold the combination

        // Release ALL keys as fast as possible (simulating simultaneous release)
        try await client.sendKeyEvent(key: keysym, down: false)
        if modifiers.contains(.command) {
            try await client.sendKeyEvent(key: X11Keysym.altL.rawValue, down: false)
        }
        if modifiers.contains(.option) {
            try await client.sendKeyEvent(key: X11Keysym.metaL.rawValue, down: false)
        }
        if modifiers.contains(.control) {
            try await client.sendKeyEvent(key: X11Keysym.controlL.rawValue, down: false)
        }
        if modifiers.contains(.shift) {
            try await client.sendKeyEvent(key: X11Keysym.shiftL.rawValue, down: false)
        }

        // Small delay after hotkey
        try await Task.sleep(nanoseconds: 50_000_000)

        Logger.debug("Char with modifiers sent via VNC client")
    }
} 