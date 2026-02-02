import Foundation
import CoreGraphics
import AppKit

/// VNC automation engine for executing boot commands
@MainActor
final class VNCAutomation {
    private let vncService: VNCService
    private let ocrService: OCRServiceProtocol
    private let displayWidth: CGFloat
    private let displayHeight: CGFloat
    private let debug: Bool
    private let debugDirectory: URL

    /// Polling interval for OCR-based waits (in seconds)
    private let pollInterval: TimeInterval = 1.0

    /// Default timeout for wait commands
    private let defaultWaitTimeout: TimeInterval = 120.0

    /// Command index for debug screenshot naming
    private var commandIndex: Int = 0

    init(
        vncService: VNCService,
        ocrService: OCRServiceProtocol = VisionOCRService(),
        displayWidth: CGFloat,
        displayHeight: CGFloat,
        debug: Bool = false,
        debugDirectory: URL? = nil
    ) {
        self.vncService = vncService
        self.ocrService = ocrService
        self.displayWidth = displayWidth
        self.displayHeight = displayHeight
        self.debug = debug
        // Use custom directory or default to unique folder in system temp directory
        if let debugDirectory = debugDirectory {
            self.debugDirectory = debugDirectory
        } else {
            self.debugDirectory = FileManager.default.temporaryDirectory
                .appendingPathComponent("unattended-\(UUID().uuidString)")
        }
    }

    /// Execute a single boot command
    func execute(_ command: BootCommand) async throws {
        switch command {
        case .waitForText(let text, let timeout):
            try await waitForText(text, timeout: timeout)

        case .clickText(let text):
            try await clickOnText(text, xOffset: 0, yOffset: 0, index: nil)

        case .clickTextWithOffset(let text, let xOffset, let yOffset):
            try await clickOnText(text, xOffset: xOffset, yOffset: yOffset, index: nil)

        case .clickTextWithIndex(let text, let index, let xOffset, let yOffset):
            try await clickOnText(text, xOffset: xOffset, yOffset: yOffset, index: index)

        case .clickAt(let x, let y):
            try await vncService.sendMouseClick(
                at: CGPoint(x: x, y: y),
                button: .left
            )

        case .typeText(let text, let delay):
            try await vncService.sendText(text, delayMs: delay)

        case .keyPress(let key):
            let keyCode = specialKeyToKeyCode(key)
            try await vncService.sendKeyPress(keyCode, modifiers: .none)

        case .hotkey(let modifiers, let key):
            try await sendHotkey(modifiers: modifiers, key: key)

        case .hotkeyChar(let modifiers, let char):
            try await sendHotkeyChar(modifiers: modifiers, char: char)

        case .delay(let duration):
            try await Task.sleep(nanoseconds: UInt64(duration * 1_000_000_000))
        }
    }

    /// Convert SpecialKey modifiers to VNCKeyModifiers
    private func specialKeysToVNCModifiers(_ modifiers: [SpecialKey]) -> VNCKeyModifiers {
        var vncModifiers: VNCKeyModifiers = .none
        for modifier in modifiers {
            switch modifier {
            case .leftShift:
                vncModifiers.insert(.shift)
            case .leftCtrl:
                vncModifiers.insert(.control)
            case .leftAlt:
                vncModifiers.insert(.option)
            case .leftSuper:
                vncModifiers.insert(.command)
            default:
                break
            }
        }
        return vncModifiers
    }

    /// Send a hotkey combination (modifiers + special key)
    private func sendHotkey(modifiers: [SpecialKey], key: SpecialKey) async throws {
        let vncModifiers = specialKeysToVNCModifiers(modifiers)
        let keyCode = specialKeyToKeyCode(key)
        try await vncService.sendKeyPress(keyCode, modifiers: vncModifiers)

        Logger.debug("Sent hotkey", metadata: [
            "modifiers": modifiers.map { "\($0)" }.joined(separator: "+"),
            "key": "\(key)"
        ])
    }

    /// Send a hotkey combination (modifiers + character key like 'q', 'c', 'v')
    private func sendHotkeyChar(modifiers: [SpecialKey], char: Character) async throws {
        let vncModifiers = specialKeysToVNCModifiers(modifiers)
        try await vncService.sendCharWithModifiers(char, modifiers: vncModifiers)

        Logger.debug("Sent hotkey char", metadata: [
            "modifiers": modifiers.map { "\($0)" }.joined(separator: "+"),
            "char": "\(char)"
        ])
    }

    /// Execute a sequence of boot commands
    func executeAll(_ commands: [BootCommand]) async throws {
        // Create debug directory if needed
        if debug {
            try? FileManager.default.createDirectory(at: debugDirectory, withIntermediateDirectories: true)
            Logger.info("Debug mode enabled - saving screenshots to \(debugDirectory.path)")
        }

        for (index, command) in commands.enumerated() {
            commandIndex = index + 1
            Logger.info("Executing boot command", metadata: [
                "index": "\(commandIndex)/\(commands.count)",
                "command": describeCommand(command)
            ])
            try await execute(command)

            // Small delay between commands for stability
            try await Task.sleep(nanoseconds: 200_000_000)
        }
    }

    /// Wait for text to appear on screen
    private func waitForText(_ text: String, timeout: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        var pollCount = 0

        while Date() < deadline {
            pollCount += 1
            do {
                let framebuffer = try await vncService.captureFramebuffer()
                Logger.debug("Captured framebuffer", metadata: [
                    "poll": "\(pollCount)",
                    "width": "\(framebuffer.width)",
                    "height": "\(framebuffer.height)"
                ])

                // Get all text from screen for debugging
                let allObservations = try await ocrService.recognizeText(in: framebuffer)
                if !allObservations.isEmpty {
                    let foundTexts = allObservations.map { "\($0.text)(\(String(format: "%.2f", $0.confidence)))" }.joined(separator: " | ")
                    Logger.debug("OCR found texts", metadata: [
                        "count": "\(allObservations.count)",
                        "texts": String(foundTexts.prefix(500))
                    ])
                } else {
                    Logger.debug("OCR found no text", metadata: ["poll": "\(pollCount)"])
                }

                if let _ = try await ocrService.findText(text, in: framebuffer) {
                    Logger.info("Text found on screen", metadata: ["text": text])
                    return
                }
            } catch {
                // Log but continue polling - framebuffer capture might fail temporarily
                Logger.debug("OCR poll failed", metadata: ["error": error.localizedDescription])
            }

            try await Task.sleep(nanoseconds: UInt64(pollInterval * 1_000_000_000))
        }

        throw UnattendedError.textNotFound(text: text, timeout: timeout)
    }

    /// Find text on screen and click on it
    /// - Parameters:
    ///   - text: Text to find via OCR
    ///   - xOffset: Horizontal offset in VNC pixels (negative = left, positive = right)
    ///   - yOffset: Vertical offset in VNC pixels (negative = up, positive = down)
    ///   - index: Which match to click (0 = first, 1 = second, -1 = last, -2 = second to last, etc.)
    private func clickOnText(_ text: String, xOffset: Int, yOffset: Int, index: Int?) async throws {
        let framebuffer = try await vncService.captureFramebuffer()

        // Always get all OCR observations for debug logging
        let allObservations = debug ? try await ocrService.recognizeText(in: framebuffer) : []

        let observation: TextObservation?
        if let idx = index {
            // Find all matches and select by index
            let allMatches = try await ocrService.findAllText(text, in: framebuffer)
            if allMatches.isEmpty {
                observation = nil
            } else if idx >= 0 {
                // Positive index: 0 = first, 1 = second, etc.
                observation = idx < allMatches.count ? allMatches[idx] : nil
            } else {
                // Negative index: -1 = last, -2 = second to last, etc.
                let positiveIdx = allMatches.count + idx
                observation = positiveIdx >= 0 ? allMatches[positiveIdx] : nil
            }

            Logger.info("Text search with index", metadata: [
                "text": text,
                "index": "\(idx)",
                "totalMatches": "\(allMatches.count)",
                "selected": observation != nil ? "yes" : "no"
            ])
        } else {
            observation = try await ocrService.findText(text, in: framebuffer)
        }

        guard let observation = observation else {
            // Save debug screenshot even on failure with full OCR tree
            if debug {
                saveDebugScreenshot(framebuffer: framebuffer, clickPoint: nil, text: text, failed: true, ocrObservations: allObservations)
            }
            throw UnattendedError.textNotFound(text: text, timeout: 0)
        }

        // Use captured framebuffer dimensions for coordinate mapping
        let capturedWidth = CGFloat(framebuffer.width)
        let capturedHeight = CGFloat(framebuffer.height)

        // Get center point in screen coordinates (using captured image dimensions)
        let centerPoint = observation.centerPoint(
            screenWidth: capturedWidth,
            screenHeight: capturedHeight
        )

        // Use captured framebuffer coordinates directly for VNC input
        // The Screen Sharing window capture represents the actual VM display,
        // and VNC coordinates should match what we see on screen
        let vncPoint = CGPoint(
            x: centerPoint.x + CGFloat(xOffset),
            y: centerPoint.y + CGFloat(yOffset)
        )

        // The click point in captured framebuffer coordinates (for debug screenshot)
        let clickPointInCapture = CGPoint(
            x: centerPoint.x + CGFloat(xOffset),
            y: centerPoint.y + CGFloat(yOffset)
        )

        Logger.info("Clicking on text", metadata: [
            "text": text,
            "captured": "\(Int(capturedWidth))x\(Int(capturedHeight))",
            "display": "\(Int(displayWidth))x\(Int(displayHeight))",
            "capturedPoint": "(\(Int(centerPoint.x)), \(Int(centerPoint.y)))",
            "vncPoint": "(\(Int(vncPoint.x)), \(Int(vncPoint.y)))",
            "xOffset": "\(xOffset)",
            "yOffset": "\(yOffset)"
        ])

        // Save debug screenshot before clicking with full OCR tree
        if debug {
            saveDebugScreenshot(framebuffer: framebuffer, clickPoint: clickPointInCapture, text: text, failed: false, ocrObservations: allObservations)
        }

        try await vncService.sendMouseClick(at: vncPoint, button: .left)
    }

    /// Convert special key enum to macOS key code
    private func specialKeyToKeyCode(_ key: SpecialKey) -> UInt16 {
        switch key {
        case .enter: return 36
        case .tab: return 48
        case .escape: return 53
        case .leftSuper: return 55  // Command
        case .leftShift: return 56
        case .leftAlt: return 58    // Option
        case .leftCtrl: return 59
        case .backspace: return 51
        case .delete: return 117
        case .up: return 126
        case .down: return 125
        case .left: return 123
        case .right: return 124
        case .space: return 49
        case .f1: return 122
        case .f2: return 120
        case .f3: return 99
        case .f4: return 118
        case .f5: return 96
        case .f6: return 97
        case .f7: return 98
        case .f8: return 100
        case .f9: return 101
        case .f10: return 109
        case .f11: return 103
        case .f12: return 111
        }
    }

    /// Generate a human-readable description of a command
    private func describeCommand(_ command: BootCommand) -> String {
        switch command {
        case .waitForText(let text, let timeout):
            return "wait for '\(text)' (timeout: \(Int(timeout))s)"
        case .clickText(let text):
            return "click '\(text)'"
        case .clickTextWithOffset(let text, let xOffset, let yOffset):
            return "click '\(text)' (xoffset: \(xOffset), yoffset: \(yOffset))"
        case .clickTextWithIndex(let text, let index, let xOffset, let yOffset):
            return "click '\(text)' (index: \(index), xoffset: \(xOffset), yoffset: \(yOffset))"
        case .clickAt(let x, let y):
            return "click at (\(x), \(y))"
        case .typeText(let text, let delay):
            let displayText = text.count > 20 ? String(text.prefix(20)) + "..." : text
            if let delay = delay {
                return "type '\(displayText)' (delay: \(delay)ms)"
            }
            return "type '\(displayText)'"
        case .keyPress(let key):
            return "press <\(key.rawValue)>"
        case .hotkey(let modifiers, let key):
            let modifierStr = modifiers.map { "\($0)" }.joined(separator: "+")
            return "hotkey <\(modifierStr)+\(key)>"
        case .hotkeyChar(let modifiers, let char):
            let modifierStr = modifiers.map { "\($0)" }.joined(separator: "+")
            return "hotkey <\(modifierStr)+\(char)>"
        case .delay(let duration):
            return "delay \(duration)s"
        }
    }

    /// Save a debug screenshot with click point marked and OCR tree log
    /// - Parameters:
    ///   - framebuffer: The captured framebuffer
    ///   - clickPoint: The point where click will occur (in framebuffer coordinates), or nil if text not found
    ///   - text: The text being searched for
    ///   - failed: Whether the text search failed
    ///   - ocrObservations: All OCR observations for logging (optional)
    private func saveDebugScreenshot(
        framebuffer: CGImage,
        clickPoint: CGPoint?,
        text: String,
        failed: Bool,
        ocrObservations: [TextObservation]? = nil
    ) {
        let width = framebuffer.width
        let height = framebuffer.height

        // Create a new image with the click point marked
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            Logger.error("Failed to create graphics context for debug screenshot")
            return
        }

        // Draw the original framebuffer
        context.draw(framebuffer, in: CGRect(x: 0, y: 0, width: width, height: height))

        if let clickPoint = clickPoint {
            // Draw a red crosshair at the click point
            // Note: CGContext has origin at bottom-left, so flip Y coordinate
            let flippedY = CGFloat(height) - clickPoint.y

            // Set red color
            context.setStrokeColor(CGColor(red: 1.0, green: 0.0, blue: 0.0, alpha: 1.0))
            context.setLineWidth(3.0)

            // Draw crosshair
            let crosshairSize: CGFloat = 20

            // Horizontal line
            context.move(to: CGPoint(x: clickPoint.x - crosshairSize, y: flippedY))
            context.addLine(to: CGPoint(x: clickPoint.x + crosshairSize, y: flippedY))
            context.strokePath()

            // Vertical line
            context.move(to: CGPoint(x: clickPoint.x, y: flippedY - crosshairSize))
            context.addLine(to: CGPoint(x: clickPoint.x, y: flippedY + crosshairSize))
            context.strokePath()

            // Draw a circle around the click point
            context.setLineWidth(2.0)
            let circleRect = CGRect(
                x: clickPoint.x - crosshairSize,
                y: flippedY - crosshairSize,
                width: crosshairSize * 2,
                height: crosshairSize * 2
            )
            context.strokeEllipse(in: circleRect)
        }

        guard let annotatedImage = context.makeImage() else {
            Logger.error("Failed to create annotated image for debug screenshot")
            return
        }

        // Generate filename
        let sanitizedText = text
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "\\", with: "_")
            .replacingOccurrences(of: "^", with: "")
            .replacingOccurrences(of: "$", with: "")
            .prefix(30)
        let status = failed ? "FAILED" : "click"
        let filename = String(format: "%03d-%@-%@.png", commandIndex, status, String(sanitizedText))
        let fileURL = debugDirectory.appendingPathComponent(filename)

        // Save as PNG
        let bitmapRep = NSBitmapImageRep(cgImage: annotatedImage)
        guard let pngData = bitmapRep.representation(using: .png, properties: [:]) else {
            Logger.error("Failed to create PNG data for debug screenshot")
            return
        }

        do {
            try pngData.write(to: fileURL)
            Logger.info("Saved debug screenshot", metadata: [
                "path": fileURL.path,
                "clickPoint": clickPoint.map { "(\(Int($0.x)), \(Int($0.y)))" } ?? "none"
            ])
        } catch {
            Logger.error("Failed to save debug screenshot", metadata: ["error": error.localizedDescription])
        }

        // Save OCR tree log as JSON alongside the screenshot
        if let observations = ocrObservations, !observations.isEmpty {
            let ocrLogFilename = String(format: "%03d-%@-%@-ocr.json", commandIndex, status, String(sanitizedText))
            let ocrLogURL = debugDirectory.appendingPathComponent(ocrLogFilename)

            // Build OCR tree with all observations, sorted top-to-bottom
            let sortedObservations = observations.sorted { $0.boundingBox.origin.y > $1.boundingBox.origin.y }

            var ocrEntries: [[String: Any]] = []
            for (idx, obs) in sortedObservations.enumerated() {
                let screenRect = obs.screenRect(screenWidth: CGFloat(width), screenHeight: CGFloat(height))
                let center = obs.centerPoint(screenWidth: CGFloat(width), screenHeight: CGFloat(height))

                // Check if this observation matches the search text
                let isMatch = obs.text.localizedCaseInsensitiveContains(text)

                let entry: [String: Any] = [
                    "index": idx,
                    "text": obs.text,
                    "confidence": String(format: "%.3f", obs.confidence),
                    "boundingBox": [
                        "x": Int(screenRect.origin.x),
                        "y": Int(screenRect.origin.y),
                        "width": Int(screenRect.width),
                        "height": Int(screenRect.height)
                    ],
                    "center": [
                        "x": Int(center.x),
                        "y": Int(center.y)
                    ],
                    "matchesSearch": isMatch
                ]
                ocrEntries.append(entry)
            }

            let ocrLog: [String: Any] = [
                "timestamp": ISO8601DateFormatter().string(from: Date()),
                "commandIndex": commandIndex,
                "searchText": text,
                "failed": failed,
                "framebufferSize": ["width": width, "height": height],
                "clickPoint": clickPoint.map { ["x": Int($0.x), "y": Int($0.y)] } as Any,
                "observationCount": observations.count,
                "observations": ocrEntries
            ]

            do {
                let jsonData = try JSONSerialization.data(withJSONObject: ocrLog, options: [.prettyPrinted, .sortedKeys])
                try jsonData.write(to: ocrLogURL)
                Logger.info("Saved OCR tree log", metadata: [
                    "path": ocrLogURL.path,
                    "observationCount": "\(observations.count)"
                ])
            } catch {
                Logger.error("Failed to save OCR tree log", metadata: ["error": error.localizedDescription])
            }
        }
    }
}
