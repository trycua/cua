// SPDX-License-Identifier: MIT

import Foundation

/// Errors from a single layer download attempt.
enum LayerDownloadError: Error, LocalizedError {
    case badStatus(Int)
    case noResponse

    var errorDescription: String? {
        switch self {
        case .badStatus(let code): return "Layer download failed with HTTP status \(code)"
        case .noResponse: return "Layer download returned no HTTP response"
        }
    }
}

/// One blob download on its own URLSession, reporting bytes as they arrive.
///
/// `session.download(for:)` only returns once the whole file is there, so it
/// cannot report progress. This uses a download task with a session delegate:
/// `didWriteData` reports the running byte count, `didFinishDownloadingTo`
/// moves the finished file to its destination (the system deletes its temp
/// file right after that callback), and Task cancellation cancels the URL
/// task, which stops the transfer and removes URLSession's temp file.
enum LayerDownload {
    /// Downloads `request` to `destination` and returns the number of bytes written.
    static func run(
        request: URLRequest,
        configuration: URLSessionConfiguration,
        to destination: URL,
        onBytes: @escaping @Sendable (Int64) -> Void
    ) async throws -> Int64 {
        try Task.checkCancellation()
        let delegate = Delegate(destination: destination, onBytes: onBytes)
        let session = URLSession(
            configuration: configuration, delegate: delegate, delegateQueue: nil)
        // The session keeps its delegate alive until it is invalidated.
        defer { session.invalidateAndCancel() }
        let task = session.downloadTask(with: request)

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                delegate.start(continuation: continuation, task: task)
            }
        } onCancel: {
            task.cancel()
        }
    }

    private final class Delegate: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
        private let destination: URL
        private let onBytes: @Sendable (Int64) -> Void
        private let lock = NSLock()
        private var continuation: CheckedContinuation<Int64, Error>?
        private var moveError: Error?
        private var moved = false
        private var bytesWritten: Int64 = 0

        init(destination: URL, onBytes: @escaping @Sendable (Int64) -> Void) {
            self.destination = destination
            self.onBytes = onBytes
        }

        func start(continuation: CheckedContinuation<Int64, Error>, task: URLSessionDownloadTask) {
            lock.withLock { self.continuation = continuation }
            task.resume()
        }

        private func statusCode(of task: URLSessionTask) -> Int? {
            (task.response as? HTTPURLResponse)?.statusCode
        }

        func urlSession(
            _ session: URLSession, downloadTask: URLSessionDownloadTask,
            didWriteData bytesWritten: Int64, totalBytesWritten: Int64,
            totalBytesExpectedToWrite: Int64
        ) {
            // Only a successful response carries blob bytes.
            guard statusCode(of: downloadTask) == 200 else { return }
            lock.withLock { self.bytesWritten = totalBytesWritten }
            onBytes(totalBytesWritten)
        }

        func urlSession(
            _ session: URLSession, downloadTask: URLSessionDownloadTask,
            didFinishDownloadingTo location: URL
        ) {
            guard statusCode(of: downloadTask) == 200 else { return }
            let fm = FileManager.default
            do {
                if fm.fileExists(atPath: destination.path) {
                    try fm.removeItem(at: destination)
                }
                try fm.moveItem(at: location, to: destination)
                let size =
                    (try? fm.attributesOfItem(atPath: destination.path)[.size] as? NSNumber)?
                    .int64Value
                lock.withLock {
                    moved = true
                    if let size { bytesWritten = size }
                }
            } catch {
                lock.withLock { moveError = error }
            }
        }

        func urlSession(
            _ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?
        ) {
            let (continuation, moveError, moved, written) = lock.withLock {
                let c = self.continuation
                self.continuation = nil
                return (c, self.moveError, self.moved, self.bytesWritten)
            }
            guard let continuation else { return }
            if let error {
                if (error as NSError).domain == NSURLErrorDomain
                    && (error as NSError).code == NSURLErrorCancelled
                {
                    continuation.resume(throwing: CancellationError())
                } else {
                    continuation.resume(throwing: error)
                }
                return
            }
            guard let status = statusCode(of: task) else {
                continuation.resume(throwing: LayerDownloadError.noResponse)
                return
            }
            guard status == 200 else {
                continuation.resume(throwing: LayerDownloadError.badStatus(status))
                return
            }
            if let moveError {
                continuation.resume(throwing: moveError)
            } else if !moved {
                continuation.resume(throwing: LayerDownloadError.noResponse)
            } else {
                continuation.resume(returning: written)
            }
        }
    }
}
