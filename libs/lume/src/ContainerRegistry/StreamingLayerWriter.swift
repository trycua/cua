import CryptoKit
import Foundation

struct BodyAdmissionPolicy: Sendable, Equatable {
    let maximumBytes: Int64

    init(maximumBytes: Int64) {
        self.maximumBytes = max(0, maximumBytes)
    }
}

struct StreamingLayerRequest: Sendable {
    let request: URLRequest
    let operationRoot: URL
    let destination: URL
    let expectedDigest: String?
    let admission: BodyAdmissionPolicy
    let onBytes: @Sendable (Int64) -> Void

    init(request: URLRequest, operationRoot: URL, destination: URL, expectedDigest: String? = nil, admission: BodyAdmissionPolicy, onBytes: @escaping @Sendable (Int64) -> Void = { _ in }) {
        self.request = request
        self.operationRoot = operationRoot
        self.destination = destination
        self.expectedDigest = expectedDigest
        self.admission = admission
        self.onBytes = onBytes
    }
}

enum StreamingLayerWriter {
    static func run(
        _ input: StreamingLayerRequest,
        configuration: URLSessionConfiguration = .ephemeral
    ) async throws -> Int64 {
        try Task.checkCancellation()
        let fm = FileManager.default
        do {
            try fm.createDirectory(at: input.operationRoot, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: input.operationRoot.path)
        } catch {
            throw PreparationPlacementError.writeFailed(error.localizedDescription)
        }
        let part = input.operationRoot.appendingPathComponent("layer-\(UUID().uuidString).part")
        let delegate = Delegate(input: input, part: part)
        let session = URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
        let task = session.dataTask(with: input.request)
        defer { session.invalidateAndCancel() }
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                delegate.start(continuation: continuation, task: task)
            }
        } onCancel: {
            task.cancel()
        }
    }

    private final class Delegate: NSObject, URLSessionDataDelegate, @unchecked Sendable {
        private let input: StreamingLayerRequest
        private let part: URL
        private let lock = NSLock()
        private var continuation: CheckedContinuation<Int64, Error>?
        private var handle: FileHandle?
        private var response: HTTPURLResponse?
        private var bytes: Int64 = 0
        private var hasher = SHA256()
        private var terminalError: Error?
        private var settled = false

        init(input: StreamingLayerRequest, part: URL) {
            self.input = input
            self.part = part
        }

        func start(continuation: CheckedContinuation<Int64, Error>, task: URLSessionDataTask) {
            lock.withLock {
                self.continuation = continuation
                do {
                    FileManager.default.createFile(atPath: part.path, contents: nil, attributes: [.posixPermissions: 0o600])
                    handle = try FileHandle(forWritingTo: part)
                } catch {
                    terminalError = PreparationPlacementError.writeFailed(error.localizedDescription)
                }
            }
            task.resume()
        }

        func urlSession(
            _ session: URLSession,
            dataTask: URLSessionDataTask,
            didReceive response: URLResponse,
            completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
        ) {
            let result: PreparationPlacementError? = lock.withLock {
                guard let http = response as? HTTPURLResponse else {
                    terminalError = PreparationPlacementError.responseMissing
                    return .responseMissing
                }
                self.response = http
                guard http.statusCode == 200 else {
                    terminalError = PreparationPlacementError.responseRejected(http.statusCode)
                    return .responseRejected(http.statusCode)
                }
                if http.expectedContentLength >= 0, http.expectedContentLength > input.admission.maximumBytes {
                    terminalError = PreparationPlacementError.contentLengthExceeded(http.expectedContentLength)
                    return .contentLengthExceeded(http.expectedContentLength)
                }
                return nil
            }
            completionHandler(result == nil ? .allow : .cancel)
        }

        func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
            lock.withLock {
                guard terminalError == nil, !settled else { return }
                let next = bytes + Int64(data.count)
                guard next <= input.admission.maximumBytes else {
                    terminalError = PreparationPlacementError.bodyLimitExceeded(next)
                    dataTask.cancel()
                    return
                }
                do {
                    try handle?.write(contentsOf: data)
                    hasher.update(data: data)
                    bytes = next
                    input.onBytes(next)
                } catch {
                    terminalError = PreparationPlacementError.writeFailed(error.localizedDescription)
                    dataTask.cancel()
                }
            }
        }

        func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
            let result: Result<Int64, Error>? = lock.withLock {
                guard !settled else { return nil }
                settled = true
                var finalError = terminalError ?? error.map(Self.mapError)
                do {
                    try handle?.synchronize()
                    try handle?.close()
                    handle = nil
                } catch {
                    finalError = PreparationPlacementError.writeFailed(error.localizedDescription)
                }
                if let finalError { return .failure(finalError) }
                guard response?.statusCode == 200 else {
                    return .failure(PreparationPlacementError.responseMissing)
                }
                if let expected = input.expectedDigest {
                    let actual = "sha256:" + hasher.finalize().map { String(format: "%02x", $0) }.joined()
                    guard actual == expected else {
                        return .failure(PreparationPlacementError.digestMismatch(expected: expected, actual: actual))
                    }
                }
                do {
                    try AtomicLayerCommit.commit(part: part, destination: input.destination)
                    return .success(bytes)
                } catch let placement as PreparationPlacementError {
                    return .failure(placement)
                } catch {
                    return .failure(PreparationPlacementError.commitFailed(error.localizedDescription))
                }
            }
            guard let result else { return }
            try? FileManager.default.removeItem(at: part)
            let continuation = lock.withLock { () -> CheckedContinuation<Int64, Error>? in
                let current = self.continuation
                self.continuation = nil
                return current
            }
            guard let continuation else { return }
            switch result {
            case .success(let bytes): continuation.resume(returning: bytes)
            case .failure(let error): continuation.resume(throwing: error)
            }
        }

        private static func mapError(_ error: Error) -> Error {
            let ns = error as NSError
            if ns.domain == NSURLErrorDomain, ns.code == NSURLErrorCancelled {
                return PreparationPlacementError.cancelled
            }
            return error
        }
    }
}

private enum AtomicLayerCommit {
    static func commit(part: URL, destination: URL) throws {
        let fm = FileManager.default
        let parent = destination.deletingLastPathComponent()
        try fm.createDirectory(at: parent, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        guard sameVolume(part.deletingLastPathComponent(), parent) else {
            throw PreparationPlacementError.destinationVolumeMismatch
        }
        try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: part.path)
        if fm.fileExists(atPath: destination.path) {
            _ = try fm.replaceItemAt(destination, withItemAt: part)
        } else {
            try fm.moveItem(at: part, to: destination)
        }
    }

    private static func sameVolume(_ left: URL, _ right: URL) -> Bool {
        let fm = FileManager.default
        guard let leftNumber = try? fm.attributesOfFileSystem(forPath: left.path)[.systemNumber] as? NSNumber,
            let rightNumber = try? fm.attributesOfFileSystem(forPath: right.path)[.systemNumber] as? NSNumber
        else { return false }
        return leftNumber == rightNumber
    }
}
