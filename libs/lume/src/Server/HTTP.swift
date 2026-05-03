import Foundation
import Network

enum HTTPError: Error {
    case internalError
}

struct HTTPRequest {
    let method: String
    let path: String
    let headers: [String: String]
    let body: Data?

    /// Memberwise init used by the NIO handler (components already parsed).
    init(method: String, path: String, headers: [String: String], body: Data?) {
        self.method = method
        self.path = path
        self.headers = headers
        self.body = body
    }

    init?(data: Data) {
        guard let requestString = String(data: data, encoding: .utf8) else { return nil }
        let components = requestString.components(separatedBy: "\r\n\r\n")
        guard components.count >= 1 else { return nil }
        
        let headerLines = components[0].components(separatedBy: "\r\n")
        guard !headerLines.isEmpty else { return nil }
        
        // Parse request line
        let requestLine = headerLines[0].components(separatedBy: " ")
        guard requestLine.count >= 2 else { return nil }
        
        self.method = requestLine[0]
        self.path = requestLine[1]
        
        // Parse headers
        var headers: [String: String] = [:]
        for line in headerLines.dropFirst() {
            let headerComponents = line.split(separator: ":", maxSplits: 1).map(String.init)
            if headerComponents.count == 2 {
                headers[headerComponents[0].trimmingCharacters(in: .whitespaces)] = 
                    headerComponents[1].trimmingCharacters(in: .whitespaces)
            }
        }
        self.headers = headers
        
        // Parse body if present
        if components.count > 1 {
            self.body = components[1].data(using: .utf8)
        } else {
            self.body = nil
        }
    }
}

struct HTTPResponse {
    enum StatusCode: Int {
        case ok = 200
        case accepted = 202
        case badRequest = 400
        case notFound = 404
        case internalServerError = 500
        
        var description: String {
            switch self {
            case .ok: return "OK"
            case .accepted: return "Accepted"
            case .badRequest: return "Bad Request"
            case .notFound: return "Not Found"
            case .internalServerError: return "Internal Server Error"
            }
        }
    }
    
    let statusCode: StatusCode
    let headers: [String: String]
    let body: Data?
    
    init(statusCode: StatusCode, headers: [String: String] = [:], body: Data? = nil) {
        self.statusCode = statusCode
        self.headers = headers
        self.body = body
    }
    
    init(statusCode: StatusCode, body: String) {
        self.statusCode = statusCode
        self.headers = ["Content-Type": "text/plain"]
        self.body = body.data(using: .utf8)
    }
    
    func serialize() -> Data {
        var response = "HTTP/1.1 \(statusCode.rawValue) \(statusCode.description)\r\n"
        
        var headers = self.headers
        if let body = body {
            headers["Content-Length"] = "\(body.count)"
        }
        
        for (key, value) in headers {
            response += "\(key): \(value)\r\n"
        }
        
        response += "\r\n"
        
        var responseData = response.data(using: .utf8) ?? Data()
        if let body = body {
            responseData.append(body)
        }
        
        return responseData
    }
}

final class HTTPServer {
    let port: UInt16
    
    init(port: UInt16) {
        self.port = port
    }
} 