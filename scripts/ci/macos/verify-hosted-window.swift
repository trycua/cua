import ApplicationServices
import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers
import Vision

struct Arguments {
    let marker: String
    let output: URL
    let displayOutput: URL

    init() throws {
        var marker: String?
        var output: String?
        var displayOutput: String?
        var index = 1
        while index < CommandLine.arguments.count {
            switch CommandLine.arguments[index] {
            case "--marker" where index + 1 < CommandLine.arguments.count:
                index += 1
                marker = CommandLine.arguments[index]
            case "--output" where index + 1 < CommandLine.arguments.count:
                index += 1
                output = CommandLine.arguments[index]
            case "--display-output" where index + 1 < CommandLine.arguments.count:
                index += 1
                displayOutput = CommandLine.arguments[index]
            default:
                throw ProbeError("unknown or incomplete argument: \(CommandLine.arguments[index])")
            }
            index += 1
        }
        guard let marker, let output, let displayOutput else {
            throw ProbeError(
                "usage: verify-hosted-window.swift --marker TEXT --output PATH --display-output PATH"
            )
        }
        self.marker = marker
        self.output = URL(fileURLWithPath: output)
        self.displayOutput = URL(fileURLWithPath: displayOutput)
    }
}

struct ProbeError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func normalize(_ value: String) -> String {
    value.uppercased().filter { $0.isLetter || $0.isNumber }
}

func shareableContent() async throws -> SCShareableContent {
    try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
}

func textEditWindow(in content: SCShareableContent) throws -> SCWindow {
    let candidates = content.windows.filter { window in
        window.owningApplication?.applicationName == "TextEdit"
            && window.windowLayer == 0
            && window.title == "probe.txt"
            && window.frame.width >= 400
            && window.frame.height >= 250
    }
    guard let window = candidates.max(by: {
        $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height
    }) else {
        throw ProbeError("no large, on-screen TextEdit window was found")
    }
    return window
}

func captureWindow(_ window: SCWindow) async throws -> CGImage {
    let filter = SCContentFilter(desktopIndependentWindow: window)
    let configuration = SCStreamConfiguration()
    configuration.width = max(1, Int(window.frame.width))
    configuration.height = max(1, Int(window.frame.height))
    configuration.showsCursor = false
    return try await SCScreenshotManager.captureImage(
        contentFilter: filter,
        configuration: configuration
    )
}

func captureDisplay(_ display: SCDisplay) async throws -> CGImage {
    let filter = SCContentFilter(display: display, excludingWindows: [])
    let configuration = SCStreamConfiguration()
    configuration.width = display.width
    configuration.height = display.height
    configuration.showsCursor = false
    return try await SCScreenshotManager.captureImage(
        contentFilter: filter,
        configuration: configuration
    )
}

func writePNG(_ image: CGImage, to output: URL) throws {
    guard let destination = CGImageDestinationCreateWithURL(
        output as CFURL,
        UTType.png.identifier as CFString,
        1,
        nil
    ) else {
        throw ProbeError("could not create PNG destination")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        throw ProbeError("could not write PNG")
    }
}

func recognizeText(in image: CGImage) throws -> String {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    let handler = VNImageRequestHandler(cgImage: image, options: [:])
    try handler.perform([request])
    return (request.results ?? []).compactMap { observation in
        observation.topCandidates(1).first?.string
    }.joined(separator: "\n")
}

func runProbe() async throws {
    let arguments = try Arguments()
    let accessibilityTrusted = AXIsProcessTrusted()
    let screenCapturePreflight = CGPreflightScreenCaptureAccess()
    let content = try await shareableContent()
    let window = try textEditWindow(in: content)
    guard let display = content.displays.first(where: { $0.frame.intersects(window.frame) }) else {
        throw ProbeError("no display contains the TextEdit probe window")
    }
    let image = try await captureWindow(window)
    try writePNG(image, to: arguments.output)
    let recognizedText = try recognizeText(in: image)
    let markerRecognized = normalize(recognizedText).contains(normalize(arguments.marker))
    let displayImage = try await captureDisplay(display)
    try writePNG(displayImage, to: arguments.displayOutput)
    let displayText = try recognizeText(in: displayImage)
    let displayMarkerRecognized = normalize(displayText).contains(normalize(arguments.marker))

    let result: [String: Any] = [
        "schema": "cua-driver/macos-hosted-window-capture@v1",
        "accessibility_trusted": accessibilityTrusted,
        "screen_capture_preflight": screenCapturePreflight,
        "marker_recognized": markerRecognized,
        "recognized_text": recognizedText,
        "permission_attribution_scope": "probe process only; does not establish CuaDriverLocal.app TCC",
        "probe_executable": Bundle.main.executableURL?.path ?? CommandLine.arguments[0],
        "probe_process_id": ProcessInfo.processInfo.processIdentifier,
        "image": ["width": image.width, "height": image.height],
        "display_capture": [
            "id": display.displayID,
            "width": display.width,
            "height": display.height,
            "image_width": displayImage.width,
            "image_height": displayImage.height,
            "marker_recognized": displayMarkerRecognized,
            "recognized_text": displayText,
        ],
        "window": [
            "id": window.windowID,
            "owner": window.owningApplication?.applicationName ?? "",
            "name": window.title ?? "",
            "width": window.frame.width,
            "height": window.frame.height,
        ],
    ]
    let encoded = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
    FileHandle.standardOutput.write(encoded)
    FileHandle.standardOutput.write(Data("\n".utf8))
    if !accessibilityTrusted || !screenCapturePreflight || !markerRecognized || !displayMarkerRecognized {
        throw ProbeError("permission preflight or marker recognition failed")
    }
}

Task {
    do {
        try await runProbe()
        exit(0)
    } catch {
        FileHandle.standardError.write(Data("hosted macOS window verification failed: \(error)\n".utf8))
        exit(1)
    }
}
dispatchMain()
