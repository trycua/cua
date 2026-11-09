import Foundation

/// Where an upload should land.
///
/// `FRICTION.md` §13: *"`upload` overwrites its destination silently … There is
/// no create-only mode, no 'exists' check that is not a `space_bash` round
/// trip, and no returned path. Every attachment therefore gets a UUID-prefixed
/// remote name minted client-side, which is then the name the Bot sees — so the
/// user's filename is mangled to work around a missing flag."*
///
/// The SDK offers the non-clobbering mode as a first-class choice, keeps the
/// user's filename intact by allocating a unique *directory* rather than
/// mangling the name, and always reports the path it actually wrote.
public enum UploadPlacement: Sendable, Hashable {
    /// Exactly this path. May overwrite — chosen deliberately, spelled out.
    case exactPath(String)

    /// Inside this directory under the user's own filename, in a freshly
    /// minted uniquely-named subdirectory so two files called `notes.txt` from
    /// two different folders stay two files and the Bot still sees `notes.txt`.
    case collisionSafe(in: String)

    /// `collisionSafe` in the provider's default upload directory.
    public static let collisionSafeDefault = UploadPlacement.collisionSafe(in: "")

    /// Inside this directory under the user's filename, overwriting anything
    /// already there. The old client's behaviour, available but named.
    case clobbering(in: String)
}

/// A file that now exists inside the Space, at the path the SDK actually wrote.
public struct RemoteFile: Sendable, Hashable {
    public let path: String
    public let name: String
    public let byteCount: Int?

    public init(path: String, name: String, byteCount: Int?) {
        self.path = path
        self.name = name
        self.byteCount = byteCount
    }
}

/// Caller-declared transfer limits.
///
/// `FRICTION.md` §14: *"The app's caps (6 attachments, 25 MB per file,
/// 200 MB per message) are a product contract with nothing behind it in the
/// API. `upload` takes one file and has no notion of a message, so the count
/// cap and the total cap have no server counterpart at all."*
///
/// The SDK cannot invent a server-side cap. What it can do — and what §14 asks
/// for — is carry the caps as **data**, check all three before any I/O, and
/// name the file that broke the rule, so a policy change is a value change
/// rather than a new app build.
public struct TransferLimits: Sendable, Hashable, Codable {
    public var maxFileCount: Int
    public var maxBytesPerFile: Int
    public var maxBytesPerBatch: Int

    /// Whether the **server** published these numbers.
    ///
    /// `false` everywhere today, and that is not a placeholder: the only cap
    /// the backend enforces is `MAX_TRANSFER_BYTES`, a hard-coded 25 MB
    /// constant at `spaces_mcp.py:62`. There is no limits tool, so a count cap
    /// and a batch cap have no server counterpart at all and two clients will
    /// drift. Render this in a debug surface; do not hide it.
    public var isServerPublished: Bool

    public init(maxFileCount: Int, maxBytesPerFile: Int, maxBytesPerBatch: Int,
                isServerPublished: Bool = false) {
        self.maxFileCount = maxFileCount
        self.maxBytesPerFile = maxBytesPerFile
        self.maxBytesPerBatch = maxBytesPerBatch
        self.isServerPublished = isServerPublished
    }

    /// The one cap the backend actually enforces, plus SDK-side defaults for
    /// the two it does not. `isServerPublished` is `false` because only the
    /// per-file number has anything behind it.
    public static let conservativeDefault = TransferLimits(
        maxFileCount: 6,
        maxBytesPerFile: 25 * 1024 * 1024,   // spaces_mcp.py:62, MAX_TRANSFER_BYTES
        maxBytesPerBatch: 200 * 1_000_000,
        isServerPublished: false)

    /// No limits. The SDK's default, because the SDK does not know the
    /// product's policy and will not invent one.
    public static let none = TransferLimits(
        maxFileCount: .max, maxBytesPerFile: .max, maxBytesPerBatch: .max)

    /// The caps OpenKoalaBots's product contract documents, as an example of the
    /// shape a caller supplies.
    public static let koalaBotsAttachments = TransferLimits(
        maxFileCount: 6, maxBytesPerFile: 25 * 1_000_000, maxBytesPerBatch: 200 * 1_000_000)

    public enum Violation: Sendable, Hashable, CustomStringConvertible, Equatable {
        case tooManyFiles(count: Int, limit: Int)
        case fileTooLarge(name: String, bytes: Int, limit: Int)
        case batchTooLarge(bytes: Int, limit: Int)

        public var description: String {
            switch self {
            case let .tooManyFiles(c, l): return "\(c) files exceeds the limit of \(l)"
            case let .fileTooLarge(n, b, l): return "\(n) is \(b) bytes, over the \(l) byte limit"
            case let .batchTooLarge(b, l): return "\(b) bytes total, over the \(l) byte limit"
            }
        }
    }

    // MARK: - Partial admission

    /// One candidate file, named and sized, with no filesystem behind it — so
    /// the failure state can be tested exactly as the user meets it.
    public struct Candidate: Sendable, Hashable {
        public var name: String
        public var byteCount: Int
        public init(name: String, byteCount: Int) {
            self.name = name
            self.byteCount = byteCount
        }
        public init(_ url: URL) {
            self.init(name: url.lastPathComponent,
                      byteCount: (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
    }

    /// What `admit` decided, per file.
    ///
    /// `check` is the batch verdict: all or nothing, before any I/O. `admit` is
    /// the *interactive* verdict, and they are genuinely different questions.
    /// Dropping seven files onto a six-attachment surface should attach six and
    /// say why the seventh did not go, not refuse all seven — and that rule,
    /// along with the order the caps are tested in, is policy an app should not
    /// be re-deriving. It lived in OpenKoalaBots's drop target; the gesture is
    /// UI, this is not.
    public struct Admission: Sendable, Hashable {
        public struct Rejection: Sendable, Hashable {
            public var candidate: Candidate
            public var violation: Violation
            public init(candidate: Candidate, violation: Violation) {
                self.candidate = candidate
                self.violation = violation
            }
        }

        public var accepted: [Candidate] = []
        public var rejected: [Rejection] = []

        public init(accepted: [Candidate] = [], rejected: [Rejection] = []) {
            self.accepted = accepted
            self.rejected = rejected
        }
    }

    /// Admit what fits and say precisely why the rest does not.
    ///
    /// The order of the checks is load-bearing: per-file size is tested before
    /// the running total, so one oversized file is reported as oversized rather
    /// than as "the batch is too big" — which would point the user at the wrong
    /// file to remove.
    ///
    /// - Parameter existing: what is already queued, so count and total carry
    ///   across drops.
    public func admit(_ candidates: [Candidate],
                      existing: [Candidate] = []) -> Admission {
        var result = Admission()
        var count = existing.count
        var total = existing.reduce(0) { $0 + $1.byteCount }

        for candidate in candidates {
            if count >= maxFileCount {
                result.rejected.append(.init(
                    candidate: candidate,
                    violation: .tooManyFiles(count: count + 1, limit: maxFileCount)))
                continue
            }
            if candidate.byteCount > maxBytesPerFile {
                result.rejected.append(.init(
                    candidate: candidate,
                    violation: .fileTooLarge(name: candidate.name,
                                             bytes: candidate.byteCount,
                                             limit: maxBytesPerFile)))
                continue
            }
            if total + candidate.byteCount > maxBytesPerBatch {
                result.rejected.append(.init(
                    candidate: candidate,
                    violation: .batchTooLarge(bytes: total + candidate.byteCount,
                                              limit: maxBytesPerBatch)))
                continue
            }
            result.accepted.append(candidate)
            count += 1
            total += candidate.byteCount
        }
        return result
    }

    /// Check every rule **before any I/O**, which is the only honest order:
    /// half a batch uploaded and then refused is worse than refused.
    public func check(_ files: [URL]) throws {
        if files.count > maxFileCount {
            throw SpacesError.limitExceeded(
                .tooManyFiles(count: files.count, limit: maxFileCount))
        }
        var total = 0
        for url in files {
            let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? {
                (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? nil
            }()
            guard let size else {
                throw SpacesError.localFileUnavailable(url.path)
            }
            if size > maxBytesPerFile {
                throw SpacesError.limitExceeded(
                    .fileTooLarge(name: url.lastPathComponent, bytes: size, limit: maxBytesPerFile))
            }
            total += size
        }
        if total > maxBytesPerBatch {
            throw SpacesError.limitExceeded(
                .batchTooLarge(bytes: total, limit: maxBytesPerBatch))
        }
    }
}
