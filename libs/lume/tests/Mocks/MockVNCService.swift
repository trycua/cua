import CoreGraphics
import Foundation
import Virtualization
@testable import lume

@MainActor
final class MockVNCService: VNCService {
    private(set) var url: String?
    private(set) var isRunning = false
    private(set) var clientOpenCount = 0
    private var _attachedVM: Any?
    private let vmDirectory: VMDirectory

    init(vmDirectory: VMDirectory) {
        self.vmDirectory = vmDirectory
    }

    nonisolated var attachedVM: String? {
        get async {
            await Task { @MainActor in
                _attachedVM as? String
            }.value
        }
    }

    // MARK: - VNCService Protocol

    var virtualMachine: VZVirtualMachine? {
        return nil
    }

    func start(port: Int, virtualMachine: Any?) async throws {
        isRunning = true
        url = "vnc://localhost:\(port)"
        _attachedVM = virtualMachine
    }

    func stop() {
        isRunning = false
        url = nil
        _attachedVM = nil
    }

    func openClient(url: String) async throws {
        guard isRunning else {
            throw VMError.vncNotConfigured
        }
        clientOpenCount += 1
    }

    // MARK: - Automation Support (stub implementations for testing)

    func captureFramebuffer() async throws -> CGImage {
        // Return a minimal 1x1 black image for testing
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: nil,
            width: 1,
            height: 1,
            bitsPerComponent: 8,
            bytesPerRow: 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ), let image = context.makeImage() else {
            throw UnattendedError.framebufferCaptureFailed("Mock failed to create image")
        }
        return image
    }

    func sendMouseClick(at point: CGPoint, button: VNCMouseButton) async throws {
        // No-op for testing
    }

    func sendKeyPress(_ keyCode: UInt16, modifiers: VNCKeyModifiers) async throws {
        // No-op for testing
    }

    func sendText(_ text: String) async throws {
        // No-op for testing
    }

    func sendCharWithModifiers(_ char: Character, modifiers: VNCKeyModifiers) async throws {
        // No-op for testing
    }

    func connectInputClient() async throws {
        // No-op for testing
    }

    func disconnectInputClient() {
        // No-op for testing
    }

    func getFrameBufferSize() async -> (width: UInt16, height: UInt16)? {
        return (1920, 1080)
    }
}
