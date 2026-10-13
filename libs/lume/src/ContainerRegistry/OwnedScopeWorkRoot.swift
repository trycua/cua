import Darwin
import Foundation

enum PreparationPlacementError: Error, LocalizedError, Equatable, Sendable {
    case workRootRequired
    case invalidWorkRoot
    case invalidScopeIdentifier
    case invalidOperationIdentifier
    case incompletePreparationTuple
    case invalidDigest
    case operationExists
    case unsupportedGCSWorkRoot
    case responseRejected(Int)
    case responseMissing
    case contentLengthExceeded(Int64)
    case bodyLimitExceeded(Int64)
    case digestMismatch(expected: String, actual: String)
    case cancelled
    case writeFailed(String)
    case destinationVolumeMismatch
    case commitFailed(String)

    var errorDescription: String? {
        switch self {
        case .workRootRequired: return "owned preparation requires a work root"
        case .invalidWorkRoot: return "preparation work root must be an absolute, non-symlink path"
        case .invalidScopeIdentifier: return "preparation scope identifier is invalid"
        case .invalidOperationIdentifier: return "preparation operation identifier is invalid"
        case .incompletePreparationTuple: return "preparation work root, scope and operation must be supplied together"
        case .invalidDigest: return "layer digest is not a supported content-address value"
        case .operationExists: return "preparation operation already exists"
        case .unsupportedGCSWorkRoot: return "owned preparation work roots are supported for GHCR only"
        case .responseRejected(let status): return "layer download returned HTTP status \(status)"
        case .responseMissing: return "layer download returned no HTTP response"
        case .contentLengthExceeded(let bytes): return "layer content length \(bytes) exceeds the admission limit"
        case .bodyLimitExceeded(let bytes): return "layer body \(bytes) exceeds the admission limit"
        case .digestMismatch(let expected, let actual): return "layer digest mismatch: expected \(expected), got \(actual)"
        case .cancelled: return "layer download cancelled"
        case .writeFailed(let detail): return "layer staging write failed: \(detail)"
        case .destinationVolumeMismatch: return "staged part and destination are on different volumes"
        case .commitFailed(let detail): return "staged part commit failed: \(detail)"
        }
    }
}

struct OwnedPreparation: Sendable, Equatable, Hashable {
    let scopeID: String
    let operationID: String
    let workRoot: OwnedScopeWorkRoot

    init(workRoot: String, scopeID: String, operationID: String) throws {
        self.workRoot = try OwnedScopeWorkRoot(
            scopeID: scopeID,
            root: URL(fileURLWithPath: workRoot, isDirectory: true)
        )
        guard OwnedScopeWorkRoot.isSafeIdentifier(operationID) else {
            throw PreparationPlacementError.invalidOperationIdentifier
        }
        self.scopeID = scopeID
        self.operationID = operationID
    }

    static func make(
        workRoot: String?,
        scopeID: String?,
        operationID: String?
    ) throws -> OwnedPreparation? {
        let values = [workRoot, scopeID, operationID]
        guard values.contains(where: { $0 != nil }) else { return nil }
        guard let workRoot, let scopeID, let operationID else {
            throw PreparationPlacementError.incompletePreparationTuple
        }
        return try OwnedPreparation(workRoot: workRoot, scopeID: scopeID, operationID: operationID)
    }

    var identity: PullPreparationIdentity {
        PullPreparationIdentity(scopeID: scopeID, operationID: operationID, workRoot: workRoot.root.path)
    }
}

struct PullPreparationIdentity: Sendable, Equatable, Hashable, Codable {
    let scopeID: String
    let operationID: String
    let workRoot: String
}

struct ContentAddress: Sendable, Equatable, Hashable {
    let algorithm: String
    let hex: String

    init(_ value: String) throws {
        let parts = value.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false)
        guard parts.count == 2 else { throw PreparationPlacementError.invalidDigest }
        let algorithm = String(parts[0]).lowercased()
        let hex = String(parts[1]).lowercased()
        guard algorithm == "sha256", hex.count == 64,
              hex.utf8.allSatisfy({ ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102) })
        else { throw PreparationPlacementError.invalidDigest }
        self.algorithm = algorithm
        self.hex = hex
    }

    var canonical: String { "\(algorithm):\(hex)" }
    var fileName: String { "\(algorithm)_\(hex)" }
}

struct OwnedScopeWorkRoot: Sendable, Equatable, Hashable {
    let scopeID: String
    let root: URL

    init(scopeID: String, root: URL) throws {
        guard Self.isSafeIdentifier(scopeID) else {
            throw PreparationPlacementError.invalidScopeIdentifier
        }
        guard root.isFileURL, root.path.hasPrefix("/"), root.path != "/" else {
            throw PreparationPlacementError.invalidWorkRoot
        }
        let standardized = root.standardizedFileURL
        guard standardized.resolvingSymlinksInPath().path == standardized.path else {
            throw PreparationPlacementError.invalidWorkRoot
        }
        self.scopeID = scopeID
        self.root = standardized
    }

    func operationRoot(_ operationID: String) throws -> URL {
        guard Self.isSafeIdentifier(operationID) else {
            throw PreparationPlacementError.invalidOperationIdentifier
        }
        let fm = FileManager.default
        try fm.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        let operations = root.appendingPathComponent("operations", isDirectory: true)
        try fm.createDirectory(at: operations, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: operations.path)
        let operation = operations.appendingPathComponent(operationID, isDirectory: true)
        guard !fm.fileExists(atPath: operation.path) else {
            throw PreparationPlacementError.operationExists
        }
        try fm.createDirectory(at: operation, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        return operation
    }

    static func isSafeIdentifier(_ value: String) -> Bool {
        !value.isEmpty && value.utf8.allSatisfy { ($0 >= 48 && $0 <= 57) || ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122) || $0 == 45 || $0 == 95 || $0 == 46 }
    }
}

struct StagingLayout: Sendable, Equatable {
    let operationRoot: URL
    let wholeVM: URL
    let legacyDownload: URL
    let ociTemporary: URL
    let cacheReassembly: URL
    let gunzipRaw: URL
    let finalAssembly: URL

    init(operationRoot: URL) throws {
        let fm = FileManager.default
        let names = ["whole-vm", "legacy-download", "oci-temporary", "cache-reassembly", "gunzip-raw", "final-assembly"]
        let urls = try names.map { name -> URL in
            let url = operationRoot.appendingPathComponent(name, isDirectory: true)
            try fm.createDirectory(at: url, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
            return url
        }
        self.operationRoot = operationRoot
        wholeVM = urls[0]
        legacyDownload = urls[1]
        ociTemporary = urls[2]
        cacheReassembly = urls[3]
        gunzipRaw = urls[4]
        finalAssembly = urls[5]
    }
}

final class DigestFileLock: @unchecked Sendable {
    private let descriptor: Int32
    private let stateLock = NSLock()
    private var released = false

    init(path: URL) throws {
        descriptor = Darwin.open(path.path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
        guard descriptor >= 0 else { throw PreparationPlacementError.writeFailed(String(cString: strerror(errno))) }
    }

    func acquire() async throws {
        while true {
            try Task.checkCancellation()
            if flock(descriptor, LOCK_EX | LOCK_NB) == 0 { return }
            if errno != EWOULDBLOCK && errno != EAGAIN {
                throw PreparationPlacementError.writeFailed(String(cString: strerror(errno)))
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    func release() {
        guard stateLock.withLock({
            if released { return false }
            released = true
            return true
        }) else { return }
        _ = flock(descriptor, LOCK_UN)
        _ = Darwin.close(descriptor)
    }

    deinit { release() }
}
