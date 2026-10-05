// SPDX-License-Identifier: MIT

import CryptoKit
import Foundation
import Testing

@testable import lume

// MARK: - Helpers

/// A settable clock for the progress tracker.
private final class TestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var current = Date(timeIntervalSince1970: 1_000_000)

    var now: Date { lock.withLock { current } }

    func advance(_ seconds: TimeInterval) {
        lock.withLock { current = current.addingTimeInterval(seconds) }
    }
}

/// Collects the progress reports a tracker emits.
private final class ProgressLog: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [PullProgress] = []

    func append(_ progress: PullProgress) { lock.withLock { items.append(progress) } }
    var all: [PullProgress] { lock.withLock { items } }
}

private func makeTracker(clock: TestClock, interval: TimeInterval = 0.15) async
    -> (ProgressTracker, ProgressLog)
{
    let tracker = ProgressTracker(
        now: { clock.now }, emitInterval: interval, printsToTerminal: false)
    let log = ProgressLog()
    await tracker.setProgressHandler { log.append($0) }
    return (tracker, log)
}

private func temporaryDirectory(_ label: String) throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("lume-pull-tests-\(label)-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

private func sha256Digest(_ data: Data) -> String {
    "sha256:" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

// MARK: - Local blob server (URLProtocol)

/// How the stub answers one blob request.
struct StubBlobPlan: Sendable {
    var status: Int = 200
    var body: Data
    var chunkSize: Int = 64 * 1024
    var delay: TimeInterval = 0.01
}

/// Plans per blob path. Each request takes the next plan; the last one repeats.
final class StubBlobPlans: @unchecked Sendable {
    private let lock = NSLock()
    private var plans: [String: [StubBlobPlan]] = [:]
    private var requests: [String: Int] = [:]

    func set(_ path: String, _ list: [StubBlobPlan]) { lock.withLock { plans[path] = list } }

    func next(for path: String) -> StubBlobPlan? {
        lock.withLock {
            requests[path, default: 0] += 1
            guard var list = plans[path], !list.isEmpty else { return nil }
            let plan = list[0]
            if list.count > 1 {
                list.removeFirst()
                plans[path] = list
            }
            return plan
        }
    }

    func requestCount(_ path: String) -> Int { lock.withLock { requests[path] ?? 0 } }
}

private final class RunLoopBox: @unchecked Sendable {
    let loop: CFRunLoop?
    init(_ loop: CFRunLoop?) { self.loop = loop }
}

/// Serves blobs for `stub.lume.test` in chunks, like a slow registry.
final class StubBlobProtocol: URLProtocol, @unchecked Sendable {
    static let host = "stub.lume.test"
    static let plans = StubBlobPlans()

    private let stateLock = NSLock()
    private var stopped = false

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == host
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url, let plan = Self.plans.next(for: url.path) else {
            client?.urlProtocol(self, didFailWithError: URLError(.fileDoesNotExist))
            return
        }
        // Client callbacks go to the run loop that started loading.
        let runLoop = RunLoopBox(CFRunLoopGetCurrent())
        let perform: @Sendable (@escaping @Sendable () -> Void) -> Void = { block in
            CFRunLoopPerformBlock(runLoop.loop, CFRunLoopMode.commonModes.rawValue, block)
            CFRunLoopWakeUp(runLoop.loop)
        }
        let response = HTTPURLResponse(
            url: url, statusCode: plan.status, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Length": "\(plan.body.count)"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)

        let queue = DispatchQueue(label: "stub-blob")
        let body = plan.body
        @Sendable func send(from offset: Int) {
            perform { [self] in
                guard !isStopped else { return }
                if offset >= body.count {
                    client?.urlProtocolDidFinishLoading(self)
                    return
                }
                let end = min(offset + plan.chunkSize, body.count)
                client?.urlProtocol(self, didLoad: body.subdata(in: offset..<end))
                queue.asyncAfter(deadline: .now() + plan.delay) { send(from: end) }
            }
        }
        queue.async { send(from: 0) }
    }

    private var isStopped: Bool { stateLock.withLock { stopped } }

    override func stopLoading() {
        stateLock.withLock { stopped = true }
    }
}

private func makeStubRegistry(cache: URL) -> ImageContainerRegistry {
    let registry = ImageContainerRegistry(
        registry: StubBlobProtocol.host, organization: "org", cacheDirectory: cache,
        cachingEnabled: true)
    registry.urlProtocolClasses = [StubBlobProtocol.self]
    registry.retryBaseDelay = 0.01
    return registry
}

private func randomBlob(_ size: Int) -> Data {
    var generator = SystemRandomNumberGenerator()
    return Data((0..<size).map { _ in UInt8.random(in: 0...255, using: &generator) })
}

// MARK: - Tracker byte accounting

struct ProgressTrackerByteTests {
    @Test("in-flight bytes count before a layer finishes, late updates are ignored")
    func inFlightBytes() async {
        let clock = TestClock()
        let (tracker, _) = await makeTracker(clock: clock, interval: 0)
        await tracker.setTotal(1000, files: 2)

        await tracker.updateTransfer(id: "a", bytesWritten: 100)
        #expect(await tracker.snapshot().downloadedBytes == 100)
        await tracker.updateTransfer(id: "b", bytesWritten: 50)
        #expect(await tracker.snapshot().downloadedBytes == 150)
        // A stale (smaller) running count never moves the total backwards.
        await tracker.updateTransfer(id: "a", bytesWritten: 80)
        #expect(await tracker.snapshot().downloadedBytes == 150)

        await tracker.finishTransfer(id: "a", bytes: 400)
        #expect(await tracker.snapshot().downloadedBytes == 450)
        // An update that arrives after the transfer finished is ignored.
        await tracker.updateTransfer(id: "a", bytesWritten: 300)
        #expect(await tracker.snapshot().downloadedBytes == 450)
        #expect(await tracker.snapshot().percent == 45)
        #expect(await tracker.snapshot().totalBytes == 1000)
    }

    @Test("a retried layer drops its in-flight bytes so nothing is counted twice")
    func retryUndo() async {
        let clock = TestClock()
        let (tracker, log) = await makeTracker(clock: clock, interval: 0)
        await tracker.setTotal(10_000, files: 1)

        await tracker.updateTransfer(id: "layer#1", bytesWritten: 600)
        #expect(await tracker.snapshot().downloadedBytes == 600)
        await tracker.dropTransfer(id: "layer#1")
        #expect(await tracker.snapshot().downloadedBytes == 0)
        // Late bytes of the dropped attempt stay ignored.
        await tracker.updateTransfer(id: "layer#1", bytesWritten: 700)
        #expect(await tracker.snapshot().downloadedBytes == 0)

        await tracker.updateTransfer(id: "layer#2", bytesWritten: 1000)
        await tracker.finishTransfer(id: "layer#2", bytes: 1000)
        #expect(await tracker.snapshot().downloadedBytes == 1000)
        #expect(log.all.allSatisfy { $0.downloadedBytes <= 1000 })
    }

    @Test("a cached layer counts as done at once and does not inflate the rate")
    func cachedCopy() async {
        let clock = TestClock()
        let (tracker, log) = await makeTracker(clock: clock, interval: 0)
        await tracker.setTotal(2000, files: 2)
        clock.advance(1)
        await tracker.addProgress(1500)
        #expect(await tracker.snapshot().downloadedBytes == 1500)
        #expect(log.all.last?.downloadedBytes == 1500)
        #expect(await tracker.snapshot().bytesPerSecond == 0)
    }

    @Test("the rate is smoothed over time")
    func rateSmoothing() async {
        let clock = TestClock()
        let (tracker, _) = await makeTracker(clock: clock, interval: 0.15)
        await tracker.setTotal(1_000_000_000, files: 1)

        // A steady 1 MB/s in 0.2 s steps converges on 1 MB/s.
        var written: Int64 = 0
        for _ in 0..<50 {
            clock.advance(0.2)
            written += 200_000
            await tracker.updateTransfer(id: "t", bytesWritten: written)
        }
        let steady = await tracker.snapshot().bytesPerSecond
        #expect(abs(steady - 1_000_000) < 1_000)

        // One burst (50 MB/s for 0.2 s) moves the rate, but only part of the way.
        clock.advance(0.2)
        written += 10_000_000
        await tracker.updateTransfer(id: "t", bytesWritten: written)
        let afterBurst = await tracker.snapshot().bytesPerSecond
        #expect(afterBurst > 1_000_000)
        #expect(afterBurst < 10_000_000)
    }

    @Test("reports are throttled, but the first and the final one always pass")
    func throttling() async {
        let clock = TestClock()
        let (tracker, log) = await makeTracker(clock: clock, interval: 0.15)
        await tracker.setTotal(1000, files: 1)
        #expect(log.all.count == 1)  // setTotal reports at once

        for i in 1...20 {
            clock.advance(0.01)
            await tracker.updateTransfer(id: "t", bytesWritten: Int64(i * 10))
        }
        // 0.2 s of updates at 100 per second give one report, not twenty.
        #expect(log.all.count == 2)

        clock.advance(0.05)
        await tracker.updateTransfer(id: "t", bytesWritten: 300)
        #expect(log.all.count == 2)
        clock.advance(0.15)
        await tracker.updateTransfer(id: "t", bytesWritten: 400)
        #expect(log.all.count == 3)

        // Reaching the total reports even inside the interval.
        clock.advance(0.01)
        await tracker.finishTransfer(id: "t", bytes: 1000)
        #expect(log.all.count == 4)
        #expect(log.all.last?.downloadedBytes == 1000)
        #expect(log.all.last?.percent == 100)
    }
}

// MARK: - Layer download over URLSession

@Suite(.serialized)
struct LayerDownloadTests {
    @Test("a layer reports bytes while it downloads and lands verified in the cache")
    func byteLevelProgress() async throws {
        let dir = try temporaryDirectory("bytes")
        defer { try? FileManager.default.removeItem(at: dir) }
        let blob = randomBlob(1_048_576)
        let digest = sha256Digest(blob)
        StubBlobProtocol.plans.set(
            "/v2/org/img/blobs/\(digest)",
            [StubBlobPlan(body: blob, chunkSize: 32 * 1024, delay: 0.005)])

        let registry = makeStubRegistry(cache: dir.appendingPathComponent("cache"))
        let clock = TestClock()
        let (tracker, log) = await makeTracker(clock: clock, interval: 0)
        await tracker.setTotal(Int64(blob.count), files: 1)
        let target = dir.appendingPathComponent("layer")

        try await registry.downloadLayer(
            repository: "org/img", digest: digest, mediaType: "application/octet-stream",
            token: "", to: target, maxRetries: 2, progress: tracker, manifestId: "m1")

        let partial = Swift.Set(
            log.all.map(\.downloadedBytes).filter { $0 > 0 && $0 < Int64(blob.count) })
        #expect(partial.count >= 5)
        #expect(await tracker.snapshot().downloadedBytes == Int64(blob.count))
        #expect((try Data(contentsOf: target) == blob) == true)
        let cached = registry.getCachedLayerPath(manifestId: "m1", digest: digest)
        #expect((try Data(contentsOf: cached) == blob) == true)
        let cacheFiles = try FileManager.default.contentsOfDirectory(
            atPath: cached.deletingLastPathComponent().path)
        #expect(cacheFiles == [cached.lastPathComponent])
    }

    @Test("a corrupt first attempt is retried without counting its bytes twice")
    func retryDoesNotOvercount() async throws {
        let dir = try temporaryDirectory("retry")
        defer { try? FileManager.default.removeItem(at: dir) }
        let blob = randomBlob(256 * 1024)
        let digest = sha256Digest(blob)
        var corrupt = blob
        corrupt[0] ^= 0xFF
        let path = "/v2/org/img/blobs/\(digest)"
        StubBlobProtocol.plans.set(
            path,
            [
                StubBlobPlan(body: corrupt, chunkSize: 64 * 1024, delay: 0.001),
                StubBlobPlan(body: blob, chunkSize: 64 * 1024, delay: 0.001),
            ])

        let registry = makeStubRegistry(cache: dir.appendingPathComponent("cache"))
        let clock = TestClock()
        let (tracker, log) = await makeTracker(clock: clock, interval: 0)
        // A total well above the blob keeps the cap from hiding an overcount.
        await tracker.setTotal(Int64(blob.count) * 4, files: 1)

        try await registry.downloadLayer(
            repository: "org/img", digest: digest, mediaType: "application/octet-stream",
            token: "", to: dir.appendingPathComponent("layer"), maxRetries: 3,
            progress: tracker, manifestId: "m1")

        #expect(StubBlobProtocol.plans.requestCount(path) == 2)
        // Let late progress tasks of the first attempt land, then check the total.
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(await tracker.snapshot().downloadedBytes == Int64(blob.count))
        #expect(log.all.allSatisfy { $0.downloadedBytes <= Int64(blob.count) })
    }

    @Test("cancelling stops the transfer, leaves no partial file and clears the marker")
    func cancelStopsTransfer() async throws {
        let dir = try temporaryDirectory("cancel")
        defer { try? FileManager.default.removeItem(at: dir) }
        let blob = randomBlob(2 * 1_048_576)
        let digest = sha256Digest(blob)
        // About 6 seconds for the whole blob.
        StubBlobProtocol.plans.set(
            "/v2/org/img/blobs/\(digest)",
            [StubBlobPlan(body: blob, chunkSize: 32 * 1024, delay: 0.1)])

        let registry = makeStubRegistry(cache: dir.appendingPathComponent("cache"))
        let clock = TestClock()
        let (tracker, _) = await makeTracker(clock: clock, interval: 0)
        await tracker.setTotal(Int64(blob.count), files: 1)
        let target = dir.appendingPathComponent("layer")

        registry.markDownloadStarted(digest)
        let task = Task {
            try await registry.downloadLayer(
                repository: "org/img", digest: digest, mediaType: "application/octet-stream",
                token: "", to: target, maxRetries: 5, progress: tracker, manifestId: "m1")
        }
        try await Task.sleep(nanoseconds: 600_000_000)
        #expect(await tracker.snapshot().downloadedBytes > 0)

        let cancelledAt = Date()
        task.cancel()
        let result = await task.result
        #expect(Date().timeIntervalSince(cancelledAt) < 1.5)
        #expect(throws: CancellationError.self) { try result.get() }

        #expect(!FileManager.default.fileExists(atPath: target.path))
        let cached = registry.getCachedLayerPath(manifestId: "m1", digest: digest)
        #expect(!FileManager.default.fileExists(atPath: cached.path))
        #expect(!registry.isDownloading(digest))
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(await tracker.snapshot().downloadedBytes == 0)
    }

    @Test("cancelling during a retry backoff ends the download instead of retrying")
    func cancelDuringBackoff() async throws {
        let dir = try temporaryDirectory("backoff")
        defer { try? FileManager.default.removeItem(at: dir) }
        let digest = sha256Digest(Data("never served".utf8))
        let path = "/v2/org/img/blobs/\(digest)"
        StubBlobProtocol.plans.set(path, [StubBlobPlan(status: 500, body: Data("boom".utf8))])

        let registry = makeStubRegistry(cache: dir.appendingPathComponent("cache"))
        registry.retryBaseDelay = 30
        let clock = TestClock()
        let (tracker, _) = await makeTracker(clock: clock, interval: 0)

        registry.markDownloadStarted(digest)
        let task = Task {
            try await registry.downloadLayer(
                repository: "org/img", digest: digest, mediaType: "application/octet-stream",
                token: "", to: dir.appendingPathComponent("layer"), maxRetries: 5,
                progress: tracker, manifestId: "m1")
        }
        try await Task.sleep(nanoseconds: 500_000_000)
        let cancelledAt = Date()
        task.cancel()
        let result = await task.result
        #expect(Date().timeIntervalSince(cancelledAt) < 1.5)
        #expect(throws: CancellationError.self) { try result.get() }
        #expect(StubBlobProtocol.plans.requestCount(path) == 1)
        #expect(!registry.isDownloading(digest))
    }
}

// MARK: - Cancelled pulls leave no VM behind

@MainActor
struct PullCancelCleanupTests {
    @Test("a cancelled pull removes the VM directory it created")
    func removesNewDirectory() async throws {
        let dir = try temporaryDirectory("vmdir")
        defer { try? FileManager.default.removeItem(at: dir) }
        let vmDir = VMDirectory(Path(dir.appendingPathComponent("cux-test")))

        let task = Task { @MainActor in
            try await LumeController.removingNewVMDirectoryOnCancel(vmDir) {
                try FileManager.default.createDirectory(
                    at: vmDir.dir.url, withIntermediateDirectories: true)
                try await Task.sleep(nanoseconds: 30_000_000_000)
            }
        }
        try await Task.sleep(nanoseconds: 200_000_000)
        #expect(FileManager.default.fileExists(atPath: vmDir.dir.path))
        task.cancel()
        let result = await task.result
        #expect(throws: CancellationError.self) { try result.get() }
        #expect(!FileManager.default.fileExists(atPath: vmDir.dir.path))
    }

    @Test("a cancel that lands after the pull finished still removes the new VM")
    func removesAfterLateCancel() async throws {
        let dir = try temporaryDirectory("vmdir-late")
        defer { try? FileManager.default.removeItem(at: dir) }
        let vmDir = VMDirectory(Path(dir.appendingPathComponent("cux-test")))

        let task = Task { @MainActor in
            try await LumeController.removingNewVMDirectoryOnCancel(vmDir) {
                try FileManager.default.createDirectory(
                    at: vmDir.dir.url, withIntermediateDirectories: true)
                withUnsafeCurrentTask { $0?.cancel() }
            }
        }
        let result = await task.result
        #expect(throws: CancellationError.self) { try result.get() }
        #expect(!FileManager.default.fileExists(atPath: vmDir.dir.path))
    }

    @Test("a VM directory that existed before the pull is never removed")
    func keepsExistingDirectory() async throws {
        let dir = try temporaryDirectory("vmdir-existing")
        defer { try? FileManager.default.removeItem(at: dir) }
        let vmDir = VMDirectory(Path(dir.appendingPathComponent("cux-test")))
        try FileManager.default.createDirectory(
            at: vmDir.dir.url, withIntermediateDirectories: true)

        let task = Task { @MainActor in
            try await LumeController.removingNewVMDirectoryOnCancel(vmDir) {
                try await Task.sleep(nanoseconds: 30_000_000_000)
            }
        }
        try await Task.sleep(nanoseconds: 100_000_000)
        task.cancel()
        _ = await task.result
        #expect(FileManager.default.fileExists(atPath: vmDir.dir.path))
    }
}

// MARK: - HTTP routes

@MainActor
@Suite(.serialized)
struct PullRouteTests {
    private func json(_ response: HTTPResponse) throws -> [String: Any] {
        let body = try #require(response.body)
        return try #require(try JSONSerialization.jsonObject(with: body) as? [String: Any])
    }

    private func request(_ method: String, _ path: String, _ body: [String: Any]? = nil)
        throws -> HTTPRequest
    {
        HTTPRequest(
            method: method, path: path, headers: ["Content-Type": "application/json"],
            body: try body.map { try JSONSerialization.data(withJSONObject: $0) })
    }

    /// Starts a fake async pull: it runs until cancelled, then removes its
    /// scratch directory (standing in for the pull's temp files) and reports
    /// the outcome, like `handlePullStart`'s task.
    private func startFakePull(name: String, scratch: URL) async -> Task<Void, Never> {
        let tracker = PullProgressTracker.shared
        let token = await tracker.begin(name: name, cancellable: true)
        let task = Task.detached {
            var outcome = PullProgressTracker.Outcome.completed
            do {
                while true { try await Task.sleep(nanoseconds: 20_000_000) }
            } catch {
                // Cleanup takes a moment; the cancel must wait for it.
                usleep(300_000)
                try? FileManager.default.removeItem(at: scratch)
                outcome = .cancelled
            }
            await tracker.finish(name: name, token: token, outcome: outcome)
        }
        await tracker.attach(task, name: name, token: token)
        await tracker.setProgress(
            PullProgress(
                percent: 25, downloadedBytes: 250_000, totalBytes: 1_000_000,
                bytesPerSecond: 12_500.5),
            for: name, token: token)
        return task
    }

    @Test("GET while pulling carries percent, bytes, total and rate")
    func getWhilePulling() async throws {
        let name = "pull-get-\(UUID().uuidString)"
        let scratch = try temporaryDirectory("get")
        defer { try? FileManager.default.removeItem(at: scratch) }
        let task = await startFakePull(name: name, scratch: scratch)
        defer { task.cancel() }

        let server = Server(port: 0)
        let response = try await server.handleRequest(try request("GET", "/lume/vms/\(name)"))
        #expect(response.statusCode == .ok)
        let body = try json(response)
        #expect(body["name"] as? String == name)
        #expect(body["status"] as? String == "pulling")
        #expect(body["downloadProgress"] as? Double == 25)
        #expect((body["downloadedBytes"] as? NSNumber)?.int64Value == 250_000)
        #expect((body["totalBytes"] as? NSNumber)?.int64Value == 1_000_000)
        #expect(body["bytesPerSecond"] as? Double == 12_500.5)
    }

    @Test("cancel with nothing running is a 404")
    func cancelNothing() async throws {
        let name = "pull-none-\(UUID().uuidString)"
        let server = Server(port: 0)
        let response = try await server.handleRequest(
            try request("POST", "/lume/pull/cancel", ["name": name]))
        #expect(response.statusCode == .notFound)
        #expect(try json(response)["message"] as? String == "no pull in progress for \(name)")

        let bad = try await server.handleRequest(try request("POST", "/lume/pull/cancel", [:]))
        #expect(bad.statusCode == .badRequest)
    }

    @Test("cancel stops the pull, waits for its cleanup and clears the pulling state")
    func cancelRunningPull() async throws {
        let name = "pull-cancel-\(UUID().uuidString)"
        let scratch = try temporaryDirectory("cancel-route")
        defer { try? FileManager.default.removeItem(at: scratch) }
        let task = await startFakePull(name: name, scratch: scratch)
        let server = Server(port: 0)

        let response = try await server.handleRequest(
            try request("POST", "/lume/pull/cancel", ["name": name]))
        #expect(response.statusCode == .ok)
        let body = try json(response)
        #expect(body["message"] as? String == "Pull cancelled")
        #expect(body["name"] as? String == name)
        // The task already ran its cleanup when the cancel returned.
        #expect(!FileManager.default.fileExists(atPath: scratch.path))
        #expect(task.isCancelled)
        #expect(await PullProgressTracker.shared.getPullProgress(for: name) == nil)
        #expect(await PullProgressTracker.shared.getError(for: name) == nil)
        #expect(await !PullProgressTracker.shared.isPulling(name))

        // GET no longer says pulling (it falls through to the VM lookup).
        let get = try await server.handleRequest(try request("GET", "/lume/vms/\(name)"))
        let getBody = try? json(get)
        #expect(getBody?["status"] as? String != "pulling")

        // A second cancel finds nothing.
        let again = try await server.handleRequest(
            try request("POST", "/lume/pull/cancel", ["name": name]))
        #expect(again.statusCode == .notFound)
    }

    @Test("a progress update that arrives after a cancel does not bring the pull back")
    func lateProgressIgnored() async throws {
        let name = "pull-late-\(UUID().uuidString)"
        let tracker = PullProgressTracker.shared
        let token = await tracker.begin(name: name, cancellable: true)
        let task = Task.detached {
            while !Task.isCancelled { try? await Task.sleep(nanoseconds: 10_000_000) }
            await tracker.finish(name: name, token: token, outcome: .cancelled)
        }
        await tracker.attach(task, name: name, token: token)
        #expect(await tracker.cancel(name: name, timeout: 5) == .cancelled)
        await tracker.setProgress(
            PullProgress(percent: 50, downloadedBytes: 5, totalBytes: 10, bytesPerSecond: 1),
            for: name, token: token)
        #expect(await tracker.getPullProgress(for: name) == nil)
    }

    @Test("a pull that ignores the cancel makes the route time out instead of hanging")
    func cancelTimesOut() async throws {
        let name = "pull-stuck-\(UUID().uuidString)"
        let tracker = PullProgressTracker.shared
        let token = await tracker.begin(name: name, cancellable: true)
        let release = Task.detached {
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            await tracker.finish(name: name, token: token, outcome: .cancelled)
        }
        // A task that never ends by itself (finish comes from `release`).
        await tracker.attach(Task.detached {}, name: name, token: token)

        let server = Server(port: 0)
        let response = try await server.handlePullCancel(
            try JSONSerialization.data(withJSONObject: ["name": name]), timeout: 0.3)
        #expect(response.statusCode == .internalServerError)
        _ = await release.value
        #expect(await !tracker.isPulling(name))
    }
}

// MARK: - Shared blobs are decompressed once

struct SharedBlobWriteTests {
    @Test("one decompression writes a shared blob at every offset that uses it")
    func writesEveryOffset() throws {
        let dir = try temporaryDirectory("shared-blob")
        defer { try? FileManager.default.removeItem(at: dir) }
        let blob = randomBlob(300_000)
        let raw = dir.appendingPathComponent("blob.raw-source")
        try blob.write(to: raw)
        let gz = dir.appendingPathComponent("blob.gz")
        FileManager.default.createFile(atPath: gz.path, contents: nil)
        let gzHandle = try FileHandle(forWritingTo: gz)
        let gzip = Process()
        gzip.executableURL = URL(fileURLWithPath: "/usr/bin/gzip")
        gzip.arguments = ["-c", raw.path]
        gzip.standardOutput = gzHandle
        try gzip.run()
        gzip.waitUntilExit()
        try gzHandle.close()

        let disk = dir.appendingPathComponent("disk.img")
        FileManager.default.createFile(atPath: disk.path, contents: nil)
        let output = try FileHandle(forWritingTo: disk)
        try output.truncate(atOffset: 16 * 1024 * 1024)
        let offsets: [UInt64] = [0, 5 * 1024 * 1024, 9 * 1024 * 1024 + 17]
        let size = try gunzipChunkAndWriteSparse(
            inputPath: gz, outputHandle: output, startOffsets: offsets)
        try output.close()
        #expect(size == UInt64(blob.count))

        let reader = try FileHandle(forReadingFrom: disk)
        defer { try? reader.close() }
        for offset in offsets {
            try reader.seek(toOffset: offset)
            // Compare as a Bool: a failing Data comparison would diff 300 KB.
            let matches = reader.readData(ofLength: blob.count) == blob
            #expect(matches, "blob missing at offset \(offset)")
        }
    }
}
