import Foundation

struct APIError: Codable {
    let message: String
}

// Helper struct to encode mixed-type dictionaries
struct AnyEncodable: Encodable {
    private let value: Encodable

    init(_ value: Encodable) {
        self.value = value
    }

    func encode(to encoder: Encoder) throws {
        try value.encode(to: encoder)
    }
}

extension HTTPResponse {
    static func json<T: Encodable>(_ value: T) throws -> HTTPResponse {
        let data = try JSONEncoder().encode(value)
        return HTTPResponse(
            statusCode: .ok,
            headers: ["Content-Type": "application/json"],
            body: data
        )
    }
    
    static func badRequest(message: String) -> HTTPResponse {
        let error = APIError(message: message)
        return try! HTTPResponse(
            statusCode: .badRequest,
            headers: ["Content-Type": "application/json"],
            body: JSONEncoder().encode(error)
        )
    }
}