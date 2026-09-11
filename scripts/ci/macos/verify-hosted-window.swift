import ApplicationServices
import CoreGraphics
import Foundation
import ImageIO
import Vision

struct ProbeError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func normalize(_ value: String) -> String {
    value.uppercased().filter { $0.isLetter || $0.isNumber }
}

func textEditWindow() throws -> [String: Any] {
    guard let windows = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw ProbeError("CoreGraphics did not return an on-screen window list")
    }
    let candidates = windows.filter { window in
        guard window[kCGWindowOwnerName as String] as? String == "TextEdit",
              window[kCGWindowName as String] as? String == "probe.txt",
              window[kCGWindowLayer as String] as? Int == 0,
              let bounds = window[kCGWindowBounds as String] as? [String: Any],
              let width = bounds["Width"] as? Double,
              let height = bounds["Height"] as? Double else {
            return false
        }
        return width >= 400 && height >= 250
    }
    guard let window = candidates.first,
          let windowID = window[kCGWindowNumber as String] as? UInt32,
          let bounds = window[kCGWindowBounds as String] as? [String: Any] else {
        throw ProbeError("no deterministic on-screen TextEdit probe.txt window was found")
    }
    return [
        "schema": "cua-driver/macos-hosted-window@v1",
        "id": windowID,
        "owner": "TextEdit",
        "name": "probe.txt",
        "x": bounds["X"] as? Double ?? 0,
        "y": bounds["Y"] as? Double ?? 0,
        "width": bounds["Width"] as? Double ?? 0,
        "height": bounds["Height"] as? Double ?? 0,
    ]
}

func loadImage(at path: String) throws -> CGImage {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw ProbeError("could not read captured image: \(path)")
    }
    return image
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

func emit(_ object: [String: Any]) throws {
    let encoded = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
    FileHandle.standardOutput.write(encoded)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func locate() throws {
    try emit(textEditWindow())
}

func verify(arguments: [String]) throws {
    var values: [String: String] = [:]
    var index = 0
    while index < arguments.count {
        guard index + 1 < arguments.count, arguments[index].hasPrefix("--") else {
            throw ProbeError("verification arguments must be --name value pairs")
        }
        values[arguments[index]] = arguments[index + 1]
        index += 2
    }
    guard let marker = values["--marker"],
          let windowInput = values["--window-input"],
          let displayInput = values["--display-input"],
          let metadataInput = values["--window-metadata"] else {
        throw ProbeError(
            "usage: verify-hosted-window.swift --verify --marker TEXT --window-input PATH "
                + "--display-input PATH --window-metadata PATH"
        )
    }

    let metadataData = try Data(contentsOf: URL(fileURLWithPath: metadataInput))
    guard let window = try JSONSerialization.jsonObject(with: metadataData) as? [String: Any] else {
        throw ProbeError("window metadata is not a JSON object")
    }
    let windowImage = try loadImage(at: windowInput)
    let displayImage = try loadImage(at: displayInput)
    let windowText = try recognizeText(in: windowImage)
    let displayText = try recognizeText(in: displayImage)
    let windowMarkerRecognized = normalize(windowText).contains(normalize(marker))
    let displayMarkerRecognized = normalize(displayText).contains(normalize(marker))
    let accessibilityTrusted = AXIsProcessTrusted()
    let screenCapturePreflight = CGPreflightScreenCaptureAccess()

    let result: [String: Any] = [
        "schema": "cua-driver/macos-hosted-capture-verification@v1",
        "accessibility_trusted": accessibilityTrusted,
        "screen_capture_preflight": screenCapturePreflight,
        "permission_attribution_scope": "probe process only; does not establish CuaDriverLocal.app TCC",
        "probe_executable": Bundle.main.executableURL?.path ?? CommandLine.arguments[0],
        "probe_process_id": ProcessInfo.processInfo.processIdentifier,
        "marker_recognized": windowMarkerRecognized,
        "recognized_text": windowText,
        "image": ["width": windowImage.width, "height": windowImage.height],
        "display_capture": [
            "marker_recognized": displayMarkerRecognized,
            "recognized_text": displayText,
            "image_width": displayImage.width,
            "image_height": displayImage.height,
        ],
        "window": window,
    ]
    try emit(result)
    if !accessibilityTrusted || !screenCapturePreflight
        || !windowMarkerRecognized || !displayMarkerRecognized {
        throw ProbeError("permission preflight or marker recognition failed")
    }
}

do {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard let mode = arguments.first else {
        throw ProbeError("expected --locate or --verify")
    }
    switch mode {
    case "--locate":
        try locate()
    case "--verify":
        try verify(arguments: Array(arguments.dropFirst()))
    default:
        throw ProbeError("unknown mode: \(mode)")
    }
} catch {
    FileHandle.standardError.write(Data("hosted macOS window verification failed: \(error)\n".utf8))
    exit(1)
}
