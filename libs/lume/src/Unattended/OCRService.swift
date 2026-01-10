import Foundation
import Vision
import CoreGraphics

/// Represents a text observation from OCR
struct TextObservation: Sendable {
    /// The recognized text string
    let text: String

    /// Bounding box in normalized coordinates (0-1), origin at bottom-left (Vision convention)
    let boundingBox: CGRect

    /// Confidence score (0-1)
    let confidence: Float

    /// Convert bounding box to screen coordinates
    /// - Parameters:
    ///   - screenWidth: The actual screen width in pixels
    ///   - screenHeight: The actual screen height in pixels
    /// - Returns: CGRect in screen coordinates with origin at top-left
    func screenRect(screenWidth: CGFloat, screenHeight: CGFloat) -> CGRect {
        // Vision uses bottom-left origin, screen uses top-left
        // Also need to flip Y axis
        let x = boundingBox.origin.x * screenWidth
        let y = (1 - boundingBox.origin.y - boundingBox.height) * screenHeight
        let width = boundingBox.width * screenWidth
        let height = boundingBox.height * screenHeight

        return CGRect(x: x, y: y, width: width, height: height)
    }

    /// Get the center point in screen coordinates
    func centerPoint(screenWidth: CGFloat, screenHeight: CGFloat) -> CGPoint {
        let rect = screenRect(screenWidth: screenWidth, screenHeight: screenHeight)
        return CGPoint(x: rect.midX, y: rect.midY)
    }
}

/// Protocol for OCR services
protocol OCRServiceProtocol: Sendable {
    /// Recognize text in an image
    /// - Parameter image: The CGImage to analyze
    /// - Returns: Array of text observations found in the image
    func recognizeText(in image: CGImage) async throws -> [TextObservation]

    /// Find text matching a pattern
    /// - Parameters:
    ///   - pattern: The text to search for (supports regex)
    ///   - image: The image to search in
    /// - Returns: The first matching text observation, or nil if not found
    func findText(_ pattern: String, in image: CGImage) async throws -> TextObservation?

    /// Find all text occurrences matching a pattern
    /// - Parameters:
    ///   - pattern: The text to search for (supports regex)
    ///   - image: The image to search in
    /// - Returns: All matching text observations, sorted by vertical position (top to bottom)
    func findAllText(_ pattern: String, in image: CGImage) async throws -> [TextObservation]
}

/// Default OCR service using Apple Vision framework
final class VisionOCRService: OCRServiceProtocol, @unchecked Sendable {
    /// Recognition level for text detection
    enum RecognitionLevel {
        case fast
        case accurate
    }

    private let recognitionLevel: RecognitionLevel
    private let minimumConfidence: Float

    init(recognitionLevel: RecognitionLevel = .accurate, minimumConfidence: Float = 0.3) {
        self.recognitionLevel = recognitionLevel
        self.minimumConfidence = minimumConfidence
    }

    func recognizeText(in image: CGImage) async throws -> [TextObservation] {
        try await withCheckedThrowingContinuation { continuation in
            let request = VNRecognizeTextRequest { request, error in
                if let error = error {
                    continuation.resume(throwing: UnattendedError.ocrFailed(error.localizedDescription))
                    return
                }

                guard let observations = request.results as? [VNRecognizedTextObservation] else {
                    continuation.resume(returning: [])
                    return
                }

                let textObservations = observations.compactMap { observation -> TextObservation? in
                    guard let topCandidate = observation.topCandidates(1).first else {
                        return nil
                    }

                    // Filter by confidence
                    guard topCandidate.confidence >= self.minimumConfidence else {
                        return nil
                    }

                    return TextObservation(
                        text: topCandidate.string,
                        boundingBox: observation.boundingBox,
                        confidence: topCandidate.confidence
                    )
                }

                continuation.resume(returning: textObservations)
            }

            // Configure recognition level
            switch recognitionLevel {
            case .fast:
                request.recognitionLevel = .fast
            case .accurate:
                request.recognitionLevel = .accurate
            }

            // Use revision that supports latest features
            request.revision = VNRecognizeTextRequestRevision3

            // Enable automatic language detection
            request.automaticallyDetectsLanguage = true

            // Perform the request
            let handler = VNImageRequestHandler(cgImage: image, options: [:])
            do {
                try handler.perform([request])
            } catch {
                continuation.resume(throwing: UnattendedError.ocrFailed(error.localizedDescription))
            }
        }
    }

    func findText(_ pattern: String, in image: CGImage) async throws -> TextObservation? {
        let matches = try await findAllText(pattern, in: image)
        return matches.first
    }

    func findAllText(_ pattern: String, in image: CGImage) async throws -> [TextObservation] {
        let observations = try await recognizeText(in: image)
        var matches: [TextObservation] = []

        // Check if pattern looks like a regex (contains special characters)
        let regexSpecialChars = CharacterSet(charactersIn: "^$.*+?[]{}()|\\")
        let isRegexPattern = pattern.unicodeScalars.contains { regexSpecialChars.contains($0) }

        if isRegexPattern {
            // Try regex match for patterns with special characters
            if let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]) {
                for observation in observations {
                    let range = NSRange(observation.text.startIndex..., in: observation.text)
                    if regex.firstMatch(in: observation.text, range: range) != nil {
                        matches.append(observation)
                    }
                }
            }
        } else {
            // First, try exact match (case-insensitive) - this prevents "Agree" matching "Disagree"
            let exactMatches = observations.filter {
                $0.text.localizedCaseInsensitiveCompare(pattern) == .orderedSame
            }

            if !exactMatches.isEmpty {
                matches = exactMatches
            } else {
                // Fallback to substring match (case-insensitive)
                matches = observations.filter {
                    $0.text.localizedCaseInsensitiveContains(pattern)
                }

                // Final fallback: try regex match if no substring matches
                if matches.isEmpty {
                    if let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]) {
                        for observation in observations {
                            let range = NSRange(observation.text.startIndex..., in: observation.text)
                            if regex.firstMatch(in: observation.text, range: range) != nil {
                                matches.append(observation)
                            }
                        }
                    }
                }
            }
        }

        // Sort by vertical position (top to bottom)
        // Note: Vision uses bottom-left origin, so higher Y = lower on screen
        // We want top-to-bottom, so sort by descending Y
        matches.sort { $0.boundingBox.origin.y > $1.boundingBox.origin.y }

        return matches
    }
}
