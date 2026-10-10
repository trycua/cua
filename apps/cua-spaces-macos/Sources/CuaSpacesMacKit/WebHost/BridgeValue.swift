// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The core's views (UniFFI records and enums) as values a WebKit reply can
/// carry: `NSString`, `NSNumber`, `NSArray`, `NSDictionary` and `NSNull`.
///
/// The bridge hands the web UI exactly what the SwiftUI views render, so it
/// encodes by reflection rather than keeping a second, hand-written copy of
/// every record. Records become objects with their Swift (camelCase) field
/// names; an enum without a payload becomes its case name (`"touchId"`);
/// one with a payload becomes `{"type": "<case>", ...fields}` (an unlabeled
/// payload is `value`); `Data` becomes base64. Objects (UniFFI handles,
/// classes) are left out as `null`: they are live handles, not data.
enum BridgeValue {
    static func encode(_ value: Any?) -> Any {
        guard let value else { return NSNull() }
        switch value {
        case let v as String: return v
        case let v as Bool: return v
        case let v as Int: return v
        case let v as Int8: return Int(v)
        case let v as Int16: return Int(v)
        case let v as Int32: return Int(v)
        case let v as Int64: return NSNumber(value: v)
        case let v as UInt: return NSNumber(value: v)
        case let v as UInt8: return Int(v)
        case let v as UInt16: return Int(v)
        case let v as UInt32: return NSNumber(value: v)
        case let v as UInt64: return NSNumber(value: v)
        case let v as Double: return v.isFinite ? v : NSNull()
        case let v as Float: return v.isFinite ? Double(v) : NSNull()
        case let v as Data: return v.base64EncodedString()
        case let v as Date: return v.timeIntervalSince1970 * 1000
        case let v as URL: return v.absoluteString
        case is NSNull: return NSNull()
        default: break
        }
        let mirror = Mirror(reflecting: value)
        switch mirror.displayStyle {
        case .optional:
            return mirror.children.first.map { encode($0.value) } ?? NSNull()
        case .collection, .set:
            return mirror.children.map { encode($0.value) }
        case .dictionary:
            var out: [String: Any] = [:]
            for child in mirror.children {
                let pair = Array(Mirror(reflecting: child.value).children)
                guard pair.count == 2 else { continue }
                out[String(describing: pair[0].value)] = encode(pair[1].value)
            }
            return out
        case .struct:
            return fields(mirror)
        case .tuple:
            return fields(mirror)
        case .enum:
            guard let payload = mirror.children.first else { return String(describing: value) }
            var out: [String: Any] = ["type": payload.label ?? String(describing: value)]
            let inner = Mirror(reflecting: payload.value)
            if inner.displayStyle == .tuple {
                for (k, v) in fields(inner) { out[k] = v }
            } else {
                out["value"] = encode(payload.value)
            }
            return out
        default:
            return NSNull()
        }
    }

    /// A record's or a tuple's fields (`.0` and the like become `value`,
    /// `value1`, ...).
    private static func fields(_ mirror: Mirror) -> [String: Any] {
        var out: [String: Any] = [:]
        var unlabeled = 0
        for child in mirror.children {
            var key = child.label ?? ".\(unlabeled)"
            if key.hasPrefix(".") {
                key = unlabeled == 0 ? "value" : "value\(unlabeled)"
                unlabeled += 1
            }
            out[key] = encode(child.value)
        }
        return out
    }
}
