// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation

// The value types the stream layer shares, independent of any transport.
//
// The rcdp v1 client (`RCDPConnection`, `RCDPWire`) is gone: the cua SDK's
// `SpaceStreamSession` speaks rcdp wire v2 (tickets, keyframe gating, audio)
// in Rust and hands this layer encoded frames through a `FrameSink`. What stays
// here is what a renderer and an input encoder need: target handles, surface
// geometry, the per-frame descriptor the H.264 decoder keys on, and the
// device-semantic input vocabulary (sent as `interactive_input` batches, which
// v2 accepts unchanged).

/// An opaque stream target (a window handle from the Space's window list).
public struct TargetHandle: Codable, Hashable, Sendable, CustomStringConvertible {
    public var value: String
    public init(_ value: String) { self.value = value }
    public init(from decoder: Decoder) throws { value = try decoder.singleValueContainer().decode(String.self) }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(value)
    }
    public var description: String { value }
}

/// A media session id.
public struct SessionID: Codable, Hashable, Sendable, CustomStringConvertible {
    public var value: String
    public init(_ value: String) { self.value = value }
    public init(from decoder: Decoder) throws { value = try decoder.singleValueContainer().decode(String.self) }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(value)
    }
    public var description: String { value }
}

/// A surface's size in pixels.
public struct SurfaceGeometry: Codable, Hashable, Sendable {
    public var width_px: Int
    public var height_px: Int
    public var scale_factor: Double

    public init(width_px: Int, height_px: Int, scale_factor: Double = 1) {
        self.width_px = width_px
        self.height_px = height_px
        self.scale_factor = scale_factor
    }
}

/// What the decoder needs to know about one encoded access unit.
public struct VideoFrameDescriptor: Sendable, Hashable {
    public var session_id: SessionID
    public var sequence: UInt64
    public var geometry_epoch: UInt64
    public var codec_epoch: UInt64
    public var width_px: Int
    public var height_px: Int
    public var capture_timestamp_us: UInt64
    public var codec: String
    public var keyframe: Bool

    public init(session_id: SessionID, sequence: UInt64, geometry_epoch: UInt64,
                codec_epoch: UInt64, width_px: Int, height_px: Int,
                capture_timestamp_us: UInt64, codec: String, keyframe: Bool) {
        self.session_id = session_id
        self.sequence = sequence
        self.geometry_epoch = geometry_epoch
        self.codec_epoch = codec_epoch
        self.width_px = width_px
        self.height_px = height_px
        self.capture_timestamp_us = capture_timestamp_us
        self.codec = codec
        self.keyframe = keyframe
    }

    /// The descriptor of an SDK frame.
    public init(_ frame: VideoFrame, session: SessionID) {
        self.init(session_id: session, sequence: frame.sequence,
                  geometry_epoch: frame.geometryEpoch, codec_epoch: frame.codecEpoch,
                  width_px: Int(frame.width), height_px: Int(frame.height),
                  capture_timestamp_us: frame.captureTimestampUs, codec: frame.codec,
                  keyframe: frame.keyframe)
    }
}

/// Errors of the stream layer.
public enum StreamError: Error, CustomStringConvertible, Sendable {
    case noStream(String)
    case serverError(code: String, message: String)
    case notConnected
    case encoding

    public var description: String {
        switch self {
        case let .noStream(m): return "no stream available: \(m)"
        case let .serverError(code, message): return "stream error \(code): \(message)"
        case .notConnected: return "no live stream"
        case .encoding: return "could not encode a stream message"
        }
    }
}

// MARK: - Input events

public enum InputModifier: String, Codable, Sendable {
    case command, shift, option, control, function
}

public enum PointerButton: String, Codable, Sendable {
    case left, right, middle
}

public enum PointerPhase: String, Codable, Sendable {
    case move, down, up, cancel
}

public enum GesturePhase: String, Codable, Sendable {
    case none, mayBegin = "may_begin", began, changed, ended, cancelled
}

/// One device-semantic input event.
///
/// Pointer and scroll coordinates are **normalized to `[0, 1]` against the
/// streamed surface**, origin top-left. The server rejects a whole batch when
/// any coordinate falls outside that range or is not finite.
public enum InteractiveInputEvent: Sendable {
    case textCommit(String)
    case key(key: String, down: Bool, modifiers: [InputModifier], repeatKey: Bool)
    case pointer(phase: PointerPhase, button: PointerButton?, x: Double, y: Double, modifiers: [InputModifier])
    case scroll(x: Double, y: Double, deltaX: Double, deltaY: Double, phase: GesturePhase, momentum: GesturePhase, precise: Bool)

    public var json: [String: Any] {
        switch self {
        case let .textCommit(text):
            return ["kind": "text_commit", "text": text]
        case let .key(key, down, modifiers, repeatKey):
            return ["kind": "key", "key": key, "state": down ? "down" : "up",
                    "modifiers": modifiers.map(\.rawValue), "repeat": repeatKey]
        case let .pointer(phase, button, x, y, modifiers):
            var event: [String: Any] = ["kind": "pointer", "phase": phase.rawValue,
                                        "x_normalized": x, "y_normalized": y,
                                        "modifiers": modifiers.map(\.rawValue)]
            event["button"] = button?.rawValue as Any? ?? NSNull()
            return event
        case let .scroll(x, y, dx, dy, phase, momentum, precise):
            return ["kind": "scroll", "x_normalized": x, "y_normalized": y,
                    "delta_x": dx, "delta_y": dy, "phase": phase.rawValue,
                    "momentum_phase": momentum.rawValue, "precise": precise]
        }
    }

    /// Whether this event's coordinates satisfy the server's validator. A batch
    /// containing one invalid event is rejected whole, so the client drops the
    /// offending event rather than losing the batch.
    public var isDispatchable: Bool {
        func ok(_ x: Double, _ y: Double) -> Bool {
            x.isFinite && y.isFinite && (0.0...1.0).contains(x) && (0.0...1.0).contains(y)
        }
        switch self {
        case let .pointer(_, _, x, y, _): return ok(x, y)
        case let .scroll(x, y, dx, dy, _, _, _): return ok(x, y) && dx.isFinite && dy.isFinite
        case let .textCommit(text): return !text.isEmpty
        case let .key(key, _, _, _): return !key.isEmpty
        }
    }
}

/// The `interactive_input` control message for one batch, as the media
/// socket's text frame (`{"direction":"client","message":{…}}`).
public func interactiveInputText(session: SessionID, firstSequence: UInt64,
                                 events: [InteractiveInputEvent]) throws -> String {
    let message: [String: Any] = [
        "type": "interactive_input",
        "payload": ["session_id": session.value, "first_sequence": firstSequence,
                    "events": events.map(\.json)],
    ]
    let header: [String: Any] = ["direction": "client", "message": message]
    let data = try JSONSerialization.data(withJSONObject: header, options: [.sortedKeys])
    guard let text = String(data: data, encoding: .utf8) else { throw StreamError.encoding }
    return text
}
