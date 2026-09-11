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

    init() throws {
        var marker: String?
        var output: String?
        var index = 1
        while index < CommandLine.arguments.count {
            switch CommandLine.arguments[index] {
            case "--marker" where index + 1 < CommandLine.arguments.count:
                index += 1
                marker = CommandLine.arguments[index]
            case "--output" where index + 1 < CommandLine.arguments.count:
                index += 1
                output = CommandLine.arguments[index]
            default:
                throw ProbeError("unknown or incomplete argument: \(CommandLine.arguments[index])")
            }
            index += 1
        }
        guard let marker, let output else {
            throw ProbeError("usage: verify-hosted-window.swift --marker TEXT --output PATH")
        }
        self.marker = marker
        self.output = URL(fileURLWithPath: output)
    }
}

struct ProbeError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func normalize(_ value: String) -> String {
    value.uppercased().filter { $0.isLetter || $0.isNumber }
}

func textEditWindow() async throws -> SCWindow {
    let content = try await SCShareableContent.excludingDesktopWindows(
        false,
        onScreenWindowsOnly: true
    )
    let candidates = content.windows.filter { window in
        window.owningApplication?.applicationName == "TextEdit"
            && window.windowLayer == 0
            && window.frame.width >= 600
            && window.frame.height >= 400
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
    configuration.width = max(1, Int(window.frame.width * 2))
    configuration.height = max(1, Int(window.frame.height * 2))
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
    let window = try await textEditWindow()
    let image = try await captureWindow(window)
    try writePNG(image, to: arguments.output)
    let recognizedText = try recognizeText(in: image)
    let markerRecognized = normalize(recognizedText).contains(normalize(arguments.marker))

    let result: [String: Any] = [
        "schema": "cua-driver/macos-hosted-window-capture@v1",
        "accessibility_trusted": accessibilityTrusted,
        "screen_capture_preflight": screenCapturePreflight,
        "marker_recognized": markerRecognized,
        "recognized_text": recognizedText,
        "image": ["width": image.width, "height": image.height],
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
    if !accessibilityTrusted || !screenCapturePreflight || !markerRecognized {
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
