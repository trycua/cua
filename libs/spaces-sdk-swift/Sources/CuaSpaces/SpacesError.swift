import Foundation

/// Everything that can go wrong, on the language's error channel.
///
/// `FRICTION.md` §3: the wire protocol reports a failing tool as a *successful*
/// result carrying `isError: true` and a text part reading `error: …`, so the
/// obvious client reads the error message as if it were the return value. The
/// SDK's single job at that seam is to make that impossible: every tool failure
/// becomes a `throw`, and no SDK call returns a value that might be a failure
/// wearing a value's clothes.
public enum SpacesError: Error, CustomStringConvertible, Sendable, Equatable {

    /// The server could not be started or has gone away.
    case transportUnavailable(String)

    /// The stdio stream produced something that is not a framed JSON-RPC
    /// message. `FRICTION.md` §1 — a desync here is undetectable from the app
    /// side, so the SDK treats any framing anomaly as fatal rather than
    /// guessing.
    case transportFraming(String)

    /// A tool reported failure (`isError: true`, or a JSON-RPC `error`).
    case toolFailed(tool: String, message: String)

    /// A tool succeeded but its payload was not the shape the SDK requires.
    case malformedResponse(tool: String, detail: String)

    /// The tool exists but this provider does not implement it (`FRICTION.md`
    /// §6: several tools are cloud-only and fail with prose saying so).
    case unsupportedByProvider(tool: String, provider: SpaceProvider, detail: String)

    /// No Space with that id, or it is not in a usable state.
    case spaceUnavailable(SpaceID, String)

    /// No such run.
    case runNotFound(RunID)

    /// A caller-declared transfer limit was exceeded before any I/O happened.
    /// `FRICTION.md` §14.
    case limitExceeded(TransferLimits.Violation)

    /// A call that would have created a Space (possibly a metered one) was
    /// refused because the caller did not ask for one. `FRICTION.md` §5, §28, §33.
    case wouldCreate(String)

    /// A local file the caller named does not exist or cannot be read.
    case localFileUnavailable(String)

    /// The requested delivery could not be made within the caller's window.
    ///
    /// Superseded by `timedOut(waitingFor:after:)`, whose type says what was
    /// being awaited. Kept so existing `catch` sites keep compiling.
    case deliveryTimedOut(RunID, String)

    /// A wait ran out of budget. **The type says what was being awaited**, so
    /// a caller never reads prose to find out which clock expired: a delivery,
    /// a state, a Space coming up, a frame arriving.
    case timedOut(waitingFor: Await, after: Duration)

    /// A primitive whose call site exists before the backend does. Thrown
    /// rather than faked, so nothing silently no-ops.
    case notImplementedYet(String)

    /// A teleport was refused, by the host, the manifest gate, or the guest.
    case teleportRefused(String)

    /// What a timed-out call was waiting for.
    public enum Await: Sendable, Hashable, CustomStringConvertible {
        case delivery(RunID)
        case runState(RunID, description: String)
        case spaceReady(SpaceID)
        case transfer(path: String)
        case frame

        public var description: String {
            switch self {
            case let .delivery(id): return "delivery to \(id)"
            case let .runState(id, what): return "\(id) to be \(what)"
            case let .spaceReady(id): return "\(id) to become ready"
            case let .transfer(path): return "transfer of \(path)"
            case .frame: return "a frame"
            }
        }
    }

    public var description: String {
        switch self {
        case let .transportUnavailable(s): return "spaces transport unavailable: \(s)"
        case let .transportFraming(s): return "spaces transport framing: \(s)"
        case let .toolFailed(tool, m): return "spaces tool \(tool) failed: \(m)"
        case let .malformedResponse(tool, d): return "unexpected \(tool) response: \(d)"
        case let .unsupportedByProvider(tool, p, d):
            return "\(tool) is not available on \(p.rawValue) Spaces: \(d)"
        case let .spaceUnavailable(id, why): return "Space \(id) unavailable: \(why)"
        case let .runNotFound(id): return "no such run: \(id)"
        case let .limitExceeded(v): return v.description
        case let .wouldCreate(s): return "refusing to create a Space: \(s)"
        // Worded to match what the server says for the same condition, so a
        // caller that already handles the server's message keeps working when
        // the SDK catches it earlier.
        case let .localFileUnavailable(p): return "host path not found: \(p)"
        case let .deliveryTimedOut(id, s): return "delivery to \(id) timed out: \(s)"
        case let .timedOut(what, after): return "timed out waiting for \(what) after \(after)"
        case let .notImplementedYet(s): return "not implemented yet: \(s)"
        case let .teleportRefused(s): return "teleport refused: \(s)"
        }
    }
}
