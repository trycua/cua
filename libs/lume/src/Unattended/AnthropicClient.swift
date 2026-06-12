import Foundation
import CoreGraphics
import ImageIO

/// Minimal Anthropic API client for computer-use agent loop
struct AnthropicClient {
    let apiKey: String
    let model: String
    let betaFlag: String = "computer-use-2025-11-24"

    private let baseURL = URL(string: "https://api.anthropic.com/v1/messages")!

    struct Message: Codable {
        let role: String
        let content: [ContentBlock]
    }

    enum ContentBlock: Codable {
        case text(String)
        case image(base64: String, mediaType: String)
        case toolUse(id: String, name: String, input: [String: AnyCodable])
        case toolResult(toolUseId: String, content: [ContentBlock], isError: Bool)

        enum CodingKeys: String, CodingKey {
            case type, text, source, id, name, input
            case toolUseId = "tool_use_id"
            case content, isError = "is_error"
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            switch self {
            case .text(let text):
                try container.encode("text", forKey: .type)
                try container.encode(text, forKey: .text)
            case .image(let base64, let mediaType):
                try container.encode("image", forKey: .type)
                let source: [String: String] = [
                    "type": "base64",
                    "media_type": mediaType,
                    "data": base64
                ]
                try container.encode(source, forKey: .source)
            case .toolUse(let id, let name, let input):
                try container.encode("tool_use", forKey: .type)
                try container.encode(id, forKey: .id)
                try container.encode(name, forKey: .name)
                try container.encode(input, forKey: .input)
            case .toolResult(let toolUseId, let content, let isError):
                try container.encode("tool_result", forKey: .type)
                try container.encode(toolUseId, forKey: .toolUseId)
                try container.encode(content, forKey: .content)
                if isError {
                    try container.encode(true, forKey: .isError)
                }
            }
        }

        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            let type = try container.decode(String.self, forKey: .type)
            switch type {
            case "text":
                let text = try container.decode(String.self, forKey: .text)
                self = .text(text)
            case "tool_use":
                let id = try container.decode(String.self, forKey: .id)
                let name = try container.decode(String.self, forKey: .name)
                let input = try container.decode([String: AnyCodable].self, forKey: .input)
                self = .toolUse(id: id, name: name, input: input)
            default:
                self = .text("[unsupported block type: \(type)]")
            }
        }
    }

    struct APIResponse: Decodable {
        let id: String
        let content: [ContentBlock]
        let stopReason: String?

        enum CodingKeys: String, CodingKey {
            case id, content
            case stopReason = "stop_reason"
        }
    }

    struct APIError: Decodable {
        let type: String
        let error: ErrorDetail

        struct ErrorDetail: Decodable {
            let type: String
            let message: String
        }
    }

    @MainActor
    func sendMessage(
        systemPrompt: String,
        messages: [[String: Any]],
        displayWidth: Int,
        displayHeight: Int
    ) async throws -> APIResponse {
        var request = URLRequest(url: baseURL)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.setValue(apiKey, forHTTPHeaderField: "x-api-key")
        request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        request.setValue(betaFlag, forHTTPHeaderField: "anthropic-beta")
        request.timeoutInterval = 120

        let body: [String: Any] = [
            "model": model,
            "max_tokens": 4096,
            "system": systemPrompt,
            "tools": [
                [
                    "type": "computer_20251124",
                    "name": "computer",
                    "display_width_px": displayWidth,
                    "display_height_px": displayHeight
                ] as [String: Any]
            ],
            "messages": messages
        ]

        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response) = try await URLSession.shared.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw AnthropicError.invalidResponse
        }

        if httpResponse.statusCode != 200 {
            if let errorResponse = try? JSONDecoder().decode(APIError.self, from: data) {
                throw AnthropicError.apiError(
                    status: httpResponse.statusCode,
                    message: errorResponse.error.message
                )
            }
            let body = String(data: data, encoding: .utf8) ?? "unknown"
            throw AnthropicError.apiError(status: httpResponse.statusCode, message: body)
        }

        return try JSONDecoder().decode(APIResponse.self, from: data)
    }
}

enum AnthropicError: Error, LocalizedError {
    case invalidResponse
    case apiError(status: Int, message: String)
    case maxIterationsReached

    var errorDescription: String? {
        switch self {
        case .invalidResponse:
            return "Invalid response from Anthropic API"
        case .apiError(let status, let message):
            return "Anthropic API error (\(status)): \(message)"
        case .maxIterationsReached:
            return "Agent reached maximum iteration limit"
        }
    }
}

/// Type-erased Codable wrapper for JSON values
struct AnyCodable: Codable {
    let value: Any

    init(_ value: Any) {
        self.value = value
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let intVal = try? container.decode(Int.self) {
            value = intVal
        } else if let doubleVal = try? container.decode(Double.self) {
            value = doubleVal
        } else if let stringVal = try? container.decode(String.self) {
            value = stringVal
        } else if let boolVal = try? container.decode(Bool.self) {
            value = boolVal
        } else if let arrayVal = try? container.decode([AnyCodable].self) {
            value = arrayVal.map { $0.value }
        } else if let dictVal = try? container.decode([String: AnyCodable].self) {
            value = dictVal.mapValues { $0.value }
        } else {
            value = NSNull()
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch value {
        case let val as Int: try container.encode(val)
        case let val as Double: try container.encode(val)
        case let val as String: try container.encode(val)
        case let val as Bool: try container.encode(val)
        case is NSNull: try container.encodeNil()
        default: try container.encodeNil()
        }
    }

    var stringValue: String? { value as? String }
    var intValue: Int? { value as? Int }
    var doubleValue: Double? { value as? Double }
    var arrayValue: [Any]? { value as? [Any] }
}

// MARK: - CGImage to base64

extension CGImage {
    func pngBase64() -> String? {
        let mutableData = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(mutableData, "public.png" as CFString, 1, nil) else {
            return nil
        }
        CGImageDestinationAddImage(destination, self, nil)
        guard CGImageDestinationFinalize(destination) else {
            return nil
        }
        return (mutableData as Data).base64EncodedString()
    }

    func jpegBase64(quality: Double = 0.8) -> String? {
        let mutableData = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(mutableData, "public.jpeg" as CFString, 1, nil) else {
            return nil
        }
        let options: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: quality]
        CGImageDestinationAddImage(destination, self, options as CFDictionary)
        guard CGImageDestinationFinalize(destination) else {
            return nil
        }
        return (mutableData as Data).base64EncodedString()
    }
}
