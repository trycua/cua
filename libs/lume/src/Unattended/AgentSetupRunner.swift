import Foundation
import CoreGraphics
import ImageIO

/// Runs macOS Setup Assistant automation using Claude's computer-use API
/// instead of brittle YAML presets. Adapts to any macOS version.
@MainActor
final class AgentSetupRunner {
    private let vncService: VNCService
    private let client: AnthropicClient
    /// Display dimensions reported to Claude (capped for API efficiency)
    private let displayWidth: Int
    private let displayHeight: Int
    /// Actual VNC framebuffer dimensions (for coordinate mapping)
    private let vncWidth: Int
    private let vncHeight: Int
    private let maxIterations: Int
    private let debug: Bool
    private let debugDirectory: URL
    private var stepIndex: Int = 0
    /// Optional SSH reachability check — returns true if SSH is reachable
    private let sshCheck: (() async -> Bool)?

    /// Max dimensions to report to Claude API (keeps screenshot payloads small)
    private static let maxAPIWidth = 1024
    private static let maxAPIHeight = 768

    static let defaultSystemPrompt = """
        You are automating the macOS Setup Assistant on a freshly installed virtual machine.
        Your goal is to complete the setup as quickly as possible with these settings:

        1. Language: English
        2. Country: United States
        3. Skip all Apple ID sign-in (look for "Set Up Later", "Skip", "Other Sign-In Options", or similar)
        4. Create a local user account with:
           - Full Name: lume
           - Account Name: lume
           - Password: lume
        5. Skip or decline all optional features (Siri, Analytics, Screen Time, Location Services, etc.)
        6. Accept any privacy/terms screens by clicking Continue/Agree
        7. Reach the macOS desktop and enable SSH

        ## Expected screen sequence (adapt as needed — screens may vary by macOS version):
        1. "Hello" greeting animation → press Space or click to dismiss
        2. Language selection → type "English", select it, press Enter
        3. Country/Region → type "United States", press Enter, click Continue
        4. Transfer Your Data → click "Set up as new", then Continue
        5. Written and Spoken Languages → click Continue
        6. Accessibility → click "Not Now"
        7. Data & Privacy → click Continue
        8. Create a Mac Account → fill Full Name "lume", Tab to skip Account Name (auto-fills), type password "lume", Tab, verify password "lume", Tab to skip Hint, Tab to checkbox, press Space to untoggle "Allow reset with Apple Account", click Continue
        9. Apple Account sign-in → look for "Set Up Later" or "Other Sign-In Options" or "Skip". If a "Don't Skip" confirmation appears, Tab to "Skip" and press Enter
        10. Terms and Conditions → click "Agree" button (at bottom, not in the text), then click "Agree" again in the confirmation popup
        11. Enable Location Services → click Continue, then confirm "Don't Use" in popup
        12. Select Your Time Zone → click Continue
        13. Analytics → click Continue
        14. Screen Time → click "Set Up Later"
        15. Siri → click to enable/disable, then click Continue
        16. FileVault → click "Not Now", then confirm in popup
        17. Choose Your Look → click Continue
        18. Update Mac Automatically → click Continue
        19. Welcome/Get Started → click "Get Started"

        ## After reaching the desktop, enable SSH:
        1. Open Spotlight (Cmd+Space), type "System Settings", press Enter
        2. Search for "Remote Login" using Cmd+F, click "Remote Login"
        3. Enable the Remote Login toggle, allow all users, click Done
        4. Close System Settings (Cmd+Q)
        5. The task is now complete.

        ## Important tips:
        - The screenshot colors may appear tinted/shifted — this is a VNC artifact. Focus on shapes and layout.
        - After each action, take a screenshot to verify the result before proceeding.
        - If a button or text is not visible, try waiting 2-3 seconds, then take another screenshot.
        - Use keyboard shortcuts when buttons are hard to click (Tab to navigate, Enter to confirm, Space to toggle).
        - If you see a password confirmation field, enter "lume" again.
        - When you see the macOS desktop with the Dock and menu bar, proceed to enable SSH.
        - When you see the lock screen with the user "lume", the setup is complete — just stop.
        """

    init(
        vncService: VNCService,
        apiKey: String,
        model: String = "claude-sonnet-4-6",
        displayWidth: Int,
        displayHeight: Int,
        maxIterations: Int = 200,
        debug: Bool = false,
        debugDirectory: URL? = nil,
        sshCheck: (() async -> Bool)? = nil
    ) {
        self.vncService = vncService
        self.client = AnthropicClient(apiKey: apiKey, model: model)
        self.vncWidth = displayWidth
        self.vncHeight = displayHeight
        // Cap display dimensions for API efficiency
        self.displayWidth = min(displayWidth, Self.maxAPIWidth)
        self.displayHeight = min(displayHeight, Self.maxAPIHeight)
        self.maxIterations = maxIterations
        self.debug = debug
        self.debugDirectory = debugDirectory ?? FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-setup-\(UUID().uuidString)")
        self.sshCheck = sshCheck
    }

    /// Run the agent loop to complete Setup Assistant
    func run(systemPrompt: String? = nil) async throws {
        let prompt = systemPrompt ?? Self.defaultSystemPrompt

        if debug {
            try? FileManager.default.createDirectory(at: debugDirectory, withIntermediateDirectories: true)
            Logger.info("Agent debug screenshots will be saved to \(debugDirectory.path)")
        }

        // Take initial screenshot
        Logger.info("Agent taking initial screenshot")
        let initialScreenshot = try await captureScreenshot()

        // Build initial messages with screenshot
        var messages: [[String: Any]] = [
            [
                "role": "user",
                "content": [
                    [
                        "type": "text",
                        "text": "Here is the current screen. Please complete the macOS Setup Assistant."
                    ] as [String: Any],
                    [
                        "type": "image",
                        "source": [
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": initialScreenshot
                        ]
                    ] as [String: Any]
                ]
            ]
        ]

        var iteration = 0
        while iteration < maxIterations {
            iteration += 1
            Logger.info("Agent iteration \(iteration)/\(maxIterations)")

            // Call Claude API
            let response = try await client.sendMessage(
                systemPrompt: prompt,
                messages: messages,
                displayWidth: displayWidth,
                displayHeight: displayHeight
            )

            // Add assistant response to conversation
            let assistantContent = try responseToRawContent(response)
            messages.append(["role": "assistant", "content": assistantContent])

            // Check if Claude is done (no tool use)
            let toolUseBlocks = response.content.filter {
                if case .toolUse = $0 { return true }
                return false
            }

            if toolUseBlocks.isEmpty {
                // Claude thinks the task is complete
                for block in response.content {
                    if case .text(let text) = block {
                        Logger.info("Agent completed: \(text)")
                    }
                }
                Logger.info("Agent setup finished after \(iteration) iterations")
                return
            }

            // Process each tool use
            var toolResults: [[String: Any]] = []
            for block in response.content {
                guard case .toolUse(let id, let name, let input) = block else { continue }
                guard name == "computer" else {
                    toolResults.append([
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": "Unknown tool: \(name)",
                        "is_error": true
                    ])
                    continue
                }

                let result = try await executeComputerAction(id: id, input: input)
                toolResults.append(result)
            }

            // Add tool results to messages
            messages.append(["role": "user", "content": toolResults])

            // Check SSH reachability — if SSH is up, setup is complete regardless of agent state.
            // Only start checking after step 50 (setup assistant takes at least that many) and
            // check every 10 iterations to avoid slowing down the loop.
            if iteration >= 50 && iteration % 10 == 0, let sshCheck = sshCheck {
                if await sshCheck() {
                    Logger.info("SSH is reachable — setup complete after \(iteration) iterations")
                    return
                }
            }

            // Keep conversation history bounded to prevent request_too_large errors.
            // Keep the first message (initial screenshot) plus the last 20 messages (10 turns).
            let maxHistory = 20
            if messages.count > maxHistory + 1 {
                let first = messages[0]
                let recent = Array(messages.suffix(maxHistory))
                messages = [first] + recent
            }
        }

        throw AnthropicError.maxIterationsReached
    }

    // MARK: - Action Execution

    private func executeComputerAction(id: String, input: [String: AnyCodable]) async throws -> [String: Any] {
        guard let action = input["action"]?.stringValue else {
            return errorResult(id: id, message: "Missing 'action' field")
        }

        Logger.info("Agent action: \(action)", metadata: inputMetadata(input))
        stepIndex += 1

        do {
            switch action {
            case "screenshot":
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "left_click":
                guard let coord = coordinateFromInput(input) else {
                    return errorResult(id: id, message: "Missing coordinate for left_click")
                }
                try await vncService.sendMouseClick(at: coord, button: .left)
                try await Task.sleep(nanoseconds: 500_000_000) // 500ms for UI to update
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "right_click":
                guard let coord = coordinateFromInput(input) else {
                    return errorResult(id: id, message: "Missing coordinate for right_click")
                }
                try await vncService.sendMouseClick(at: coord, button: .right)
                try await Task.sleep(nanoseconds: 500_000_000)
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "double_click":
                guard let coord = coordinateFromInput(input) else {
                    return errorResult(id: id, message: "Missing coordinate for double_click")
                }
                try await vncService.sendMouseClick(at: coord, button: .left)
                try await Task.sleep(nanoseconds: 100_000_000)
                try await vncService.sendMouseClick(at: coord, button: .left)
                try await Task.sleep(nanoseconds: 500_000_000)
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "type":
                guard let text = input["text"]?.stringValue else {
                    return errorResult(id: id, message: "Missing text for type action")
                }
                try await vncService.sendText(text, delayMs: 50)
                try await Task.sleep(nanoseconds: 300_000_000)
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "key":
                guard let keyText = input["text"]?.stringValue else {
                    return errorResult(id: id, message: "Missing text for key action")
                }
                try await sendKey(keyText)
                try await Task.sleep(nanoseconds: 300_000_000)
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "scroll":
                // For scroll, we just take a screenshot after — VNC scroll support varies
                guard let coord = coordinateFromInput(input) else {
                    return errorResult(id: id, message: "Missing coordinate for scroll")
                }
                let direction = input["scroll_direction"]?.stringValue ?? "down"
                let amount = input["scroll_amount"]?.intValue ?? 3
                try await sendScroll(at: coord, direction: direction, amount: amount)
                try await Task.sleep(nanoseconds: 500_000_000)
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            case "mouse_move":
                // Just acknowledge — VNC doesn't need explicit moves before clicks
                return [
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": "Mouse moved"
                ]

            case "wait":
                let duration = input["duration"]?.intValue ?? 2
                try await Task.sleep(nanoseconds: UInt64(duration) * 1_000_000_000)
                let base64 = try await captureScreenshot()
                return screenshotResult(id: id, base64: base64)

            default:
                return errorResult(id: id, message: "Unsupported action: \(action)")
            }
        } catch {
            Logger.error("Agent action failed", metadata: ["action": action, "error": error.localizedDescription])
            return errorResult(id: id, message: "Action '\(action)' failed: \(error.localizedDescription)")
        }
    }

    // MARK: - Screenshot

    private func captureScreenshot() async throws -> String {
        let image = try await vncService.captureFramebuffer()

        // Normalize and resize: re-render through a standard RGBA CGContext.
        // This fixes VNC color channel issues (BGRA→RGBA) and scales to display size.
        let normalized = normalizeImage(image, maxWidth: displayWidth, maxHeight: displayHeight)

        if debug {
            saveDebugScreenshot(normalized)
        }

        // Encode as JPEG for much smaller payloads (PNG screenshots can be 3MB+)
        guard let base64 = normalized.jpegBase64(quality: 0.7) else {
            throw UnattendedError.framebufferCaptureFailed("Failed to encode screenshot as JPEG")
        }

        return base64
    }

    /// Resize the image to fit within the given bounds. Let CoreGraphics handle pixel format
    /// conversion naturally — no manual channel swapping needed.
    private func normalizeImage(_ image: CGImage, maxWidth: Int, maxHeight: Int) -> CGImage {
        let srcWidth = image.width
        let srcHeight = image.height

        // Scale to fit within bounds
        let scale: Double
        if srcWidth > maxWidth || srcHeight > maxHeight {
            scale = min(Double(maxWidth) / Double(srcWidth), Double(maxHeight) / Double(srcHeight))
        } else {
            scale = 1.0
        }
        let newWidth = Int(Double(srcWidth) * scale)
        let newHeight = Int(Double(srcHeight) * scale)

        let colorSpace = CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: nil,
            width: newWidth,
            height: newHeight,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            return image
        }

        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: newWidth, height: newHeight))

        return context.makeImage() ?? image
    }

    private func saveDebugScreenshot(_ image: CGImage) {
        let filename = String(format: "%03d-agent-step.png", stepIndex)
        let url = debugDirectory.appendingPathComponent(filename)
        guard let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else {
            return
        }
        CGImageDestinationAddImage(destination, image, nil)
        CGImageDestinationFinalize(destination)
        Logger.info("Saved debug screenshot", metadata: ["path": url.path])
    }

    // MARK: - Input Helpers

    /// Parse coordinate from Claude's response and scale to VNC framebuffer space.
    /// Claude operates in displayWidth x displayHeight space, VNC uses vncWidth x vncHeight.
    private func coordinateFromInput(_ input: [String: AnyCodable]) -> CGPoint? {
        if let coordArray = input["coordinate"]?.value as? [Any], coordArray.count == 2 {
            let x: Double
            let y: Double
            if let xi = coordArray[0] as? Int, let yi = coordArray[1] as? Int {
                x = Double(xi)
                y = Double(yi)
            } else if let xd = coordArray[0] as? Double, let yd = coordArray[1] as? Double {
                x = xd
                y = yd
            } else {
                return nil
            }
            // Scale from Claude's coordinate space to VNC framebuffer space
            let scaleX = Double(vncWidth) / Double(displayWidth)
            let scaleY = Double(vncHeight) / Double(displayHeight)
            return CGPoint(x: x * scaleX, y: y * scaleY)
        }
        return nil
    }

    private func sendKey(_ keyText: String) async throws {
        // Parse key combinations like "ctrl+s", "Return", "space", etc.
        let parts = keyText.lowercased().split(separator: "+").map(String.init)

        var modifiers: VNCKeyModifiers = .none
        var mainKey: String = ""

        for part in parts {
            switch part {
            case "ctrl", "control": modifiers.insert(.control)
            case "shift": modifiers.insert(.shift)
            case "alt", "option": modifiers.insert(.option)
            case "cmd", "command", "super", "meta": modifiers.insert(.command)
            default: mainKey = part
            }
        }

        let keyCode = keyNameToKeyCode(mainKey)
        try await vncService.sendKeyPress(keyCode, modifiers: modifiers)
    }

    private func sendScroll(at point: CGPoint, direction: String, amount: Int) async throws {
        // VNC scroll is button 4 (up) or button 5 (down)
        // Use key-based scrolling as fallback (VNC scroll button support varies)
        for _ in 0..<amount {
            switch direction {
            case "up":
                try await vncService.sendKeyPress(keyNameToKeyCode("up"), modifiers: .none)
            case "down":
                try await vncService.sendKeyPress(keyNameToKeyCode("down"), modifiers: .none)
            case "left":
                try await vncService.sendKeyPress(keyNameToKeyCode("left"), modifiers: .none)
            case "right":
                try await vncService.sendKeyPress(keyNameToKeyCode("right"), modifiers: .none)
            default:
                break
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
    }

    private func keyNameToKeyCode(_ name: String) -> UInt16 {
        // Map common key names to macOS key codes
        switch name {
        case "return", "enter": return 36
        case "tab": return 48
        case "space": return 49
        case "escape", "esc": return 53
        case "backspace", "delete": return 51
        case "up": return 126
        case "down": return 125
        case "left": return 123
        case "right": return 124
        case "a": return 0
        case "b": return 11
        case "c": return 8
        case "d": return 2
        case "e": return 14
        case "f": return 3
        case "g": return 5
        case "h": return 4
        case "i": return 34
        case "j": return 38
        case "k": return 40
        case "l": return 37
        case "m": return 46
        case "n": return 45
        case "o": return 31
        case "p": return 35
        case "q": return 12
        case "r": return 15
        case "s": return 1
        case "t": return 17
        case "u": return 32
        case "v": return 9
        case "w": return 13
        case "x": return 7
        case "y": return 16
        case "z": return 6
        case "f1": return 122
        case "f2": return 120
        case "f3": return 99
        case "f4": return 118
        case "f5": return 96
        case "f6": return 97
        case "f7": return 98
        case "f8": return 100
        case "f9": return 101
        case "f10": return 109
        case "f11": return 103
        case "f12": return 111
        default: return 49 // space as fallback
        }
    }

    // MARK: - Result Builders

    private func screenshotResult(id: String, base64: String) -> [String: Any] {
        return [
            "type": "tool_result",
            "tool_use_id": id,
            "content": [
                [
                    "type": "image",
                    "source": [
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": base64
                    ]
                ] as [String: Any]
            ]
        ]
    }

    private func errorResult(id: String, message: String) -> [String: Any] {
        return [
            "type": "tool_result",
            "tool_use_id": id,
            "content": message,
            "is_error": true
        ]
    }

    private func inputMetadata(_ input: [String: AnyCodable]) -> [String: String] {
        var meta: [String: String] = [:]
        if let action = input["action"]?.stringValue { meta["action"] = action }
        if let text = input["text"]?.stringValue { meta["text"] = text }
        if let coord = input["coordinate"]?.value as? [Any] {
            meta["coordinate"] = "\(coord)"
        }
        return meta
    }

    // MARK: - Response Serialization

    private func responseToRawContent(_ response: AnthropicClient.APIResponse) throws -> [[String: Any]] {
        var content: [[String: Any]] = []
        for block in response.content {
            switch block {
            case .text(let text):
                content.append(["type": "text", "text": text])
            case .toolUse(let id, let name, let input):
                var inputDict: [String: Any] = [:]
                for (key, val) in input {
                    inputDict[key] = val.value
                }
                content.append([
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": inputDict
                ])
            default:
                break
            }
        }
        return content
    }
}
