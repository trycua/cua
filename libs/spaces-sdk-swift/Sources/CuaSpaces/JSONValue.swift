import Foundation

/// A `Sendable` JSON tree.
///
/// The MCP binding this SDK replaces passed `[String: Any]` across every seam.
/// That is not `Sendable`, so an `async` client built on it either fights the
/// concurrency checker or lies to it with `@unchecked`. `JSONValue` is the
/// smallest type that lets the whole SDK be `async` *and* safe to call from
/// `@MainActor` — see `FRICTION.md` §30.
public enum JSONValue: Sendable, Hashable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    // MARK: Reading

    public var boolValue: Bool? {
        if case let .bool(b) = self { return b }
        return nil
    }

    public var doubleValue: Double? {
        if case let .number(n) = self { return n }
        return nil
    }

    public var intValue: Int? {
        if case let .number(n) = self { return Int(n) }
        return nil
    }

    public var stringValue: String? {
        if case let .string(s) = self { return s }
        return nil
    }

    public var arrayValue: [JSONValue]? {
        if case let .array(a) = self { return a }
        return nil
    }

    public var objectValue: [String: JSONValue]? {
        if case let .object(o) = self { return o }
        return nil
    }

    public var isNull: Bool { self == .null }

    public subscript(key: String) -> JSONValue? { objectValue?[key] }

    // MARK: Bridging

    /// Build from the output of `JSONSerialization.jsonObject`.
    public init(any: Any) {
        switch any {
        case is NSNull:
            self = .null
        case let n as NSNumber:
            // `Bool` and `Int` are both `NSNumber` on Darwin; only the
            // ObjC type encoding separates them.
            if CFGetTypeID(n) == CFBooleanGetTypeID() { self = .bool(n.boolValue) }
            else { self = .number(n.doubleValue) }
        case let s as String:
            self = .string(s)
        case let a as [Any]:
            self = .array(a.map(JSONValue.init(any:)))
        case let d as [String: Any]:
            self = .object(d.mapValues(JSONValue.init(any:)))
        default:
            self = .string("\(any)")
        }
    }

    /// Convert back for `JSONSerialization.data`.
    public var foundationObject: Any {
        switch self {
        case .null: return NSNull()
        case let .bool(b): return b
        case let .number(n): return n == n.rounded() && abs(n) < 9e15 ? Int(n) : n
        case let .string(s): return s
        case let .array(a): return a.map(\.foundationObject)
        case let .object(o): return o.mapValues(\.foundationObject)
        }
    }

    /// Parse a JSON document, returning `nil` when the bytes are not JSON.
    /// Used to tell a tool that answered with JSON from one that answered with
    /// prose — see `FRICTION.md` §4.
    public static func parse(_ text: String) -> JSONValue? {
        guard let data = text.data(using: .utf8),
              let any = try? JSONSerialization.jsonObject(
                with: data, options: [.fragmentsAllowed]) else { return nil }
        return JSONValue(any: any)
    }
}

extension JSONValue: ExpressibleByStringLiteral {
    public init(stringLiteral value: String) { self = .string(value) }
}

extension JSONValue: ExpressibleByBooleanLiteral {
    public init(booleanLiteral value: Bool) { self = .bool(value) }
}

extension JSONValue: ExpressibleByIntegerLiteral {
    public init(integerLiteral value: Int) { self = .number(Double(value)) }
}

extension JSONValue: CustomStringConvertible {
    public var description: String {
        switch self {
        case .null: return "null"
        case let .bool(b): return "\(b)"
        case let .number(n): return n == n.rounded() && abs(n) < 9e15 ? "\(Int(n))" : "\(n)"
        case let .string(s): return s
        case .array, .object:
            let obj = foundationObject
            guard JSONSerialization.isValidJSONObject(obj),
                  let d = try? JSONSerialization.data(withJSONObject: obj),
                  let s = String(data: d, encoding: .utf8) else { return "\(obj)" }
            return s
        }
    }
}
